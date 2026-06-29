//! Engine thread entry point and the deterministic headless driver.
//!
//! The real engine (`spawn_engine`) runs `step_engine` in a loop on a monotonic clock;
//! the test driver (`run_engine_headless`) runs the *same* `step_engine` over a virtual
//! clock. Only the headless driver is unit-tested — the threaded one is not deterministic.

pub mod clock;
pub mod scheduler;
pub mod transport;

use crate::link::LinkClock;
use crate::midi::ports::{connect, list_output_ports, match_port, MidiSink, NullSink};
#[cfg(test)]
use crate::pattern::model::TrigCond;
use crate::pattern::model::{Lane, LaneRoute, Pattern, Set};
use clock::ClockGen;
use scheduler::{LaunchState, Quant, Sequencer};
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
    /// M3: queue `pattern` on `lane` to launch at the next `quant` boundary (clip-launcher
    /// style) instead of swapping mid-phrase. Used while PLAYING; `LoadPattern` stays for the
    /// immediate (stopped) load + undo resync path.
    QueuePattern {
        lane: usize,
        pattern: Pattern,
        quant: Quant,
    },
    /// M3: cancel a pending queued launch on `lane`.
    CancelQueue {
        lane: usize,
    },
    /// M6: queue an all-lane scene recall as ONE coordinated quantized launch. Every entry
    /// `(lane, pattern, state)` is queued at the SAME `quant` boundary, so the sequencer
    /// fires them all together on one bar/beat boundary (restarting each lane at step 1 and
    /// applying its mute/solo/transpose/octave at that instant). Lanes absent from `lanes`
    /// (e.g. a missing pattern) are left untouched. `CancelQueue` per lane cancels them.
    QueueScene {
        quant: Quant,
        lanes: Vec<(usize, Pattern, LaunchState)>,
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
    /// Change a single lane's MIDI route at runtime (`None` = derive from profile).
    /// Releases the lane's sounding notes first, updates the route, then asks the real
    /// engine loop (via `route_dirty`) to re-plan ports / re-spawn the watcher if the
    /// distinct destination port set changed.
    SetRoute {
        lane: usize,
        route: Option<LaneRoute>,
    },
    /// All-notes-off / all-sound-off live recovery; does not stop transport.
    Panic,
    /// Enable or disable the virtual-port mirror (T2: toggled by the UI toggle).
    /// Safe to receive in headless mode — just sets the flag; no port is opened.
    SetMirror(bool),
    /// Per-drum-voice mute (§2.6): silence a single MIDI note on a drum lane, latched.
    /// When `on=true`, the note is added to `lane.muted_voices` and any sounding instance
    /// is immediately released. When `on=false`, the note is removed (unmuted silently).
    MuteVoice {
        lane: usize,
        note: u8,
        on: bool,
    },
    /// Latch fill-active performance state. Read by Fill/NotFill trig conditions
    /// in the scheduler. Separate from ToggleFill (a pattern transform).
    SetFillActive(bool),
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
    /// Engine-resolved tempo after a Tap or SetBpm command. Carries the BPM that
    /// the engine actually applied so the UI can update its displayed value.
    Tempo { bpm: f64 },
    /// M3: engine-confirmed that a queued per-lane launch fired at global `step`
    /// (so the UI can flip ACTIVE↔QUEUED on confirmation).
    Launched { lane: usize, step: usize },
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
    /// Set by `SetRoute` to ask the real `spawn_engine` loop to recompute the route
    /// plan (channel→port / clock / lane→port) and, if the distinct port set changed,
    /// re-spawn the device watcher. `apply_command` cannot touch the loop's
    /// port_sinks/watcher, so it flags here and the loop reacts. Ignored headless.
    route_dirty: bool,
    /// Whether the virtual "midip" output mirror is active. Default off.
    /// When on, the fan-out ALSO delivers every message (notes/CC + Clock) to the virtual
    /// port IN ADDITION TO the hardware fanout — purely additive; hardware path is
    /// byte-identical either way (a lane explicitly routed to "midip" is not double-sent).
    mirror_on: bool,
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
            route_dirty: false,
            mirror_on: false,
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
            events.push(EngineEvent::Tempo {
                bpm: st.transport.manual_bpm,
            });
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
        UiCommand::QueuePattern {
            lane,
            pattern,
            quant,
        } => {
            // M3: arm a per-lane launch; the sequencer applies it at the next boundary.
            st.seq.queue_launch(lane, pattern, quant);
        }
        UiCommand::CancelQueue { lane } => {
            st.seq.cancel_launch(lane);
        }
        UiCommand::QueueScene { quant, lanes } => {
            // M6: queue every scene lane with the SAME quant so the sequencer's
            // boundary check (`is_boundary(step, quant)`) fires them all on ONE step.
            for (lane, pattern, state) in lanes {
                st.seq.queue_launch_with_state(lane, pattern, quant, state);
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
            // A new set may carry different lane routes (port/channel). Flag the loop to
            // re-plan ports and re-spawn the watcher so notes route to the correct device.
            // Without this, SetBrowserLoad / RecoveryRecover would leave the old route plan
            // active until restart.
            st.route_dirty = true;
        }
        UiCommand::SetRoute { lane, route } => {
            // Release the lane's sounding notes BEFORE the route changes, so the
            // NoteOffs go out on the OLD channel (route_channel still reflects it).
            st.seq.release_lanes(&[lane], now, sink);
            st.seq.set_lane_route(lane, route);
            // The real loop owns port_sinks + the watcher; flag it to re-plan. The
            // headless driver ignores this (no ports).
            st.route_dirty = true;
        }
        UiCommand::Panic => {
            // All-notes-off / all-sound-off on every lane channel. Does NOT touch the
            // transport or clock — playback keeps running while stuck notes are cleared.
            st.seq.panic(now, sink);
        }
        UiCommand::SetMirror(on) => {
            st.mirror_on = on;
        }
        UiCommand::MuteVoice { lane, note, on } => {
            st.seq.set_voice_mute(lane, note, on, now, sink);
        }
        UiCommand::SetFillActive(on) => {
            st.seq.set_fill_active(on);
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
    // M3: emit a Launched event for each lane whose queued launch fired this tick.
    // The boundary step is the sequencer's current absolute step (launches apply at the
    // step being materialized, which is the latest `current`).
    for lane in st.seq.take_launched() {
        events.push(EngineEvent::Launched {
            lane,
            step: st.seq.current_step(),
        });
    }
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

/// Route-targeted output plan, derived purely from the lanes' `effective_route()` — NO
/// hardware access. Replaces the old profile-only `map_lanes_to_ports`/broadcast model.
///
/// - `ports[i]` is the i-th distinct destination port (deduped by `PortRef.stable_key`,
///   falling back to `name`), in first-seen lane order.
/// - `channel_to_port[ch]` is the port index that MIDI channel `ch` (0..=15) routes to,
///   or `None` if no lane uses that channel.
/// - `clock_ports` lists the distinct port indices whose route has `clock_out` — MIDI
///   Clock is sent once to each (deduped: a shared port appears once even with two lanes).
/// - `lane_to_port[lane]` is the port index a lane delivers to (lanes sharing a port
///   collapse to the SAME index — one connection per physical port, no double-clock).
struct RoutePlan {
    ports: Vec<crate::pattern::model::PortRef>,
    channel_to_port: [Option<usize>; 16],
    clock_ports: Vec<usize>,
    lane_to_port: Vec<usize>,
}

/// The dedup key for a `PortRef`: prefer `stable_key`, fall back to `name` when empty.
fn port_key(port: &crate::pattern::model::PortRef) -> &str {
    if port.stable_key.is_empty() {
        &port.name
    } else {
        &port.stable_key
    }
}

/// Build the route plan from each lane's `effective_route()`. Pure; UNIT-TESTED.
///
/// Dedups ports by `port_key`; maps `channel_to_port[route.channel] = port_idx`; adds the
/// port to `clock_ports` (deduped) when `route.clock_out`; records `lane_to_port[lane]`.
fn build_route_plan(lanes: &[Lane]) -> RoutePlan {
    let mut ports: Vec<crate::pattern::model::PortRef> = Vec::new();
    let mut channel_to_port = [None; 16];
    let mut clock_ports: Vec<usize> = Vec::new();
    let mut lane_to_port: Vec<usize> = Vec::with_capacity(lanes.len());

    for lane in lanes {
        let route = lane.effective_route();
        let key = port_key(&route.port).to_string();
        let port_idx = match ports.iter().position(|p| port_key(p) == key) {
            Some(i) => i,
            None => {
                ports.push(route.port.clone());
                ports.len() - 1
            }
        };
        lane_to_port.push(port_idx);
        if let Some(slot) = channel_to_port.get_mut(route.channel as usize) {
            *slot = Some(port_idx);
        }
        if route.clock_out && !clock_ports.contains(&port_idx) {
            clock_ports.push(port_idx);
        }
    }

    RoutePlan {
        ports,
        channel_to_port,
        clock_ports,
        lane_to_port,
    }
}

/// Build the route plan AND guarantee the engine-managed virtual "midip" port is present
/// in `ports` (so it is always a valid routable target, even when no lane uses it). Pure;
/// UNIT-TESTED. Used by the real engine; `build_route_plan` stays virtual-free for the
/// hardware-only dedup tests.
///
/// A lane whose `effective_route().port.is_virtual()` is recognised by `build_route_plan`
/// like any other key (dedup by `VIRTUAL_PORT_KEY`), so its channel/clock map to the same
/// index this function then ensures exists. If no lane routed to it, the virtual port is
/// appended at the end with no channel/clock mappings (mirror still uses it on demand).
fn build_route_plan_with_virtual(lanes: &[Lane]) -> RoutePlan {
    let mut plan = build_route_plan(lanes);
    if virtual_port_index(&plan).is_none() {
        plan.ports
            .push(crate::pattern::model::PortRef::virtual_midip());
    }
    plan
}

/// Index of the virtual "midip" port in a plan's `ports`, if present. Pure; UNIT-TESTED.
fn virtual_port_index(plan: &RoutePlan) -> Option<usize> {
    plan.ports.iter().position(|p| p.is_virtual())
}

/// Port indices a single message targets (route-targeted, NOT broadcast). Pure; UNIT-TESTED.
///
/// - `NoteOn`/`NoteOff`/`ControlChange{channel}` → `channel_to_port[channel]` as a 0-or-1
///   element vec (empty when the channel is unmapped → the message drops silently).
/// - `Clock` → every clock-out port.
/// - Any other variant (Start/Stop/Continue) → empty.
fn route_targets(
    msg: &crate::midi::message::MidiMessage,
    channel_to_port: &[Option<usize>; 16],
    clock_ports: &[usize],
) -> Vec<usize> {
    use crate::midi::message::MidiMessage;
    match msg {
        MidiMessage::NoteOn { channel, .. }
        | MidiMessage::NoteOff { channel, .. }
        | MidiMessage::ControlChange { channel, .. } => channel_to_port
            .get(*channel as usize)
            .copied()
            .flatten()
            .into_iter()
            .collect(),
        MidiMessage::Clock => clock_ports.to_vec(),
        _ => Vec::new(),
    }
}

/// Port indices a message targets, with the virtual-"midip" mirror FOLDED IN. Pure; UNIT-TESTED.
///
/// Starts from `route_targets` (route-driven hardware + any lane routed to the virtual port).
/// When `mirror_on` and a `virtual_idx` exists, the virtual port is ADDED for the FULL stream
/// — every channel message (routed or not) and every Clock — replicating the old additive
/// `TeeSink` mirror. Deduped: if a lane already routes the message to the virtual port (so it
/// is in the base targets), the mirror does NOT add a second copy. Non-virtual targets are
/// untouched, so the hardware path is byte-identical whether the mirror is on or off.
fn route_targets_with_mirror(
    msg: &crate::midi::message::MidiMessage,
    channel_to_port: &[Option<usize>; 16],
    clock_ports: &[usize],
    virtual_idx: Option<usize>,
    mirror_on: bool,
) -> Vec<usize> {
    use crate::midi::message::MidiMessage;
    let mut targets = route_targets(msg, channel_to_port, clock_ports);
    if mirror_on {
        if let Some(vidx) = virtual_idx {
            // Mirror only the message types the mirror has always carried: channel
            // messages + Clock (the full performance stream). Other realtime (Start/Stop/
            // Continue) are not emitted by the engine, matching the old TeeSink behavior of
            // forwarding whatever `send` received — but those variants never reach `send`.
            let mirrorable = matches!(
                msg,
                MidiMessage::NoteOn { .. }
                    | MidiMessage::NoteOff { .. }
                    | MidiMessage::ControlChange { .. }
                    | MidiMessage::Clock
            );
            // Dedup: only add the virtual port if it is not already a target (e.g. a lane
            // routed here, or the virtual port is in clock_ports).
            if mirrorable && !targets.contains(&vidx) {
                targets.push(vidx);
            }
        }
    }
    targets
}

/// All lanes that resolve to the given port index, in lane order. Used to release the
/// correct notes when a port connects/disconnects/fails (lanes sharing a port move
/// together — the registry tracks per-lane ownership). Pure; UNIT-TESTED.
fn lanes_of(lane_to_port: &[usize], port_idx: usize) -> Vec<usize> {
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
    port_matches: Vec<String>,
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
        // The engine-managed virtual "midip" port is NEVER enumerated/connected/dropped here:
        // it is not a system destination (it never appears in `list_output_ports()`), it is
        // always present if the engine created it, and the engine installs its sink directly.
        // Force its presence to `false` so `plan_port_actions` emits no Connect/Drop for it
        // and `connect()` is never called for the virtual key. Index parity with the engine's
        // `port_sinks` is preserved (the slot still exists, the watcher just skips it).
        let present: Vec<bool> = port_matches
            .iter()
            .map(|m| {
                if m == crate::pattern::model::VIRTUAL_PORT_KEY {
                    false
                } else {
                    match_port(&available, m).is_some()
                }
            })
            .collect();

        for action in plan_port_actions(&present, &connected) {
            match action {
                PortAction::Connect(idx) => {
                    if let Ok(sink) = connect(&port_matches[idx]) {
                        let name = port_display_name(&available, &port_matches[idx]);
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
fn emit_lane_status(ports: &[PortSink], lane_to_port: &[usize], events: &mut Vec<EngineEvent>) {
    for (lane, &port_idx) in lane_to_port.iter().enumerate() {
        let ps = &ports[port_idx];
        events.push(EngineEvent::DeviceStatus {
            lane,
            connected: ps.connected,
            port: ps.port_name.clone(),
        });
    }
}

/// Spawn a fresh device-watcher thread for the given distinct port keys, returning its
/// join handle plus the channels the engine talks to it over. The watcher OWNS all port
/// enumeration/connection; the engine only installs the ready-made sinks it streams back.
/// Used at startup and again whenever a `SetRoute` changes the distinct port set (the old
/// watcher is told to `Quit` and joined, a new one spawned for the new keys).
fn spawn_watcher(
    keys: Vec<String>,
) -> (
    std::thread::JoinHandle<()>,
    crossbeam_channel::Receiver<PortUpdate>,
    crossbeam_channel::Sender<PortRequest>,
) {
    let (update_tx, update_rx) = crossbeam_channel::unbounded::<PortUpdate>();
    let (request_tx, request_rx) = crossbeam_channel::unbounded::<PortRequest>();
    let handle = std::thread::spawn(move || run_port_watcher(keys, update_tx, request_rx));
    (handle, update_rx, request_tx)
}

/// Spawn the real engine on its own thread, driven by a monotonic clock. NOT unit-tested
/// (non-deterministic timing); shares `step_engine` with the headless driver.
///
/// All `PortSink`s start as `NullSink`/disconnected — a dedicated device-watcher thread
/// (the ONLY caller of `list_output_ports()`/`connect()`) owns the entire connection
/// lifecycle and hands the engine ready-made sinks over a channel. The timing loop never
/// enumerates or connects: it only installs delivered sinks, releases notes on
/// loss/route-change (P1), and detects health failures (asking the watcher to reconnect).
/// One connection per distinct destination port (shared-port lanes collapse — no
/// double-clock). The destination ports come from the set's per-lane routes
/// (`Lane::effective_route`), so no device-profile array is needed — the plan is
/// derived entirely from `set.lanes`.
pub fn spawn_engine(set: Set, mut link: Box<dyn LinkClock>) -> EngineHandle {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<UiCommand>();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded::<EngineEvent>();

    let join = std::thread::spawn(move || {
        // Route plan from the initial set's lanes (route-driven, NOT profile-only). Built
        // before `set` is moved into EngineState::new. The plan is RE-derived at runtime
        // when a `SetRoute` flags `route_dirty` (see the re-plan block in the loop), so
        // these are `mut`. The watcher connects each distinct port by its key/name — for
        // profile-derived routes the key == the old `port_match`, so default behavior is
        // unchanged.
        // Route plan ALWAYS includes the engine-managed virtual "midip" port (so a lane can
        // target it and the mirror has a destination), even when no lane currently routes to
        // it. The virtual port lives in `port_sinks` like any other port, but the watcher
        // SKIPS it (it is never enumerated/connected — see `run_port_watcher`).
        let plan = build_route_plan_with_virtual(&set.lanes);
        // Index of the virtual port in `port_sinks`. Stable across re-plans because the
        // virtual key is always present in both the old and new key sets (carried over).
        // Computed before the plan's fields are moved out below.
        let mut virtual_idx = virtual_port_index(&plan);
        // Distinct port keys currently realized in `port_sinks` (parallel by index). The
        // re-plan compares the new key set against this to decide whether to re-spawn.
        let mut port_keys: Vec<String> =
            plan.ports.iter().map(|p| port_key(p).to_string()).collect();
        let mut channel_to_port = plan.channel_to_port;
        let mut clock_ports = plan.clock_ports;
        let mut lane_to_port = plan.lane_to_port;

        let mut st = EngineState::new(set);
        let mut pending: Vec<(u64, UiCommand)> = Vec::new();
        let mut events: Vec<EngineEvent> = Vec::new();
        let start = std::time::Instant::now();

        // Create the virtual "midip" output ONCE before the loop — this opens a CoreMIDI
        // virtual source that other apps on the machine can subscribe to. No-op on non-Unix
        // or if CoreMIDI is unavailable; the engine runs without it in both cases.
        // The virtual sink is installed DIRECTLY into `port_sinks[virtual_idx]` (a first-class
        // managed port). It is never connected by the watcher; `connected` reflects whether
        // the real virtual output was created, so a lane routed to "midip" shows CON ●.
        let virtual_created = crate::midi::ports::create_virtual_output("midip");

        // Every port starts disconnected (NullSink); the watcher will connect present hardware
        // ports. The virtual slot is overwritten below with the real virtual output (if any).
        let mut port_sinks: Vec<PortSink> = port_keys
            .iter()
            .map(|_| PortSink {
                sink: Box::new(NullSink),
                connected: false,
                port_name: String::new(),
            })
            .collect();
        if let Some(vidx) = virtual_idx {
            if let Some(sink) = virtual_created {
                port_sinks[vidx] = PortSink {
                    sink: Box::new(sink),
                    connected: true,
                    port_name: crate::pattern::model::VIRTUAL_PORT_NAME.to_string(),
                };
            }
            // If creation failed, the slot stays NullSink/disconnected (CON ○ for midip).
        }

        // Spawn the device-watcher: it owns enumeration/connection and streams PortUpdates.
        // Each distinct port is matched by its stable_key (falling back to name). The virtual
        // key is passed for index parity but the watcher skips it. Re-spawned on a SetRoute
        // that changes the distinct port set, so these handles are `mut`.
        let (watcher, mut update_rx, mut request_tx) = spawn_watcher(port_keys.clone());
        let mut watcher = Some(watcher);

        // Emit initial (all-disconnected) per-lane DeviceStatus before the first tick;
        // the watcher's first scan (immediate, no initial sleep) flips present ports shortly.
        emit_lane_status(&port_sinks, &lane_to_port, &mut events);
        for ev in events.drain(..) {
            if evt_tx.send(ev).is_err() {
                let _ = request_tx.send(PortRequest::Quit);
                if let Some(w) = watcher.take() {
                    let _ = w.join();
                }
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
                                channel_to_port: &channel_to_port,
                                clock_ports: &clock_ports,
                                virtual_idx,
                                mirror_on: false,
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
                        // `release_lanes` emits NoteOffs carrying each lane's channel, so the
                        // route-targeted fanout delivers them to exactly the right port (T6 —
                        // no longer broadcast to every port).
                        let lanes = lanes_of(&lane_to_port, idx);
                        let mut fanout = PortFanoutSink {
                            ports: &mut port_sinks,
                            channel_to_port: &channel_to_port,
                            clock_ports: &clock_ports,
                            virtual_idx,
                            mirror_on: false,
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

            // Route-targeted fan-out: each channel message reaches ONLY its mapped port;
            // Clock reaches each clock-out port ONCE (a shared port appears once — no
            // double-clock). NO enumerate/connect here — the watcher owns connections.
            //
            // The mirror is FOLDED into the fan-out: when `mirror_on`, the full stream is
            // ALSO delivered to the virtual port index (deduped against any lane already
            // routed there). The hardware path is byte-identical whether the mirror is on or
            // off. `fanout` is scoped to the inner block so its borrow of `port_sinks` is
            // dropped before the health-check / route-replan below re-borrows it.
            let quit = {
                let mut fanout = PortFanoutSink {
                    ports: &mut port_sinks,
                    channel_to_port: &channel_to_port,
                    clock_ports: &clock_ports,
                    virtual_idx,
                    mirror_on: st.mirror_on,
                };
                step_engine(
                    &mut st,
                    now,
                    &mut pending,
                    link.as_mut(),
                    &mut fanout,
                    &mut events,
                )
            };

            // --- Health: detect connected ports whose sink failed; release notes, drop, ask
            //     the watcher to reconnect. Gather unhealthy indices in an immutable pass
            //     first (borrow checker), then mutate. NO enumerate/connect here. The virtual
            //     port is SKIPPED — it is engine-managed (never watcher-reconnected); dropping
            //     it and asking the watcher to rebuild it would lose it permanently. ---
            let unhealthy: Vec<usize> = port_sinks
                .iter()
                .enumerate()
                .filter(|(idx, ps)| Some(*idx) != virtual_idx && ps.connected && !ps.sink.health())
                .map(|(idx, _)| idx)
                .collect();
            for idx in unhealthy {
                let lanes = lanes_of(&lane_to_port, idx);
                {
                    let mut fanout = PortFanoutSink {
                        ports: &mut port_sinks,
                        channel_to_port: &channel_to_port,
                        clock_ports: &clock_ports,
                        virtual_idx,
                        mirror_on: false,
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
                if let Some(w) = watcher.take() {
                    let _ = w.join();
                }
                return;
            }

            // --- Dynamic re-plan: a `SetRoute` set `route_dirty`. Recompute the route plan
            //     from the current lanes. If the DISTINCT port KEY set changed, release all
            //     sounding notes (the channel→port map is about to change underneath them),
            //     then re-spawn the watcher for the new keys and rebuild `port_sinks` —
            //     carrying over still-present connections by key, NullSink for new keys.
            //     NO enumerate/connect here; the watcher owns that. ---
            if st.route_dirty {
                st.route_dirty = false;
                // Re-plan ALWAYS includes the virtual port (same as startup) so it stays a
                // valid target after a route change. Its slot is carried over by key below,
                // preserving the live virtual sink.
                let new_plan = build_route_plan_with_virtual(st.seq.lanes());
                let new_keys: Vec<String> = new_plan
                    .ports
                    .iter()
                    .map(|p| port_key(p).to_string())
                    .collect();
                virtual_idx = virtual_port_index(&new_plan);

                // Always adopt the new channel/clock/lane maps (a route change can move a
                // channel between existing ports without changing the key SET).
                channel_to_port = new_plan.channel_to_port;
                clock_ports = new_plan.clock_ports;
                lane_to_port = new_plan.lane_to_port;

                if new_keys != port_keys {
                    // Release every sounding note before the topology shifts (the old
                    // channel→port map still routes the NoteOffs to the live connections).
                    // Mirror is OFF for cleanup, so `virtual_idx` is unused here — pass None.
                    {
                        let mut fanout = PortFanoutSink {
                            ports: &mut port_sinks,
                            channel_to_port: &channel_to_port,
                            clock_ports: &clock_ports,
                            virtual_idx: None,
                            mirror_on: false,
                        };
                        st.seq.release_all(now, &mut fanout);
                    }

                    // Carry over still-present connections by key; NullSink for new keys.
                    let mut new_sinks: Vec<PortSink> = Vec::with_capacity(new_keys.len());
                    for key in &new_keys {
                        if let Some(old_idx) = port_keys.iter().position(|k| k == key) {
                            new_sinks.push(std::mem::replace(
                                &mut port_sinks[old_idx],
                                PortSink {
                                    sink: Box::new(NullSink),
                                    connected: false,
                                    port_name: String::new(),
                                },
                            ));
                        } else {
                            new_sinks.push(PortSink {
                                sink: Box::new(NullSink),
                                connected: false,
                                port_name: String::new(),
                            });
                        }
                    }
                    port_sinks = new_sinks;
                    port_keys = new_keys;

                    // Re-spawn the watcher with the new key set: signal old → Quit + join,
                    // then spawn fresh. (NO enumerate/connect on this thread.)
                    let _ = request_tx.send(PortRequest::Quit);
                    if let Some(w) = watcher.take() {
                        let _ = w.join();
                    }
                    let (w, urx, rtx) = spawn_watcher(port_keys.clone());
                    watcher = Some(w);
                    update_rx = urx;
                    request_tx = rtx;

                    emit_lane_status(&port_sinks, &lane_to_port, &mut events);
                    if flush_events!() {
                        let _ = request_tx.send(PortRequest::Quit);
                        if let Some(w) = watcher.take() {
                            let _ = w.join();
                        }
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

/// Route-targeted fan-out: delivers each `send` ONLY to the port(s) its targets resolve to
/// — a channel message goes to that channel's single mapped port; MIDI Clock goes once to
/// each clock-out port. Unmapped channels drop silently. One delivery per physical port
/// preserves no-double-clock.
///
/// The virtual-"midip" MIRROR is folded in here (replacing the old additive `TeeSink`):
/// when `mirror_on` and a `virtual_idx` exists, the FULL stream is ALSO delivered to the
/// virtual port — deduped so a lane already routed to "midip" is not sent twice. Non-virtual
/// (hardware) targets are byte-identical whether the mirror is on or off.
struct PortFanoutSink<'a> {
    ports: &'a mut Vec<PortSink>,
    channel_to_port: &'a [Option<usize>; 16],
    clock_ports: &'a [usize],
    /// Index of the engine-managed virtual "midip" port in `ports`, if present.
    virtual_idx: Option<usize>,
    /// When true, mirror the full stream to `virtual_idx` (in addition to routing).
    mirror_on: bool,
}

impl<'a> MidiSink for PortFanoutSink<'a> {
    fn send(&mut self, msg: crate::midi::message::MidiMessage, at_micros: u64) {
        for idx in route_targets_with_mirror(
            &msg,
            self.channel_to_port,
            self.clock_ports,
            self.virtual_idx,
            self.mirror_on,
        ) {
            if let Some(ps) = self.ports.get_mut(idx) {
                ps.sink.send(msg.clone(), at_micros);
            }
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

    /// REGRESSION (no hardware): the two T-8 lanes (drums + bass) share the "T-8" port
    /// and MUST collapse to a SINGLE distinct port — one connection, so the 24 PPQN MIDI
    /// clock reaches the device ONCE (two connections doubled the T-8's tempo on real hardware).
    /// S-1 is a distinct port. Ported from `map_lanes_to_ports` to `build_route_plan` (T6).
    #[test]
    fn shared_port_lanes_collapse_to_one_connection() {
        let set = default_set();
        let plan = build_route_plan(&set.lanes);

        // Default profiles are [T8_DRUMS("T-8"), T8_BASS("T-8"), S1("S-1")].
        // Exactly TWO distinct physical ports, NOT three — the two T-8 lanes dedupe.
        assert_eq!(
            plan.ports.len(),
            2,
            "two T-8 lanes must collapse to one port"
        );
        assert_eq!(plan.ports[0].stable_key, "T-8");
        assert_eq!(plan.ports[1].stable_key, "S-1");

        // Both T-8 lanes map to the SAME port index (one shared connection).
        assert_eq!(
            plan.lane_to_port[0], plan.lane_to_port[1],
            "T-8 drums + bass share one port"
        );
        // S-1 maps to a different port.
        assert_ne!(
            plan.lane_to_port[2], plan.lane_to_port[0],
            "S-1 is a distinct port"
        );

        // One connection per distinct port: two lanes target T-8 but only ONE T-8 port exists.
        let t8_lanes = plan
            .lane_to_port
            .iter()
            .filter(|&&p| plan.ports[p].stable_key == "T-8")
            .count();
        assert_eq!(t8_lanes, 2, "two lanes target T-8");
        let t8_distinct_ports = plan.ports.iter().filter(|p| p.stable_key == "T-8").count();
        assert_eq!(
            t8_distinct_ports, 1,
            "but only ONE T-8 connection is opened"
        );
    }

    /// `build_route_plan` dedups shared ports and builds channel/clock maps (T6, pure).
    #[test]
    fn build_route_plan_dedups_shared_port() {
        let set = default_set();
        let plan = build_route_plan(&set.lanes);

        // Exactly two distinct ports (T-8 + S-1).
        assert_eq!(plan.ports.len(), 2);

        // Drums (ch9) and bass (ch1) both live on the T-8 port → same port index.
        let t8 = plan.channel_to_port[9];
        let bass = plan.channel_to_port[1];
        let s1 = plan.channel_to_port[0];
        assert!(t8.is_some() && bass.is_some() && s1.is_some());
        assert_eq!(t8, bass, "ch9 and ch1 both route to the T-8 port");
        assert_ne!(t8, s1, "S-1 (ch0) is a different port from T-8");

        // Both distinct ports send clock, each appearing exactly once (deduped).
        assert_eq!(plan.clock_ports.len(), 2, "both ports clock-out once each");
        let mut cp = plan.clock_ports.clone();
        cp.sort_unstable();
        cp.dedup();
        assert_eq!(cp.len(), 2, "clock_ports has no duplicates");
    }

    /// A lane whose route has `clock_out=false` is excluded from `clock_ports` (T6, pure).
    #[test]
    fn build_route_plan_respects_clock_out_false() {
        let mut set = default_set();
        // Give the S-1 lane an explicit route with clock disabled.
        let mut r = set.lanes[2].effective_route();
        r.clock_out = false;
        set.lanes[2].route = Some(r);

        let plan = build_route_plan(&set.lanes);
        let s1_idx = plan.channel_to_port[0].expect("S-1 mapped");
        assert!(
            !plan.clock_ports.contains(&s1_idx),
            "clock_out=false excludes the port from clock_ports; got {:?}",
            plan.clock_ports
        );
        // T-8 (drums/bass) still clocks out.
        let t8_idx = plan.channel_to_port[9].expect("T-8 mapped");
        assert!(plan.clock_ports.contains(&t8_idx), "T-8 still clocks out");
    }

    /// A channel message routes to ONLY its mapped port (T6, pure).
    #[test]
    fn route_targets_channel_msg_goes_to_its_port_only() {
        let mut channel_to_port = [None; 16];
        channel_to_port[9] = Some(0);
        channel_to_port[0] = Some(1);
        let clock_ports = vec![0, 1];

        let note = crate::midi::MidiMessage::NoteOn {
            channel: 9,
            note: 36,
            vel: 100,
        };
        assert_eq!(
            route_targets(&note, &channel_to_port, &clock_ports),
            vec![0]
        );

        let off = crate::midi::MidiMessage::NoteOff {
            channel: 0,
            note: 60,
        };
        assert_eq!(route_targets(&off, &channel_to_port, &clock_ports), vec![1]);

        let cc = crate::midi::MidiMessage::ControlChange {
            channel: 9,
            controller: 123,
            value: 0,
        };
        assert_eq!(route_targets(&cc, &channel_to_port, &clock_ports), vec![0]);
    }

    /// Clock goes to every clock-out port (T6, pure).
    #[test]
    fn route_targets_clock_goes_to_all_clock_ports() {
        let channel_to_port = [None; 16];
        let clock_ports = vec![0, 1];
        assert_eq!(
            route_targets(
                &crate::midi::MidiMessage::Clock,
                &channel_to_port,
                &clock_ports
            ),
            vec![0, 1]
        );
        // No clock ports → nothing.
        assert!(route_targets(&crate::midi::MidiMessage::Clock, &channel_to_port, &[]).is_empty());
    }

    /// An unmapped channel (no port) drops silently — empty targets (T6, pure).
    #[test]
    fn route_targets_unmapped_channel_is_dropped() {
        let channel_to_port = [None; 16]; // nothing mapped
        let clock_ports = vec![0];
        let note = crate::midi::MidiMessage::NoteOn {
            channel: 5,
            note: 60,
            vel: 64,
        };
        assert!(route_targets(&note, &channel_to_port, &clock_ports).is_empty());
        // Non-channel, non-clock realtime (Start) → empty regardless.
        assert!(route_targets(
            &crate::midi::MidiMessage::Start,
            &channel_to_port,
            &clock_ports
        )
        .is_empty());
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
        use crate::pattern::model::{MelodicNote, MelodicStep, Pattern, PatternData};

        // Build a set whose lane 2 (S-1, melodic) has a single long note on step 0.
        let mut set = default_set();
        // 16-step melodic pattern: step 0 has a long (full-bar) note, rest silent.
        let mut steps = vec![MelodicStep::default(); 16];
        steps[0] = MelodicStep::from(vec![MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 4.0, // 4 steps long — much longer than our tick window
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        set.lanes[2].pattern = Pattern {
            name: "test".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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
            micro: 0,
            cond: TrigCond::Always,
        });
        set.lanes[0].pattern = Pattern {
            name: "test".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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
        let set = default_set();
        let plan = build_route_plan(&set.lanes);
        let t8_idx = plan
            .ports
            .iter()
            .position(|p| p.stable_key == "T-8")
            .unwrap();
        let s1_idx = plan
            .ports
            .iter()
            .position(|p| p.stable_key == "S-1")
            .unwrap();
        assert_eq!(lanes_of(&plan.lane_to_port, t8_idx), vec![0, 1]);
        assert_eq!(lanes_of(&plan.lane_to_port, s1_idx), vec![2]);
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

    // --- Task 7: route channel emission + SetRoute -------------------------

    use crate::pattern::model::{
        LaneRoute, MelodicNote, MelodicStep, Pattern, PatternData, PortRef,
    };

    /// A melodic note on step 0 of the given lane (full-bar length so it stays sounding).
    fn put_long_note(set: &mut Set, lane: usize, semi: i8) {
        let mut steps = vec![MelodicStep::default(); 16];
        steps[0] = MelodicStep::from(vec![MelodicNote {
            semi,
            vel: 1.0,
            slide: false,
            len: 4.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        set.lanes[lane].pattern = Pattern {
            name: "t".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
    }

    fn route_on(channel: u8) -> LaneRoute {
        LaneRoute {
            port: PortRef {
                stable_key: "S-1".to_string(),
                name: "S-1".to_string(),
            },
            channel,
            clock_out: true,
        }
    }

    /// Important fix: a melodic lane with a route channel override emits its NoteOn on the
    /// ROUTE channel, not the profile channel.
    #[test]
    fn note_emits_on_route_channel_when_overridden() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        set.bpm = 120.0;
        let profile_ch = set.lanes[2].profile.channel; // S-1 → 0
        put_long_note(&mut set, 2, 0);
        set.lanes[2].route = Some(route_on(5));

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::Play)],
            5_000,
            500,
        );

        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { channel: 5, .. })),
            "NoteOn must fire on the route channel 5; got {:?}",
            sink.events
        );
        assert!(
            !sink.events.iter().any(
                |(_, m)| matches!(m, MidiMessage::NoteOn { channel, .. } if *channel == profile_ch)
            ),
            "no NoteOn should fire on the profile channel {profile_ch}"
        );
    }

    /// SetRoute changes a lane's channel mid-play: subsequent NoteOns use the NEW channel,
    /// and a NoteOff for the old channel is emitted at the switch (release before change).
    #[test]
    fn set_route_command_updates_lane_and_emits_on_new_channel() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        set.bpm = 120.0; // step_dur 125_000 µs
                         // Step 0: a long note (still sounding at the switch). Step 4: another
                         // note that lands AFTER the switch (so a post-switch NoteOn exists).
        let mut steps = vec![MelodicStep::default(); 16];
        steps[0] = MelodicStep::from(vec![MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 4.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        steps[4] = MelodicStep::from(vec![MelodicNote {
            semi: 2,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        set.lanes[2].pattern = Pattern {
            name: "t".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        set.lanes[2].route = Some(route_on(3)); // start on channel 3

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let switch_at = 50_000u64; // inside step 0's long note (still sounding) → release on switch
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![
                (0, UiCommand::Play),
                (
                    switch_at,
                    UiCommand::SetRoute {
                        lane: 2,
                        route: Some(route_on(8)),
                    },
                ),
            ],
            600_000,
            500,
        );

        // A NoteOff for the old channel 3 is emitted at/after the switch (release first).
        assert!(
            sink.events
                .iter()
                .any(|(at, m)| *at >= switch_at
                    && matches!(m, MidiMessage::NoteOff { channel: 3, .. })),
            "SetRoute must release the old-channel note at the switch; got {:?}",
            sink.events
        );
        // After the switch, NoteOns appear on the NEW channel 8.
        assert!(
            sink.events
                .iter()
                .any(|(at, m)| *at > switch_at
                    && matches!(m, MidiMessage::NoteOn { channel: 8, .. })),
            "after SetRoute, NoteOns must use the new channel 8; got {:?}",
            sink.events
        );
        // And no NoteOn appears on channel 3 after the switch.
        assert!(
            !sink
                .events
                .iter()
                .any(|(at, m)| *at > switch_at
                    && matches!(m, MidiMessage::NoteOn { channel: 3, .. })),
            "no NoteOn should fire on the old channel 3 after the switch"
        );
    }

    /// SetRoute on a lane with a SOUNDING note emits that note's NoteOff at the switch.
    #[test]
    fn set_route_releases_lane_before_switch() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        set.bpm = 120.0;
        put_long_note(&mut set, 2, 0); // long note on lane 2, sounding well past the switch
        set.lanes[2].route = Some(route_on(4));

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let switch_at = 50_000u64; // note (len 4 steps = 500_000) is still sounding here
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![
                (0, UiCommand::Play),
                (
                    switch_at,
                    UiCommand::SetRoute {
                        lane: 2,
                        route: Some(route_on(9)),
                    },
                ),
            ],
            100_000,
            500,
        );

        // NoteOff for the sounding note on the OLD channel 4, at the switch time.
        assert!(
            sink.events
                .iter()
                .any(|(at, m)| *at >= switch_at
                    && matches!(m, MidiMessage::NoteOff { channel: 4, .. })),
            "SetRoute must release the lane's sounding note (NoteOff ch4) at the switch; got {:?}",
            sink.events
        );
    }

    /// Pure: build_route_plan reflects a route change — a new channel maps to a port.
    #[test]
    fn build_route_plan_reflects_route_change() {
        let mut set = default_set();
        // Initially the S-1 lane (idx 2) is on channel 0.
        let plan0 = build_route_plan(&set.lanes);
        assert!(plan0.channel_to_port[0].is_some(), "ch0 mapped initially");
        assert!(plan0.channel_to_port[6].is_none(), "ch6 unmapped initially");

        // Override the S-1 lane to channel 6 on the same port.
        set.lanes[2].route = Some(route_on(6));
        let plan1 = build_route_plan(&set.lanes);
        assert!(
            plan1.channel_to_port[6].is_some(),
            "ch6 must be mapped after the route change"
        );
        // Same physical port (S-1) → same number of distinct ports.
        assert_eq!(plan0.ports.len(), plan1.ports.len());
    }

    /// Tap tempo: two taps 500 ms apart → engine emits EngineEvent::Tempo{bpm≈120}.
    #[test]
    fn tap_tempo_emits_tempo_event() {
        let set = default_set();
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        // Two taps: t=0 and t=500_000 µs → interval 500 ms → 120 BPM.
        let evs = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::Tap), (500_000, UiCommand::Tap)],
            600_000,
            1_000,
        );
        let tempo_event = evs.iter().find_map(|e| {
            if let EngineEvent::Tempo { bpm } = e {
                Some(*bpm)
            } else {
                None
            }
        });
        assert!(
            tempo_event.is_some(),
            "expected a Tempo event after two taps, got: {:?}",
            evs
        );
        let bpm = tempo_event.unwrap();
        assert!((bpm - 120.0).abs() < 2.0, "expected bpm ≈ 120, got {bpm}");
    }

    // --- Virtual "midip" port: first-class routable destination + folded mirror ---
    //
    // The virtual port is now a managed `PortSink` (NOT a separate TeeSink target). The
    // mirror is folded into the fan-out: when `mirror_on`, the FULL stream is delivered to
    // the virtual port index in ADDITION to whatever `route_targets` resolves, deduped so a
    // lane routed to "midip" while the mirror is also on is not sent twice. The earlier
    // M2.5 `TeeSink` tests were rewritten to assert the SAME observable mirror behavior via
    // the new `route_targets_with_mirror` path (TeeSink removed).

    use crate::pattern::model::VIRTUAL_PORT_KEY;

    /// Sink that records into a shared buffer the test can inspect AFTER it is type-erased
    /// into `Box<dyn MidiSink>` (as a `PortSink` stores it). `Arc<Mutex<..>>` (not `Rc`)
    /// because `MidiSink: Send`. Replaces the old `SharedRecordingSink` used by the TeeSink
    /// tests; same purpose, used now to inspect fan-out delivery per port.
    #[derive(Clone)]
    struct RecordingProbe {
        events: std::sync::Arc<std::sync::Mutex<Vec<(u64, crate::midi::MidiMessage)>>>,
    }
    impl RecordingProbe {
        fn new() -> Self {
            RecordingProbe {
                events: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
        fn len(&self) -> usize {
            self.events.lock().unwrap().len()
        }
    }
    impl MidiSink for RecordingProbe {
        fn send(&mut self, msg: crate::midi::MidiMessage, at_micros: u64) {
            self.events.lock().unwrap().push((at_micros, msg));
        }
    }

    /// A lane explicitly routed to the virtual "midip" port on the given channel.
    fn virtual_lane(set: &Set, lane: usize, channel: u8, clock_out: bool) -> Lane {
        let mut l = set.lanes[lane].clone();
        l.route = Some(LaneRoute {
            port: PortRef::virtual_midip(),
            channel,
            clock_out,
        });
        l
    }

    /// `build_route_plan_with_virtual` always includes the virtual "midip" port in its
    /// `ports` list (so it is a valid routable target even when no lane currently uses it).
    #[test]
    fn build_route_plan_includes_virtual_port() {
        let set = default_set();
        let plan = build_route_plan_with_virtual(&set.lanes);
        let vidx = virtual_port_index(&plan).expect("virtual port must be present in the plan");
        assert_eq!(plan.ports[vidx].stable_key, VIRTUAL_PORT_KEY);
        assert_eq!(plan.ports[vidx].name, "midip");
        // The two hardware ports (T-8, S-1) plus the virtual port = 3 distinct ports.
        assert_eq!(plan.ports.len(), 3, "two hardware ports + the virtual port");
    }

    /// A lane whose route key == VIRTUAL_PORT_KEY maps its channel to the virtual port index.
    #[test]
    fn lane_routed_to_virtual_maps_to_virtual_port() {
        let mut set = default_set();
        // Route lane 2 (S-1, melodic) to the virtual port on channel 7.
        set.lanes[2] = virtual_lane(&set, 2, 7, true);

        let plan = build_route_plan_with_virtual(&set.lanes);
        let vidx = virtual_port_index(&plan).expect("virtual port present");

        // Channel 7 routes to the virtual port index.
        assert_eq!(
            plan.channel_to_port[7],
            Some(vidx),
            "ch7 must map to the virtual port index"
        );
        // The lane itself maps to the virtual port index.
        assert_eq!(
            plan.lane_to_port[2], vidx,
            "lane 2 delivers to the virtual port"
        );
        // clock_out=true → the virtual port appears in clock_ports.
        assert!(
            plan.clock_ports.contains(&vidx),
            "clock_out lane routed to virtual must clock the virtual port"
        );
    }

    /// When a lane routes channel→virtual (and mirror is OFF), `route_targets_with_mirror`
    /// delivers that channel's messages to the virtual port index.
    #[test]
    fn route_targets_delivers_to_virtual_when_routed() {
        let mut channel_to_port = [None; 16];
        let vidx = 1usize;
        channel_to_port[3] = Some(vidx); // ch3 routed to the virtual port
        let clock_ports: Vec<usize> = vec![];

        let note = crate::midi::MidiMessage::NoteOn {
            channel: 3,
            note: 60,
            vel: 100,
        };
        // mirror_on=false: only the routed target.
        assert_eq!(
            route_targets_with_mirror(&note, &channel_to_port, &clock_ports, Some(vidx), false),
            vec![vidx]
        );
        // An unrouted channel with mirror off → nothing reaches the virtual port.
        let other = crate::midi::MidiMessage::NoteOn {
            channel: 9,
            note: 36,
            vel: 100,
        };
        assert!(route_targets_with_mirror(
            &other,
            &channel_to_port,
            &clock_ports,
            Some(vidx),
            false
        )
        .is_empty());
    }

    /// With mirror_on=true, the FULL stream (notes/CC + Clock) reaches the virtual port,
    /// regardless of routing — equivalent to the old additive TeeSink mirror.
    #[test]
    fn route_targets_delivers_everything_to_virtual_when_mirror_on() {
        let mut channel_to_port = [None; 16];
        let vidx = 0usize;
        // ch9 is routed to a DIFFERENT (hardware) port index 1; the virtual port is vidx 0.
        channel_to_port[9] = Some(1);
        let clock_ports: Vec<usize> = vec![1]; // hardware clocks; virtual not in clock_ports

        // A note on a hardware-routed channel: hardware target PLUS the virtual mirror.
        let note = crate::midi::MidiMessage::NoteOn {
            channel: 9,
            note: 36,
            vel: 100,
        };
        let mut got =
            route_targets_with_mirror(&note, &channel_to_port, &clock_ports, Some(vidx), true);
        got.sort_unstable();
        assert_eq!(
            got,
            vec![0, 1],
            "mirror adds the virtual port to the hardware target"
        );

        // Clock reaches every clock port PLUS the virtual port when mirror is on.
        let mut clk = route_targets_with_mirror(
            &crate::midi::MidiMessage::Clock,
            &channel_to_port,
            &clock_ports,
            Some(vidx),
            true,
        );
        clk.sort_unstable();
        assert_eq!(clk, vec![0, 1], "mirror clocks the virtual port too");

        // An UNROUTED channel still mirrors to the virtual port (full stream).
        let unrouted = crate::midi::MidiMessage::NoteOn {
            channel: 5,
            note: 40,
            vel: 64,
        };
        assert_eq!(
            route_targets_with_mirror(&unrouted, &channel_to_port, &clock_ports, Some(vidx), true),
            vec![vidx],
            "mirror delivers even unrouted channels to the virtual port"
        );
    }

    /// Dedup: a lane routed to the virtual port WHILE the mirror is also on must NOT send the
    /// message twice — the virtual port appears exactly once in the target list.
    #[test]
    fn mirror_plus_route_does_not_double_send() {
        let mut channel_to_port = [None; 16];
        let vidx = 0usize;
        channel_to_port[3] = Some(vidx); // ch3 ROUTED to the virtual port
        let clock_ports: Vec<usize> = vec![vidx]; // and the virtual port clocks out

        let note = crate::midi::MidiMessage::NoteOn {
            channel: 3,
            note: 60,
            vel: 100,
        };
        let targets =
            route_targets_with_mirror(&note, &channel_to_port, &clock_ports, Some(vidx), true);
        assert_eq!(
            targets,
            vec![vidx],
            "virtual port must appear exactly once (route + mirror deduped); got {targets:?}"
        );

        // Same for Clock: virtual port in clock_ports AND mirror on → still once.
        let clk = route_targets_with_mirror(
            &crate::midi::MidiMessage::Clock,
            &channel_to_port,
            &clock_ports,
            Some(vidx),
            true,
        );
        assert_eq!(
            clk,
            vec![vidx],
            "Clock to virtual must not double when in clock_ports AND mirror on; got {clk:?}"
        );
    }

    /// Rewritten M2.5 mirror test (was `tee_forwards_to_primary_always`): with mirror OFF the
    /// hardware path is unaffected and the virtual port receives nothing for an unrouted msg.
    #[test]
    fn fanout_mirror_off_does_not_reach_virtual() {
        let channel_to_port = [None; 16]; // nothing routed
        let vidx = 0usize;
        let clock_ports: Vec<usize> = vec![];
        // Clock with mirror off and virtual not in clock_ports → virtual gets nothing.
        let clk = route_targets_with_mirror(
            &crate::midi::MidiMessage::Clock,
            &channel_to_port,
            &clock_ports,
            Some(vidx),
            false,
        );
        assert!(
            !clk.contains(&vidx),
            "mirror OFF must not deliver to the virtual port; got {clk:?}"
        );
    }

    /// Rewritten M2.5 mirror test (was `tee_mirrors_when_on`): mirror ON delivers the full
    /// stream to the virtual port via a `PortFanoutSink` carrying RecordingSinks — the same
    /// observable behavior as the old TeeSink mirror, now folded into the fan-out.
    #[test]
    fn fanout_mirrors_full_stream_when_on() {
        // Two ports: index 0 = hardware (ch9 routed here), index 1 = virtual.
        let hw_probe = RecordingProbe::new();
        let virtual_probe = RecordingProbe::new();
        let mut ports = vec![
            PortSink {
                sink: Box::new(hw_probe.clone()),
                connected: true,
                port_name: "HW".into(),
            },
            PortSink {
                sink: Box::new(virtual_probe.clone()),
                connected: true,
                port_name: "midip".into(),
            },
        ];
        let mut channel_to_port = [None; 16];
        channel_to_port[9] = Some(0);
        let clock_ports = vec![0];
        let vidx = 1usize;

        {
            let mut fanout = PortFanoutSink {
                ports: &mut ports,
                channel_to_port: &channel_to_port,
                clock_ports: &clock_ports,
                virtual_idx: Some(vidx),
                mirror_on: true,
            };
            let note = crate::midi::MidiMessage::NoteOn {
                channel: 9,
                note: 36,
                vel: 100,
            };
            fanout.send(note, 999);
            fanout.send(crate::midi::MidiMessage::Clock, 1000);
        }

        // The virtual port (idx 1) must have received BOTH the note and the clock (mirror).
        assert_eq!(
            virtual_probe.len(),
            2,
            "mirror ON: virtual port must receive the full stream (note + clock)"
        );
        // Hardware port (idx 0) byte-identical: the note (routed) + the clock (clock_ports).
        assert_eq!(hw_probe.len(), 2, "hardware path unaffected by the mirror");
    }

    /// Rewritten M2.5 mirror test (was `tee_no_mirror_when_none`): when there is NO virtual
    /// port (None), mirror_on=true is a no-op and never panics; hardware path is untouched.
    #[test]
    fn fanout_no_virtual_port_is_noop_when_mirror_on() {
        let channel_to_port = [None; 16];
        let clock_ports = vec![0usize];
        // virtual_idx = None: mirror on must not add any phantom index.
        let clk = route_targets_with_mirror(
            &crate::midi::MidiMessage::Clock,
            &channel_to_port,
            &clock_ports,
            None,
            true,
        );
        assert_eq!(clk, vec![0], "no virtual port → mirror adds nothing");
    }

    /// CON ●: a lane routed to the virtual port reports `connected=true` whenever the virtual
    /// sink is the real virtual output (created). Pure status derivation.
    #[test]
    fn lane_routed_to_virtual_reports_connected() {
        let mut set = default_set();
        set.lanes[2] = virtual_lane(&set, 2, 7, true);
        let plan = build_route_plan_with_virtual(&set.lanes);
        let vidx = virtual_port_index(&plan).unwrap();

        // Build port_sinks mirroring spawn_engine: hardware ports NullSink/disconnected; the
        // virtual port carries the (simulated) created virtual sink → connected=true.
        let ports: Vec<PortSink> = plan
            .ports
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == vidx {
                    PortSink {
                        sink: Box::new(RecordingSink::new()), // stands in for the real virtual sink
                        connected: true,
                        port_name: p.name.clone(),
                    }
                } else {
                    PortSink {
                        sink: Box::new(NullSink),
                        connected: false,
                        port_name: String::new(),
                    }
                }
            })
            .collect();

        let mut events: Vec<EngineEvent> = Vec::new();
        emit_lane_status(&ports, &plan.lane_to_port, &mut events);

        let lane2 = events.iter().find_map(|e| match e {
            EngineEvent::DeviceStatus {
                lane: 2,
                connected,
                port,
            } => Some((*connected, port.clone())),
            _ => None,
        });
        assert_eq!(
            lane2,
            Some((true, "midip".to_string())),
            "lane routed to the virtual port must report connected=true with name 'midip'"
        );
    }

    /// SetMirror(true) sets mirror_on; SetMirror(false) clears it.
    #[test]
    fn set_mirror_toggles_flag() {
        let mut st = EngineState::new(default_set());
        assert!(!st.mirror_on, "mirror_on should start false");

        let mut events: Vec<EngineEvent> = Vec::new();
        apply_command(
            &mut st,
            UiCommand::SetMirror(true),
            0,
            &mut FakeLink::default(),
            &mut RecordingSink::default(),
            &mut events,
        );
        assert!(st.mirror_on, "SetMirror(true) must set mirror_on");

        apply_command(
            &mut st,
            UiCommand::SetMirror(false),
            0,
            &mut FakeLink::default(),
            &mut RecordingSink::default(),
            &mut events,
        );
        assert!(!st.mirror_on, "SetMirror(false) must clear mirror_on");
    }

    /// SetSet must mark `route_dirty` so the engine loop re-plans ports / re-spawns the
    /// watcher for the new set's routes (custom lane routes would mis-route without this).
    #[test]
    fn set_set_marks_route_dirty() {
        let mut st = EngineState::new(default_set());
        assert!(!st.route_dirty, "route_dirty should start false");

        let other = default_set();
        let mut events: Vec<EngineEvent> = Vec::new();
        apply_command(
            &mut st,
            UiCommand::SetSet(other),
            0,
            &mut FakeLink::default(),
            &mut RecordingSink::default(),
            &mut events,
        );

        assert!(
            st.route_dirty,
            "SetSet must set route_dirty so the loop re-plans routes"
        );
    }

    /// M3 Task 1: a QueuePattern while playing must, after the bar boundary, emit an
    /// `EngineEvent::Launched { lane, .. }` (so the UI can flip ACTIVE↔QUEUED).
    #[test]
    fn queue_then_launched_event_emitted() {
        use crate::engine::scheduler::Quant;
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0; // step_dur 125_000 µs; one bar = 16 * 125_000 = 2_000_000 µs

        // A replacement pattern: kick (note 50) on local step 0 of a 16-step lane.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note: 50,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let pattern = Pattern {
            name: "q".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };

        let mut link = FakeLink::new(); // manual transport
        let mut sink = RecordingSink::new();
        // Play at 0; queue on lane 0 at t=300_000 (mid-bar, NextBar); run past the
        // bar boundary at 2_000_000 µs.
        let evs = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![
                (0, UiCommand::Play),
                (
                    300_000,
                    UiCommand::QueuePattern {
                        lane: 0,
                        pattern,
                        quant: Quant::NextBar,
                    },
                ),
            ],
            2_300_000,
            1_000,
        );

        let launched = evs
            .iter()
            .find(|e| matches!(e, EngineEvent::Launched { lane: 0, .. }));
        assert!(
            launched.is_some(),
            "expected an EngineEvent::Launched{{lane:0,..}} after the bar boundary; got: {:?}",
            evs
        );
    }

    /// CancelQueue clears a pending launch so no Launched event ever fires.
    #[test]
    fn cancel_queue_prevents_launched_event() {
        use crate::engine::scheduler::Quant;
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0;
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note: 50,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let pattern = Pattern {
            name: "q".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let evs = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![
                (0, UiCommand::Play),
                (
                    300_000,
                    UiCommand::QueuePattern {
                        lane: 0,
                        pattern,
                        quant: Quant::NextBar,
                    },
                ),
                (600_000, UiCommand::CancelQueue { lane: 0 }),
            ],
            2_300_000,
            1_000,
        );

        assert!(
            !evs.iter()
                .any(|e| matches!(e, EngineEvent::Launched { .. })),
            "cancelled queue must not emit a Launched event; got: {:?}",
            evs
        );
    }

    /// M7 Task 5 note-safety: the App drives chain auto-advance by emitting the existing
    /// `QueueScene` recall on each bar boundary and a terminal `UiCommand::Stop` at
    /// stop-at-end. This test reproduces that exact command sequence against a set whose
    /// lane sustains a long note ACROSS each transition, then asserts that every NoteOn is
    /// matched by a NoteOff (net == 0 per (channel,note)) — i.e. no hung notes across the
    /// scene swaps or the final stop. All emission rides the already-tested recall +
    /// `seq.stop` (release-before-swap, M1 registry) paths; chain playback adds none.
    #[test]
    fn chain_recall_transitions_leave_no_hung_notes() {
        use crate::engine::scheduler::{LaunchState, Quant};
        use crate::midi::MidiMessage;
        use crate::pattern::model::{MelodicNote, MelodicStep, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0; // one 16th = 125_000 µs, one bar (16 steps) = 2_000_000 µs

        // A pattern that holds a note for a full bar (len 16) on lane 2 so it is still
        // sounding when the next scene's QueueScene swaps the lane at the bar boundary.
        let held = |semi: i8| -> Pattern {
            let mut steps = vec![MelodicStep::default(); 16];
            steps[0] = MelodicStep::from(vec![MelodicNote {
                semi,
                vel: 1.0,
                slide: false,
                len: 16.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }]);
            Pattern {
                name: "held".into(),
                desc: String::new(),
                length: 16,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            }
        };
        let state = LaunchState {
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
        };

        let bar = 2_000_000u64;
        let step = 125_000u64;
        let mut link = FakeLink::new(); // manual transport
        let mut sink = RecordingSink::new();
        // Mirror the App: recall entry 0 at play, entry 1 queued before bar boundary 1,
        // then Stop at the end of entry 1 (bar boundary 2). Each recall is ONE QueueScene
        // carrying lane 2 (the held note), exactly as `recall_scene` builds it.
        let _ = run_engine_headless(
            set,
            &mut link,
            &mut sink,
            vec![
                (0, UiCommand::Play),
                (
                    0,
                    UiCommand::QueueScene {
                        quant: Quant::NextBar,
                        lanes: vec![(2, held(0), state)],
                    },
                ),
                (
                    bar - step,
                    UiCommand::QueueScene {
                        quant: Quant::NextBar,
                        lanes: vec![(2, held(5), state)],
                    },
                ),
                (2 * bar, UiCommand::Stop),
            ],
            2 * bar + step, // a hair past the stop
            step / 4,
        );

        // Net per-(channel,note) balance: NoteOn (vel>0) = +1, NoteOff (or vel==0) = -1.
        use std::collections::HashMap;
        let mut net: HashMap<(u8, u8), i32> = HashMap::new();
        for (_, msg) in &sink.events {
            match msg {
                MidiMessage::NoteOn { channel, note, vel } if *vel > 0 => {
                    *net.entry((*channel, *note)).or_insert(0) += 1;
                }
                MidiMessage::NoteOn { channel, note, .. } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                MidiMessage::NoteOff { channel, note } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                _ => {}
            }
        }
        let hung: Vec<_> = net.iter().filter(|(_, &v)| v > 0).collect();
        assert!(
            hung.is_empty(),
            "no hung notes across chain transitions + stop; leftover: {hung:?}; events: {:?}",
            sink.events
        );
        // Sanity: at least one note actually sounded (otherwise the test is vacuous).
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { vel, .. } if *vel > 0)),
            "expected real NoteOns in the run; got {:?}",
            sink.events
        );
    }
}
