//! Engine thread entry point and the deterministic headless driver.
//!
//! The real engine (`spawn_engine`) runs `step_engine` in a loop on a monotonic clock;
//! the test driver (`run_engine_headless`) runs the *same* `step_engine` over a virtual
//! clock. Only the headless driver is unit-tested — the threaded one is not deterministic.

pub mod clock;
pub mod scheduler;
pub mod transport;

use crate::devices::profiles::DeviceProfile;
use crate::link::LinkClock;
use crate::midi::ports::{connect, list_output_ports, match_port, MidiSink, NullSink};
use crate::pattern::model::{Lane, Pattern, Set};
use clock::ClockGen;
use scheduler::Sequencer;
use transport::{TempoSource, Transport};

/// Commands sent UI -> engine.
#[derive(Clone, Debug, PartialEq)]
pub enum UiCommand {
    Play,
    Stop,
    SetBpm(f64),
    Tap,
    SetSwing(f32),
    ToggleLink(bool),
    LoadPattern {
        lane: usize,
        pattern: Pattern,
    },
    Mute {
        lane: usize,
        on: bool,
    },
    Solo {
        lane: usize,
        on: bool,
    },
    Transpose {
        lane: usize,
        semis: i8,
    },
    /// Sync all lane state after undo/redo. Does NOT rebuild the Sequencer or reset the clock/playhead.
    SyncLanes(Vec<Lane>),
    /// Update a single lane's octave shift without touching anything else.
    SetOctave {
        lane: usize,
        octave: i8,
    },
    SetSet(Set),
    /// All-notes-off / all-sound-off live recovery; does not stop transport.
    Panic,
    Quit,
}

/// Events sent engine -> UI.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Playhead {
        step: usize,
        bar: u32,
        beat: u32,
        phase: f32,
    },
    LinkStatus {
        enabled: bool,
        tempo: f64,
        peers: u64,
    },
    DeviceStatus {
        lane: usize,
        connected: bool,
        port: String,
    },
    /// Engine-confirmed: Play was received with Link enabled; waiting for the quantized bar
    /// boundary. The sequencer is NOT yet running — the UI should show an "armed" indicator.
    Armed,
    /// Engine-confirmed: sequencer has started playing (step 0 on manual; bar boundary on Link).
    Started { at_step: usize },
    /// Engine-confirmed: sequencer has stopped.
    Stopped,
}

/// Handle returned by `spawn_engine`.
pub struct EngineHandle {
    pub tx: crossbeam_channel::Sender<UiCommand>,
    pub rx: crossbeam_channel::Receiver<EngineEvent>,
    pub join: std::thread::JoinHandle<()>,
}

/// Emit a `LinkStatus` event roughly this often (in ticks) to avoid flooding.
const LINK_STATUS_EVERY: u64 = 200;

/// Mutable engine state shared by both drivers.
struct EngineState {
    seq: Sequencer,
    clock: ClockGen,
    transport: Transport,
    link_enabled: bool,
    /// True after a Link-gated Play: waiting for the quantized bar boundary before
    /// actually starting the sequencer. While armed, `seq.playing` is false so no
    /// notes escape.
    armed: bool,
    last_step: Option<usize>,
    bar: u32,
    tick_count: u64,
}

impl EngineState {
    fn new(set: Set) -> Self {
        let bpm = set.bpm;
        let mut transport = Transport::new();
        transport.manual_bpm = bpm;
        transport.source = TempoSource::Manual(bpm);
        EngineState {
            seq: Sequencer::new(set),
            clock: ClockGen::new(),
            transport,
            link_enabled: false,
            armed: false,
            last_step: None,
            bar: 0,
            tick_count: 0,
        }
    }
}

