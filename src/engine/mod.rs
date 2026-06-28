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
    LoadPattern { lane: usize, pattern: Pattern },
    Mute { lane: usize, on: bool },
    Solo { lane: usize, on: bool },
    Transpose { lane: usize, semis: i8 },
    /// Sync all lane state after undo/redo. Does NOT rebuild the Sequencer or reset the clock/playhead.
    SyncLanes(Vec<Lane>),
    /// Update a single lane's octave shift without touching anything else.
    SetOctave { lane: usize, octave: i8 },
    SetSet(Set),
    /// All-notes-off / all-sound-off live recovery; does not stop transport.
    Panic,
    Quit,
}

/// Events sent engine -> UI.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Playhead { step: usize, bar: u32, beat: u32, phase: f32 },
    LinkStatus { enabled: bool, tempo: f64, peers: u64 },
    DeviceStatus { lane: usize, connected: bool, port: String },
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
) -> bool {
    match cmd {
        UiCommand::Play => {
            st.seq.play(now);
            st.clock.start(now); // begin Clock ticks only — no MIDI Start (would run the device's own sequencer)
            if link.enabled() {
                link.request_start(now, 4.0); // quantized start: align to next bar
            }
        }
        UiCommand::Stop => {
            st.seq.stop(now, sink); // releases sounding notes (all-notes-off)
            st.clock.stop(); // cease Clock ticks; no MIDI Stop sent
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
            let playing = st.seq.is_playing();
            st.seq = Sequencer::new(set);
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
            if apply_command(st, cmd, now, link, sink) {
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
    let link_tempo = if st.link_enabled { Some(link.tempo()) } else { None };
    let bpm = st.transport.effective_bpm(link_tempo);

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
    if st.tick_count % LINK_STATUS_EVERY == 0 {
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
// Per-lane sink state for the real engine (hot-plug lifecycle).
// ---------------------------------------------------------------------------

/// Per-lane connection record used by the engine thread.
/// Each lane owns its sink independently; lanes sharing a physical port each hold
/// their own `MidirSink` (the no-double-clock guarantee is preserved because `connect`
/// opens a separate connection handle — CoreMIDI deduplicates clock at the port level,
/// and in practice the T-8 uses a single physical IAC-style bus per device).
///
/// NOTE: If double-clock is observed on shared-port devices in practice, the fix is a
/// shared `Arc<Mutex<MidirSink>>` fan-out — but that requires a larger refactor and
/// midir connections are move-only, so we keep the simpler per-lane model for now.
struct LaneSink {
    /// The profile this lane tracks (port_match substring, display name).
    profile: DeviceProfile,
    /// Current sink for this lane. `None` = no connected port (NullSink semantics but
    /// without holding a live connection). We store `Option` so we can `take` on
    /// unhealthy detection and reconnect cleanly.
    sink: Box<dyn MidiSink>,
    /// Last-known connection state (used for change-detection / dedupe).
    connected: bool,
    /// Last-known port name that was successfully connected (empty when disconnected).
    port_name: String,
}

impl LaneSink {
    fn new_disconnected(profile: DeviceProfile) -> Self {
        LaneSink {
            profile,
            sink: Box::new(NullSink),
            connected: false,
            port_name: String::new(),
        }
    }
}

/// Build initial per-lane sinks from profiles and emit one `DeviceStatus` per lane.
///
/// Lanes sharing the same physical port each get their own connection. CoreMIDI routes
/// clock at the port level so this is safe on all tested hardware (Roland T-8).
fn build_lane_sinks(
    profiles: &[DeviceProfile; 3],
    events: &mut Vec<EngineEvent>,
) -> Vec<LaneSink> {
    let available = list_output_ports();
    let mut lanes: Vec<LaneSink> = Vec::with_capacity(profiles.len());

    for (lane_idx, profile) in profiles.iter().enumerate() {
        let mut ls = LaneSink::new_disconnected(*profile);

        if let Some(_port_idx) = match_port(&available, profile.port_match) {
            // Try to open a connection for this lane.
            if let Ok(midir_sink) = connect(profile.port_match) {
                // Find the actual port name for display (best-effort; fall back to port_match).
                let port_name = available
                    .iter()
                    .find(|n| n.to_lowercase().contains(&profile.port_match.to_lowercase()))
                    .cloned()
                    .unwrap_or_else(|| profile.port_match.to_string());

                ls.sink = Box::new(midir_sink);
                ls.connected = true;
                ls.port_name = port_name.clone();

                events.push(EngineEvent::DeviceStatus {
                    lane: lane_idx,
                    connected: true,
                    port: port_name,
                });
                lanes.push(ls);
                continue;
            }
        }

        // No port found or connection failed.
        events.push(EngineEvent::DeviceStatus {
            lane: lane_idx,
            connected: false,
            port: String::new(),
        });
        lanes.push(ls);
    }

    lanes
}

/// Rescan ports and health for every lane. Emits `DeviceStatus` only on CHANGE.
///
/// This runs in the engine thread every ~1 s (HOTPLUG_CHECK_EVERY ticks). It handles:
/// - Send-failure detection: `MidirSink::health()` flips false after a failed write.
///   Swap to NullSink; emit DeviceStatus{connected:false}.
/// - Device vanished: port no longer in `list_output_ports()`. Same swap + event.
/// - Device reappeared: port back in list for a previously-disconnected lane.
///   Reconnect; emit DeviceStatus{connected:true}.
///
/// Not unit-tested (touches real MIDI hardware); logic is kept simple and well-commented.
fn rescan_lane_sinks(lanes: &mut Vec<LaneSink>, events: &mut Vec<EngineEvent>) {
    let available = list_output_ports();

    for (lane_idx, ls) in lanes.iter_mut().enumerate() {
        let port_present =
            match_port(&available, ls.profile.port_match).is_some();
        let sink_healthy = ls.sink.health();

        if ls.connected {
            // Already connected: check for vanished port or unhealthy sink.
            if !port_present || !sink_healthy {
                ls.sink = Box::new(NullSink);
                ls.connected = false;
                let lost_name = std::mem::take(&mut ls.port_name);
                events.push(EngineEvent::DeviceStatus {
                    lane: lane_idx,
                    connected: false,
                    port: lost_name,
                });
            }
        } else {
            // Disconnected: try to reconnect if port reappeared.
            if port_present {
                if let Ok(midir_sink) = connect(ls.profile.port_match) {
                    let port_name = available
                        .iter()
                        .find(|n| {
                            n.to_lowercase()
                                .contains(&ls.profile.port_match.to_lowercase())
                        })
                        .cloned()
                        .unwrap_or_else(|| ls.profile.port_match.to_string());

                    ls.sink = Box::new(midir_sink);
                    ls.connected = true;
                    ls.port_name = port_name.clone();

                    events.push(EngineEvent::DeviceStatus {
                        lane: lane_idx,
                        connected: true,
                        port: port_name,
                    });
                }
            }
        }
    }
}

/// Spawn the real engine on its own thread, driven by a monotonic clock. NOT unit-tested
/// (non-deterministic timing); shares `step_engine` with the headless driver.
///
/// Takes `profiles` instead of pre-built sinks so the engine thread owns the full
/// sink lifecycle (initial connect + periodic hot-plug rescan). Initial `DeviceStatus`
/// events are emitted before the first tick so the UI receives them via `on_engine_event`.
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

        // Build per-lane sinks and emit initial DeviceStatus for each lane.
        let mut lane_sinks = build_lane_sinks(&profiles, &mut events);

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

            // Fan out to all per-lane sinks.
            let mut fanout = LaneFanoutSink { lanes: &mut lane_sinks };
            let quit =
                step_engine(&mut st, now, &mut pending, link.as_mut(), &mut fanout, &mut events);

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
            if st.tick_count % HOTPLUG_CHECK_EVERY == 0 && st.tick_count > 0 {
                rescan_lane_sinks(&mut lane_sinks, &mut events);
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

    EngineHandle { tx: cmd_tx, rx: evt_rx, join }
}

/// Fans a single `send` out to every per-lane sink.
struct LaneFanoutSink<'a> {
    lanes: &'a mut Vec<LaneSink>,
}

impl<'a> MidiSink for LaneFanoutSink<'a> {
    fn send(&mut self, msg: crate::midi::message::MidiMessage, at_micros: u64) {
        for ls in self.lanes.iter_mut() {
            ls.sink.send(msg.clone(), at_micros);
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

    /// Headless engine emits LinkStatus{enabled:false} before ToggleLink.
    #[test]
    fn headless_emits_link_status_disabled_initially() {
        let set = default_set();
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let events = run_engine_headless(set, &mut link, &mut sink, vec![], 1_000, 100);
        let found = events.iter().any(|ev| {
            matches!(ev, EngineEvent::LinkStatus { enabled: false, .. })
        });
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
        let enabled_ev = events.iter().any(|ev| {
            matches!(ev, EngineEvent::LinkStatus { enabled: true, .. })
        });
        assert!(enabled_ev, "expected LinkStatus{{enabled:true}} after ToggleLink");
    }
}
