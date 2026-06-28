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

/// Check for hot-plug / send-failure every this many ticks (~1 s at 1 ms/tick).
const HOTPLUG_CHECK_EVERY: u64 = 1_000;

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

/// One connection per distinct physical port. `port_match` is the substring that
/// identifies the port; `sink` is the single live connection (or `NullSink` when
/// disconnected). Health + hot-plug operate at this PORT level.
struct PortSink {
    /// The port_match substring shared by every lane mapped to this port.
    port_match: &'static str,
    /// The single sink for this port (NullSink when not connected).
    sink: Box<dyn MidiSink>,
    /// Last-known connection state (for change-detection / dedupe).
    connected: bool,
    /// Last-known connected port name (empty when disconnected).
    port_name: String,
}

/// Distinct-port plan derived purely from profiles — NO hardware access.
///
/// Returns `(ports, lane_to_port)` where `ports[i].port_match` is the i-th distinct
/// `port_match` in first-seen order, and `lane_to_port[lane]` is the index into `ports`
/// for that lane. Lanes sharing a `port_match` map to the SAME port index — this is the
/// dedup that guarantees one connection per physical port. Mirrors the old `unique_ports`.
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

/// Build ONE sink per distinct physical port and emit one initial `DeviceStatus` per lane
/// (derived from the shared port state). Opens at most one connection per port_match.
fn build_port_sinks(
    profiles: &[DeviceProfile; 3],
    events: &mut Vec<EngineEvent>,
) -> (Vec<PortSink>, [usize; 3]) {
    let (distinct, lane_to_port) = map_lanes_to_ports(profiles);
    let available = list_output_ports();
    let mut ports: Vec<PortSink> = Vec::with_capacity(distinct.len());

    for port_match in distinct {
        let mut ps = PortSink {
            port_match,
            sink: Box::new(NullSink),
            connected: false,
            port_name: String::new(),
        };
        // Connect exactly once if the port is present.
        if match_port(&available, port_match).is_some() {
            if let Ok(midir_sink) = connect(port_match) {
                ps.sink = Box::new(midir_sink);
                ps.connected = true;
                ps.port_name = port_display_name(&available, port_match);
            }
        }
        ports.push(ps);
    }

    // Per-lane DeviceStatus derived from the (single) port state.
    emit_lane_status(&ports, &lane_to_port, events);
    (ports, lane_to_port)
}

/// Rescan health + port presence at the PORT level. Emits per-lane `DeviceStatus` only when
/// a port's state CHANGES (so both lanes on a shared port flip together, consistently).
///
/// Runs in the engine thread every ~1 s (HOTPLUG_CHECK_EVERY ticks). Handles:
/// - Send-failure: `MidiSink::health()` flips false after a failed write → port → NullSink.
/// - Device vanished: port no longer in `list_output_ports()` → port → NullSink.
/// - Device reappeared: port back in list for a disconnected port → reconnect ONCE.
///
/// Not unit-tested (touches real MIDI hardware); logic kept simple and well-commented.
fn rescan_port_sinks(
    ports: &mut [PortSink],
    lane_to_port: &[usize; 3],
    events: &mut Vec<EngineEvent>,
) {
    let available = list_output_ports();
    let mut changed = false;

    for ps in ports.iter_mut() {
        let port_present = match_port(&available, ps.port_match).is_some();
        let sink_healthy = ps.sink.health();

        if ps.connected {
            // Vanished or unhealthy → drop the single connection.
            if !port_present || !sink_healthy {
                ps.sink = Box::new(NullSink);
                ps.connected = false;
                ps.port_name.clear();
                changed = true;
            }
        } else if port_present {
            // Reappeared → reconnect exactly one connection for this port.
            if let Ok(midir_sink) = connect(ps.port_match) {
                ps.sink = Box::new(midir_sink);
                ps.connected = true;
                ps.port_name = port_display_name(&available, ps.port_match);
                changed = true;
            }
        }
    }

    // Re-derive per-lane status only when something changed (dedupe).
    if changed {
        emit_lane_status(ports, lane_to_port, events);
    }
}

/// Spawn the real engine on its own thread, driven by a monotonic clock. NOT unit-tested
/// (non-deterministic timing); shares `step_engine` with the headless driver.
///
/// Takes `profiles` instead of pre-built sinks so the engine thread owns the full
/// sink lifecycle (initial connect + periodic hot-plug rescan). Builds ONE connection
/// per distinct physical port (shared-port lanes collapse — no double-clock). Initial
/// `DeviceStatus` events are emitted per lane (derived from port state) before the first
/// tick so the UI receives them via `on_engine_event`.
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

        // Build ONE sink per distinct port; emit initial per-lane DeviceStatus.
        let (mut port_sinks, lane_to_port) = build_port_sinks(&profiles, &mut events);

        // Flush initial DeviceStatus events before the first step.
        for ev in events.drain(..) {
            if evt_tx.send(ev).is_err() {
                return;
            }
        }

        loop {
            let now = start.elapsed().as_micros() as u64;

            // Drain channel into the pending queue (timestamped at `now`).
            while let Ok(cmd) = cmd_rx.try_recv() {
                pending.push((now, cmd));
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

            // Forward any events to the UI; drop on disconnect.
            for ev in events.drain(..) {
                if evt_tx.send(ev).is_err() {
                    return;
                }
            }
            if quit {
                return;
            }

            // Periodic hot-plug rescan: check health + port presence every ~1 s.
            if st.tick_count.is_multiple_of(HOTPLUG_CHECK_EVERY) && st.tick_count > 0 {
                rescan_port_sinks(&mut port_sinks, &lane_to_port, &mut events);
                for ev in events.drain(..) {
                    if evt_tx.send(ev).is_err() {
                        return;
                    }
                }
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