/// Apply a single UI command to engine state at time `now`.
/// Returns `true` if a `Quit` was processed (signals loop exit).
fn apply_command(
    st: &mut EngineState,
    cmd: UiCommand,
    now: u64,
    link: &mut dyn LinkClock,
    sink: &mut dyn MidiSink,
    events: &mut Vec<EngineEvent>,
) -> bool {
    match cmd {
        UiCommand::Play => {
            if link.enabled() {
                // Link mode: defer sequencer start to the quantized bar boundary.
                // While armed, seq.playing is false so no notes escape.
                link.request_start(now, 4.0);
                st.clock.start(now);
                st.armed = true;
                events.push(EngineEvent::Armed);
            } else {
                // Manual mode: start immediately and confirm.
                st.seq.play(now);
                st.clock.start(now); // begin Clock ticks only — no MIDI Start (would run the device's own sequencer)
                events.push(EngineEvent::Started { at_step: 0 });
            }
        }
        UiCommand::Stop => {
            st.seq.stop(now, sink); // releases sounding notes (all-notes-off)
            st.clock.stop(); // cease Clock ticks; no MIDI Stop sent
            st.armed = false;
            events.push(EngineEvent::Stopped);
        }
        UiCommand::SetBpm(bpm) => {
            st.transport.manual_bpm = bpm;
            st.transport.source = TempoSource::Manual(bpm);
            st.seq.set_bpm(bpm);
        }
        UiCommand::Tap => {
            st.transport.tap(now);
            st.transport.source = TempoSource::Manual(st.transport.manual_bpm);
            st.seq.set_bpm(st.transport.manual_bpm);
        }
        UiCommand::SetSwing(s) => {
            st.seq.set_swing(s);
        }
        UiCommand::ToggleLink(on) => {
            st.link_enabled = on;
            link.set_enabled(on);
            st.transport.source = if on {
                TempoSource::Link
            } else {
                TempoSource::Manual(st.transport.manual_bpm)
            };
            // If we were armed waiting for a Link boundary and Link is turned off,
            // start the sequencer immediately.
            if !on && st.armed {
                st.seq.play(now);
                st.armed = false;
                events.push(EngineEvent::Started { at_step: 0 });
            }
        }
        UiCommand::LoadPattern { lane, pattern } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.pattern = pattern;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::Mute { lane, on } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.mute = on;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::Solo { lane, on } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.solo = on;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::Transpose { lane, semis } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.transpose = semis;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::SyncLanes(lanes) => {
            // Restore all lane state from an undo/redo snapshot without disturbing the
            // clock or playhead — the engine keeps playing from the current position.
            for (i, lane) in lanes.into_iter().enumerate() {
                st.seq.update_lane(i, lane);
            }
        }
        UiCommand::SetOctave { lane, octave } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.octave = octave;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::SetSet(set) => {
            // Grab the new BPM before `set` is moved into Sequencer::new.
            let new_bpm = set.bpm;
            let playing = st.seq.is_playing();
            // Release every sounding note via the registry before dropping the old sequencer
            // (P2: slide/held notes would otherwise hang on hardware).
            st.seq.release_all(now, sink);
            st.seq = Sequencer::new(set);
            // Sync transport so the MIDI clock and note timing agree on the new BPM (bug 2).
            st.transport.manual_bpm = new_bpm;
            if !st.link_enabled {
                st.transport.source = TempoSource::Manual(new_bpm);
            }
            if playing {
                st.seq.play(now);
            }
        }
        UiCommand::Panic => {
            // All-notes-off / all-sound-off on every lane channel. Does NOT touch the
            // transport or clock — playback keeps running while stuck notes are cleared.
            st.seq.panic(now, sink);
        }
        UiCommand::Quit => {
            // Release all sounding notes before exiting — avoids hanging notes on hardware.
            st.seq.panic(now, sink);
            return true;
        }
    }
    false
}

/// One iteration of the engine loop at virtual/real time `now`.
/// Processes all commands due at or before `now`, ticks the sequencer and clock, and
/// appends any `EngineEvent`s to `events`. Returns true if a Quit was processed.
fn step_engine(
    st: &mut EngineState,
    now: u64,
    pending: &mut Vec<(u64, UiCommand)>,
    link: &mut dyn LinkClock,
    sink: &mut dyn MidiSink,
    events: &mut Vec<EngineEvent>,
) -> bool {
    // 1. Process all commands due at or before `now`, in order.
    let mut quit = false;
    let mut i = 0;
    while i < pending.len() {
        if pending[i].0 <= now {
            let (_, cmd) = pending.remove(i);
            if apply_command(st, cmd, now, link, sink, events) {
                quit = true;
            }
        } else {
            i += 1;
        }
    }
    if quit {
        return true;
    }

    // 2. Tempo source resolution via Transport::effective_bpm.
    //    `ToggleLink` keeps `transport.source` in sync (Manual ↔ Link), so this call is
    //    the single authoritative BPM resolution path for both headless tests and the real
    //    engine thread.
    let link_tempo = if st.link_enabled {
        Some(link.tempo())
    } else {
        None
    };
    let bpm = st.transport.effective_bpm(link_tempo);

    // Link-gated start: once the quantized bar boundary is reached (beat >= 0),
    // fire the sequencer. While armed, seq.playing is false so tick emits nothing.
    if st.armed && st.link_enabled && link.beat_at(now, 4.0) >= 0.0 {
        st.seq.play(now);
        st.armed = false;
        events.push(EngineEvent::Started { at_step: 0 });
    }

    if st.link_enabled {
        let beat = link.beat_at(now, 4.0);
        st.seq.sync_to_beat(beat, bpm);
    }

    // 3. Advance sequencer + clock.
    let advanced = st.seq.tick(now, sink);
    st.clock.tick(now, bpm, sink);

    // 4. Emit a Playhead event on step advance.
    let cur = st.seq.current_step();
    let should_emit = advanced.is_some() || (st.last_step.is_none() && st.seq.is_playing());
    if should_emit {
        st.last_step = Some(cur);
        // Absolute monotonic step -> bar/beat (4/4: 16 sixteenths per bar, 4 per beat).
        st.bar = (cur / 16) as u32;
        let beat = ((cur / 4) % 4) as u32;
        let phase = (cur % 4) as f32 / 4.0;
        events.push(EngineEvent::Playhead {
            step: cur,
            bar: st.bar,
            beat,
            phase,
        });
    }

    // 5. Periodic Link status.
    if st.tick_count.is_multiple_of(LINK_STATUS_EVERY) {
        events.push(EngineEvent::LinkStatus {
            enabled: st.link_enabled,
            tempo: link.tempo(),
            peers: link.num_peers(),
        });
    }
    st.tick_count += 1;

    false
}

/// Headless, deterministic driver for tests: feeds `commands` at their timestamps, advances a
/// virtual clock from 0 to `total_micros` in `tick` steps, returns emitted `EngineEvent`s.
/// Sink is caller-injected; no hot-plug rescan runs here (hardware/integration-only path).
pub fn run_engine_headless(
    set: Set,
    link: &mut dyn LinkClock,
    sink: &mut dyn MidiSink,
    commands: Vec<(u64, UiCommand)>,
    total_micros: u64,
    tick: u64,
) -> Vec<EngineEvent> {
    let mut st = EngineState::new(set);
    let mut pending = commands;
    let mut events = Vec::new();
    let tick = tick.max(1);

    // Run now from 0 up to (but not including) total_micros so that events
    // scheduled exactly at total_micros (i.e. the first step of the *next* bar)
    // are not emitted. Commands timestamped at 0 fire on the first iteration.
    let mut now: u64 = 0;
    loop {
        if step_engine(&mut st, now, &mut pending, link, sink, &mut events) {
            break;
        }
        now = now.saturating_add(tick);
        if now >= total_micros {
            break;
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Per-PORT sink state for the real engine (hot-plug lifecycle).
//
// CRITICAL INVARIANT: exactly ONE MIDI connection per distinct physical port.
// Lanes that resolve to the same `port_match` (e.g. the T-8's drum + bass lanes
// both match "T-8") COLLAPSE to a single connection. The fan-out sends each
// message once per PORT, so the 24 PPQN MIDI clock reaches a shared device ONCE.
// Two connections to one port would deliver the clock twice and the device would
// read DOUBLE tempo — a hardware-confirmed bug fixed in "open one MIDI connection
// per physical port…" and which this port-level model preserves.
// ---------------------------------------------------------------------------

/// One connection per distinct physical port. `sink` is the single live connection (or
/// `NullSink` when disconnected). The device-watcher thread owns the port_match → sink
/// mapping (it holds the distinct `port_match` list itself), so the engine only carries
/// the installed sink + its last-known state for change-detection and per-lane status.
struct PortSink {
    /// The single sink for this port (NullSink when not connected).
    sink: Box<dyn MidiSink>,
    /// Last-known connection state (for change-detection / dedupe).
    connected: bool,
    /// Last-known connected port name (empty when disconnected).
    port_name: String,
}

/// Distinct-port plan derived purely from profiles — NO hardware access.
///
/// Returns `(distinct, lane_to_port)` where `distinct[i]` is the i-th distinct `port_match`
/// substring in first-seen order, and `lane_to_port[lane]` is the index into `distinct` for
/// that lane. Lanes sharing a `port_match` map to the SAME port index — this is the dedup
/// that guarantees one connection per physical port. Mirrors the old `unique_ports`.
fn map_lanes_to_ports(profiles: &[DeviceProfile; 3]) -> (Vec<&'static str>, [usize; 3]) {
    let mut distinct: Vec<&'static str> = Vec::new();
    let mut lane_to_port = [0usize; 3];
    for (lane, p) in profiles.iter().enumerate() {
        let idx = match distinct.iter().position(|m| *m == p.port_match) {
            Some(i) => i,
            None => {
                distinct.push(p.port_match);
                distinct.len() - 1
            }
        };
        lane_to_port[lane] = idx;
    }
    (distinct, lane_to_port)
}

/// All lanes that resolve to the given port index, in lane order. Used to release the
/// correct notes when a port connects/disconnects/fails (lanes sharing a port move
/// together — the registry tracks per-lane ownership). Pure; UNIT-TESTED.
fn lanes_of(lane_to_port: &[usize; 3], port_idx: usize) -> Vec<usize> {
    lane_to_port
        .iter()
        .enumerate()
        .filter(|(_, &p)| p == port_idx)
        .map(|(lane, _)| lane)
        .collect()
}

/// What the device-watcher should do for a single port this scan. Pure data — no hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortAction {
    Connect(usize),
    Drop(usize),
}

/// Pure connection planner (UNIT-TESTED): compares the just-enumerated presence of each
/// port against its last-known connection state. `present && !connected` → Connect;
/// `!present && connected` → Drop; otherwise nothing. No hardware access — the watcher
/// thread executes the resulting actions.
fn plan_port_actions(present: &[bool], connected: &[bool]) -> Vec<PortAction> {
    let mut actions = Vec::new();
    for (idx, (&p, &c)) in present.iter().zip(connected.iter()).enumerate() {
        if p && !c {
            actions.push(PortAction::Connect(idx));
        } else if !p && c {
            actions.push(PortAction::Drop(idx));
        }
    }
    actions
}

/// watcher → engine. `Box<dyn MidiSink>` is `Send` (`MidiSink: Send`), so a ready-made
/// connection can be moved across the channel — the engine only installs it.
enum PortUpdate {
    Connected {
        idx: usize,
        sink: Box<dyn MidiSink>,
        name: String,
    },
    Disconnected {
        idx: usize,
    },
}

/// engine → watcher. `Reconnect` is sent after the engine drops an unhealthy port so the
/// watcher rebuilds it; `Quit` (or a closed channel) tells the watcher to exit.
enum PortRequest {
    Reconnect(usize),
    Quit,
}

/// Watcher scan cadence. Blocking enumeration/connection is fine here — this is a
/// dedicated thread, never the timing loop.
const WATCHER_SCAN_MS: u64 = 250;

/// Device-watcher thread body. OWNS all port enumeration/connection (the ONLY place that
/// calls `list_output_ports()` / `connect()` in the real engine), so a double-connect is
/// structurally impossible. The first scan runs immediately (no initial sleep) so startup
/// connects within ~one iteration.
fn run_port_watcher(
    port_matches: Vec<&'static str>,
    updates: crossbeam_channel::Sender<PortUpdate>,
    requests: crossbeam_channel::Receiver<PortRequest>,
) {
    // Local mirror of each port's connection state; only this thread mutates it.
    let mut connected = vec![false; port_matches.len()];
    let mut first = true;
    loop {
        if !first {
            std::thread::sleep(std::time::Duration::from_millis(WATCHER_SCAN_MS));
        }
        first = false;

        // Drain engine requests: Reconnect marks a port stale so the planner rebuilds it;
        // Quit (or a dropped sender) ends the thread.
        loop {
            match requests.try_recv() {
                Ok(PortRequest::Reconnect(idx)) => {
                    if let Some(c) = connected.get_mut(idx) {
                        *c = false;
                    }
                }
                Ok(PortRequest::Quit) => return,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => return,
            }
        }

        let available = list_output_ports();
        let present: Vec<bool> = port_matches
            .iter()
            .map(|m| match_port(&available, m).is_some())
            .collect();

        for action in plan_port_actions(&present, &connected) {
            match action {
                PortAction::Connect(idx) => {
                    if let Ok(sink) = connect(port_matches[idx]) {
                        let name = port_display_name(&available, port_matches[idx]);
                        if updates
                            .send(PortUpdate::Connected {
                                idx,
                                sink: Box::new(sink),
                                name,
                            })
                            .is_err()
                        {
                            return; // engine gone
                        }
                        connected[idx] = true;
                    }
                    // Connect failure: leave `connected=false`; retried next scan.
                }
                PortAction::Drop(idx) => {
                    if updates.send(PortUpdate::Disconnected { idx }).is_err() {
                        return;
                    }
                    connected[idx] = false;
                }
            }
        }
    }
}

/// Look up the actual port display name for a `port_match` in the available list,
/// falling back to the `port_match` substring itself.
fn port_display_name(available: &[String], port_match: &str) -> String {
    available
        .iter()
        .find(|n| n.to_lowercase().contains(&port_match.to_lowercase()))
        .cloned()
        .unwrap_or_else(|| port_match.to_string())
}

/// Emit a `DeviceStatus` for every lane, derived from its mapped port's current state.
/// Both lanes sharing a port report the SAME connected/port — consistent with the single
/// underlying connection.
fn emit_lane_status(ports: &[PortSink], lane_to_port: &[usize; 3], events: &mut Vec<EngineEvent>) {
    for (lane, &port_idx) in lane_to_port.iter().enumerate() {
        let ps = &ports[port_idx];
        events.push(EngineEvent::DeviceStatus {
            lane,
            connected: ps.connected,
            port: ps.port_name.clone(),
        });
    }
}

/// Spawn the real engine on its own thread, driven by a monotonic clock. NOT unit-tested
/// (non-deterministic timing); shares `step_engine` with the headless driver.
///
/// Takes `profiles` instead of pre-built sinks. All `PortSink`s start as `NullSink`/
/// disconnected — a dedicated device-watcher thread (the ONLY caller of
/// `list_output_ports()`/`connect()`) owns the entire connection lifecycle and hands the
/// engine ready-made sinks over a channel. The timing loop never enumerates or connects:
/// it only installs delivered sinks, releases notes on loss/route-change (P1), and detects
/// health failures (asking the watcher to reconnect). One connection per distinct physical
/// port (shared-port lanes collapse — no double-clock).
pub fn spawn_engine(
    set: Set,
    mut link: Box<dyn LinkClock>,
    profiles: [DeviceProfile; 3],
) -> EngineHandle {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<UiCommand>();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded::<EngineEvent>();

    let join = std::thread::spawn(move || {
        let mut st = EngineState::new(set);
        let mut pending: Vec<(u64, UiCommand)> = Vec::new();
        let mut events: Vec<EngineEvent> = Vec::new();
        let start = std::time::Instant::now();

        // Distinct-port plan from profiles only (no hardware access on this thread).
        let (distinct, lane_to_port) = map_lanes_to_ports(&profiles);

        // Every port starts disconnected (NullSink); the watcher will connect present ports.
        let mut port_sinks: Vec<PortSink> = distinct
            .iter()
            .map(|_| PortSink {
                sink: Box::new(NullSink),
                connected: false,
                port_name: String::new(),
            })
            .collect();

        // Spawn the device-watcher: it owns enumeration/connection and streams PortUpdates.
        let (update_tx, update_rx) = crossbeam_channel::unbounded::<PortUpdate>();
        let (request_tx, request_rx) = crossbeam_channel::unbounded::<PortRequest>();
        let watcher = {
            let port_matches = distinct.clone();
            std::thread::spawn(move || run_port_watcher(port_matches, update_tx, request_rx))
        };

        // Emit initial (all-disconnected) per-lane DeviceStatus before the first tick;
        // the watcher's first scan (immediate, no initial sleep) flips present ports shortly.
        emit_lane_status(&port_sinks, &lane_to_port, &mut events);
        for ev in events.drain(..) {
            if evt_tx.send(ev).is_err() {
                let _ = request_tx.send(PortRequest::Quit);
                let _ = watcher.join();
                return;
            }
        }

        // Helper: forward queued events to the UI; on a closed channel, shut the watcher
        // down and exit. Returns true if the engine should stop.
        macro_rules! flush_events {
            () => {{
                let mut closed = false;
                for ev in events.drain(..) {
                    if evt_tx.send(ev).is_err() {
                        closed = true;
                        break;
                    }
                }
                closed
            }};
        }

        loop {
            let now = start.elapsed().as_micros() as u64;

            // --- Install ready-made sinks from the watcher (NON-BLOCKING; no enumerate/connect). ---
            while let Ok(update) = update_rx.try_recv() {
                match update {
                    PortUpdate::Connected { idx, sink, name } => {
                        // Route change: if this slot was already live, release its lanes'
                        // notes before swapping so nothing hangs on the outgoing connection.
                        if port_sinks[idx].connected {
                            let lanes = lanes_of(&lane_to_port, idx);
                            let mut fanout = PortFanoutSink {
                                ports: &mut port_sinks,
                            };
                            st.seq.release_lanes(&lanes, now, &mut fanout);
                        }
                        let ps = &mut port_sinks[idx];
                        ps.sink = sink;
                        ps.connected = true;
                        ps.port_name = name;
                        emit_lane_status(&port_sinks, &lane_to_port, &mut events);
                    }
                    PortUpdate::Disconnected { idx } => {
                        // P1: release this port's sounding notes BEFORE swapping to NullSink,
                        // so the NoteOffs go out the still-live connection (not into the void).
                        // `release_lanes` selects by lane, but the fanout broadcasts the NoteOffs to
                        // every port (deliberate, harmless: devices ignore notes on channels they
                        // never sounded — matches the pre-existing per-port fanout model; M4 routing
                        // will make sends port-targeted).
                        let lanes = lanes_of(&lane_to_port, idx);
                        let mut fanout = PortFanoutSink {
                            ports: &mut port_sinks,
                        };
                        st.seq.release_lanes(&lanes, now, &mut fanout);
                        let ps = &mut port_sinks[idx];
                        ps.sink = Box::new(NullSink);
                        ps.connected = false;
                        ps.port_name.clear();
                        emit_lane_status(&port_sinks, &lane_to_port, &mut events);
                    }
                }
            }

            // Fan out once per PORT (a shared port gets each message — incl. clock — ONCE).
            let mut fanout = PortFanoutSink {
                ports: &mut port_sinks,
            };
            let quit = step_engine(
                &mut st,
                now,
                &mut pending,
                link.as_mut(),
                &mut fanout,
                &mut events,
            );

            // --- Health: detect connected ports whose sink failed; release notes, drop, ask
            //     the watcher to reconnect. Gather unhealthy indices in an immutable pass
            //     first (borrow checker), then mutate. NO enumerate/connect here. ---
            let unhealthy: Vec<usize> = port_sinks
                .iter()
                .enumerate()
                .filter(|(_, ps)| ps.connected && !ps.sink.health())
                .map(|(idx, _)| idx)
                .collect();
            for idx in unhealthy {
                let lanes = lanes_of(&lane_to_port, idx);
                {
                    let mut fanout = PortFanoutSink {
                        ports: &mut port_sinks,
                    };
                    st.seq.release_lanes(&lanes, now, &mut fanout);
                }
                let ps = &mut port_sinks[idx];
                ps.sink = Box::new(NullSink);
                ps.connected = false;
                ps.port_name.clear();
                emit_lane_status(&port_sinks, &lane_to_port, &mut events);
                let _ = request_tx.send(PortRequest::Reconnect(idx));
            }

            // Drain command channel into the pending queue (timestamped at `now`).
            while let Ok(cmd) = cmd_rx.try_recv() {
                pending.push((now, cmd));
            }

            // Forward any events to the UI.
            if flush_events!() || quit {
                // Engine stopping (Quit or UI gone): shut the watcher down; don't leak it.
                let _ = request_tx.send(PortRequest::Quit);
                let _ = watcher.join();
                return;
            }

            // ~1 ms loop; cheap and keeps timing tight enough for 24 PPQN.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    EngineHandle {
        tx: cmd_tx,
        rx: evt_rx,
        join,
    }
}

/// Fans a single `send` out to every distinct-port sink — ONCE per physical port.
struct PortFanoutSink<'a> {
    ports: &'a mut Vec<PortSink>,
}

impl<'a> MidiSink for PortFanoutSink<'a> {
    fn send(&mut self, msg: crate::midi::message::MidiMessage, at_micros: u64) {
        for ps in self.ports.iter_mut() {
            ps.sink.send(msg.clone(), at_micros);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles::default_profiles;
    use crate::link::FakeLink;
    use crate::midi::ports::RecordingSink;
    use crate::pattern::model::Set;

    fn default_set() -> Set {
        Set::default_set(default_profiles())
    }

    /// REGRESSION (no hardware): the two T-8 lanes (drums + bass) share the "T-8" port_match
    /// and MUST collapse to a SINGLE distinct port — one connection, so the 24 PPQN MIDI
    /// clock reaches the device ONCE (two connections doubled the T-8's tempo on real hardware).
    /// S-1 is a distinct port. Mirrors the old `unique_ports` dedup intent.
    #[test]
    fn shared_port_lanes_collapse_to_one_connection() {
        let profiles = default_profiles();
        let (distinct, lane_to_port) = map_lanes_to_ports(&profiles);

        // Default profiles are [T8_DRUMS("T-8"), T8_BASS("T-8"), S1("S-1")].
        // Exactly TWO distinct physical ports, NOT three — the two T-8 lanes dedupe.
        assert_eq!(distinct.len(), 2, "two T-8 lanes must collapse to one port");
        assert_eq!(distinct, vec!["T-8", "S-1"]);

        // Both T-8 lanes map to the SAME port index (one shared connection).
        assert_eq!(
            lane_to_port[0], lane_to_port[1],
            "T-8 drums + bass share one port"
        );
        // S-1 maps to a different port.
        assert_ne!(lane_to_port[2], lane_to_port[0], "S-1 is a distinct port");

        // The number of distinct ports == number of connections the engine will open:
        // one per unique port_match. With two T-8 lanes that is ONE T-8 connection.
        let t8_connections = lane_to_port
            .iter()
            .filter(|&&p| distinct[p] == "T-8")
            .count();
        assert_eq!(t8_connections, 2, "two lanes target T-8");
        let t8_distinct_ports = distinct.iter().filter(|m| **m == "T-8").count();
        assert_eq!(
            t8_distinct_ports, 1,
            "but only ONE T-8 connection is opened"
        );
    }

    /// effective_bpm: when link is disabled, Transport::effective_bpm(None) == manual_bpm.
    /// Verified via the transport layer directly (the headless driver's BPM path).
    #[test]
    fn effective_bpm_uses_manual_when_link_disabled() {
        let set = default_set();
        let bpm = set.bpm;
        // Transport is initialised with Manual source; effective_bpm(None) must return manual_bpm.
        let mut t = Transport::new();
        t.manual_bpm = bpm;
        t.source = TempoSource::Manual(bpm);
        assert_eq!(t.effective_bpm(None), bpm);
        // And effective_bpm ignores a link tempo when source is Manual.
        assert_eq!(t.effective_bpm(Some(140.0)), bpm);
    }

    /// effective_bpm: after ToggleLink(true), transport.source == TempoSource::Link,
    /// so effective_bpm(Some(link_tempo)) returns link_tempo.
    #[test]
    fn effective_bpm_uses_link_tempo_when_source_is_link() {
        let mut t = Transport::new();
        t.manual_bpm = 120.0;
        t.source = TempoSource::Link;
        assert_eq!(t.effective_bpm(Some(140.0)), 140.0);
        // Falls back to manual_bpm when link value absent.
        assert_eq!(t.effective_bpm(None), 120.0);
    }

    /// P2: SetSet must release every sounding note BEFORE replacing the sequencer.
    /// Scenario: play a set with a single long melodic note on step 0, tick far enough that
    /// the NoteOn fires, then send SetSet. The sink must contain a NoteOff (or CC123) for
    /// that note at the swap time — proving release_all ran before the drop.
    #[test]
    fn setset_releases_sounding_notes_before_swap() {
        use crate::midi::MidiMessage;
        use crate::pattern::model::{MelodicNote, Pattern, PatternData};

        // Build a set whose lane 2 (S-1, melodic) has a single long note on step 0.
        let mut set = default_set();
        // 16-step melodic pattern: step 0 has a long (full-bar) note, rest silent.
        let mut steps = vec![None; 16];
        steps[0] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 4.0, // 4 steps long — much longer than our tick window
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[2].pattern = Pattern {
            name: "test".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
        };
        set.bpm = 120.0; // step_dur = 125_000 µs

        let other = default_set(); // second set — content irrelevant

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // Play at t=0, tick past the first step (step fires at t=0), then SetSet at t=10_000.
        // step 0 fires at origin_micros (0), so NoteOn is at t=0. The note len is 4 steps =
        // 500_000 µs — the NoteOff would normally fire much later. SetSet at 10_000 should
        // emit release_all which produces a NoteOff immediately (at t=10_000).
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::Play), (10_000, UiCommand::SetSet(other))],
            50_000,
            1_000,
        );

        // After the SetSet there should be a NoteOff or CC123 (release_all) in the sink.
        let has_release = sink.events.iter().any(|(at, msg)| {
            *at >= 10_000
                && matches!(
                    msg,
                    MidiMessage::NoteOff { .. }
                        | MidiMessage::ControlChange {
                            controller: 123,
                            ..
                        }
                )
        });
        assert!(
            has_release,
            "SetSet must emit NoteOff/CC123 for sounding notes before replacing the sequencer; \
             got events: {:?}",
            sink.events
        );
    }

    /// Bug 2: After SetSet the transport BPM must match the new set's BPM so that MIDI
    /// Clock spacing reflects the NEW set's tempo, not the old one.
    /// Approach: load initial set at 100 BPM, SetSet to 150 BPM while not playing, then
    /// Play. Collect Clock messages and verify consecutive spacing ≈ 16_666 µs (150 BPM),
    /// NOT 25_000 µs (100 BPM).  Window: 200_000 µs at tick 500 µs gives ≥10 clock pulses.
    #[test]
    fn setset_syncs_clock_bpm_to_new_set() {
        use crate::midi::MidiMessage;

        let mut set = default_set();
        set.bpm = 100.0;
        let mut other = default_set();
        other.bpm = 150.0;

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // SetSet at t=1_000 (while stopped), then Play at t=2_000.
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(1_000, UiCommand::SetSet(other)), (2_000, UiCommand::Play)],
            200_000,
            500,
        );

        // Collect Clock timestamps after Play started (t >= 2_000).
        let clock_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(at, msg)| *at >= 2_000 && matches!(msg, MidiMessage::Clock))
            .map(|(at, _)| *at)
            .collect();

        assert!(
            clock_times.len() >= 2,
            "need at least 2 Clock messages to measure spacing; got {:?}",
            clock_times
        );

        // Expected spacing at 150 BPM = 60_000_000 / (150 * 24) = 16_666 µs.
        // Allow ±one tick (500 µs) of quantization error.
        let expected = 60_000_000u64 / (150 * 24);
        let old_expected = 60_000_000u64 / (100 * 24);

        // Check first consecutive pair.
        let spacing = clock_times[1] - clock_times[0];
        assert!(
            spacing.abs_diff(expected) <= 1_000,
            "Clock spacing should be ≈{expected} µs (150 BPM) but got {spacing} µs \
             (old 100 BPM spacing would be {old_expected} µs)"
        );
    }

    /// Headless engine emits LinkStatus{enabled:false} before ToggleLink.
    #[test]
    fn headless_emits_link_status_disabled_initially() {
        let set = default_set();
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let events = run_engine_headless(set, &mut link, &mut sink, vec![], 1_000, 100);
        let found = events
            .iter()
            .any(|ev| matches!(ev, EngineEvent::LinkStatus { enabled: false, .. }));
        assert!(found, "expected at least one LinkStatus{{enabled:false}}");
    }

    /// Headless engine emits LinkStatus{enabled:true} after ToggleLink(true).
    #[test]
    fn headless_emits_link_status_enabled_after_toggle() {
        let set = default_set();
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let events = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::ToggleLink(true))],
            1_000,
            100,
        );
        let enabled_ev = events
            .iter()
            .any(|ev| matches!(ev, EngineEvent::LinkStatus { enabled: true, .. }));
        assert!(
            enabled_ev,
            "expected LinkStatus{{enabled:true}} after ToggleLink"
        );
    }

    /// Bug 1b: with Link enabled and beat < 0 (pre-boundary countdown), Play must NOT
    /// start the sequencer immediately — no NoteOn and no Started event.
    #[test]
    fn link_play_defers_notes_until_boundary() {
        use crate::midi::MidiMessage;

        let set = default_set();
        let mut link = FakeLink::new();
        link.set_enabled(true);
        link.set_beat(-1.0); // still counting down; boundary not yet reached

        let mut sink = RecordingSink::new();
        let evs = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::ToggleLink(true)), (0, UiCommand::Play)],
            5_000,
            1_000,
        );

        assert!(
            !sink
                .events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { .. })),
            "no NoteOn should fire before the Link bar boundary; got: {:?}",
            sink.events
        );
        assert!(
            !evs.iter().any(|e| matches!(e, EngineEvent::Started { .. })),
            "no Started event should be emitted before boundary; got: {:?}",
            evs
        );
    }

    /// Regression: without Link, Play must start the sequencer immediately and emit Started.
    #[test]
    fn manual_play_starts_immediately() {
        use crate::midi::MidiMessage;
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0; // step_dur = 125_000 µs

        // Add a drum hit on lane 0 step 0 so a NoteOn fires immediately.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[0].pattern = Pattern {
            name: "test".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
        };

        let mut link = FakeLink::new(); // link disabled
        let mut sink = RecordingSink::new();
        let evs = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::Play)],
            500,
            100,
        );

        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { .. })),
            "NoteOn must fire at t=0 in manual (no-Link) mode; got: {:?}",
            sink.events
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, EngineEvent::Started { at_step: 0 })),
            "Started{{at_step:0}} must be emitted immediately in manual mode; got: {:?}",
            evs
        );
    }

    /// Task 5 pure planner: present&&!connected → Connect; !present&&connected → Drop; else nothing.
    #[test]
    fn plan_connects_present_unconnected_and_drops_absent_connected() {
        assert_eq!(
            plan_port_actions(&[true, false], &[false, false]),
            vec![PortAction::Connect(0)]
        );
        assert_eq!(
            plan_port_actions(&[false, true], &[true, true]),
            vec![PortAction::Drop(0)]
        );
        assert!(plan_port_actions(&[true, true], &[true, true]).is_empty());
    }

    /// Task 5: lanes_of groups every lane that shares a port. Default profiles =
    /// [T-8 drums, T-8 bass, S-1]: the T-8 port owns lanes [0,1]; the S-1 port owns [2].
    #[test]
    fn lanes_of_port_groups_shared_port_lanes() {
        let profiles = default_profiles();
        let (distinct, lane_to_port) = map_lanes_to_ports(&profiles);
        let t8_idx = distinct.iter().position(|m| *m == "T-8").unwrap();
        let s1_idx = distinct.iter().position(|m| *m == "S-1").unwrap();
        assert_eq!(lanes_of(&lane_to_port, t8_idx), vec![0, 1]);
        assert_eq!(lanes_of(&lane_to_port, s1_idx), vec![2]);
    }

    /// Link-gated start fires at the boundary: armed at beat=-1, boundary crossed at beat=0.
    /// Tests step_engine directly since FakeLink's beat is static across run_engine_headless.
    #[test]
    fn link_play_starts_at_boundary() {
        let set = default_set();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        link.set_enabled(true);
        link.set_beat(-1.0); // pre-boundary

        let mut sink = crate::midi::ports::RecordingSink::new();
        let mut events: Vec<EngineEvent> = Vec::new();

        // Arm the engine: send Play command through step_engine
        let mut pending = vec![(0u64, UiCommand::ToggleLink(true)), (0u64, UiCommand::Play)];
        step_engine(&mut st, 0, &mut pending, &mut link, &mut sink, &mut events);

        // After arming: no Started yet, sequencer not playing
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EngineEvent::Started { .. })),
            "no Started before boundary; got: {:?}",
            events
        );
        assert!(!st.seq.is_playing(), "seq must NOT be playing while armed");
        assert!(st.armed, "engine must be armed");

        // Now cross the boundary
        link.set_beat(0.0);
        events.clear();
        let mut pending2: Vec<(u64, UiCommand)> = vec![];
        step_engine(
            &mut st,
            1_000,
            &mut pending2,
            &mut link,
            &mut sink,
            &mut events,
        );

        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::Started { at_step: 0 })),
            "Started{{at_step:0}} must be emitted when beat >= 0; got: {:?}",
            events
        );
        assert!(
            st.seq.is_playing(),
            "sequencer must be playing after boundary"
        );
        assert!(!st.armed, "armed must be cleared after boundary");
    }
}
