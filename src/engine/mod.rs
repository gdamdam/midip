//! Engine thread entry point and the deterministic headless driver.
//!
//! The real engine (`spawn_engine`) runs `step_engine` in a loop on a monotonic clock;
//! the test driver (`run_engine_headless`) runs the *same* `step_engine` over a virtual
//! clock. Only the headless driver is unit-tested — the threaded one is not deterministic.

pub mod clock;
pub mod clock_in;
pub mod scheduler;
pub mod transport;

use crate::engine::clock_in::{ClockInMsg, ClockInState};
use crate::link::LinkClock;
use crate::midi::ports::{
    connect, connect_clock_in, list_output_ports, match_port, MidiSink, MidirClockIn, NullSink,
};
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
    /// Like `SetBpm`, but for undo/redo: restore the stored BPM WITHOUT changing the active
    /// tempo source. `SetBpm` is an explicit manual override (it disables Link/ClockIn);
    /// undoing an unrelated edit must never drop the user out of Link or Clock-In.
    RestoreBpm(f64),
    Tap,
    SetSwing(f32),
    ToggleLink(bool),
    LoadPattern {
        lane: usize,
        pattern: Pattern,
    },
    /// H1: push per-lane timing overrides (swing / clock division) to the engine.
    /// `LoadPattern` only carries the pattern, so swing/clock_div edits would otherwise
    /// never reach the live scheduler until an unrelated reload. Sent by the lane
    /// swing/clock-div handlers so timing changes take effect immediately.
    UpdateLaneParams {
        lane: usize,
        swing: Option<f32>,
        clock_div: Option<u8>,
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
    /// M10 T4: select (or clear) the external MIDI clock-input port AND switch the
    /// tempo source to `ClockIn`. `Some(port)` follows that port's clock; `None` clears
    /// the port and reverts the source to `Manual`. In the real engine the
    /// `spawn_engine` loop also intercepts this to (re)connect the clock-input watcher.
    /// Switching to ClockIn releases the other tempo sources (see `apply_command`).
    SetClockInPort(Option<crate::pattern::model::PortRef>),
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
    /// M10 T4: external MIDI clock-input status. `locked` is true while ticks are arriving
    /// (a stable tempo is being followed) and false when the clock is lost / stopped.
    /// `tempo` is the followed BPM (smoothed; falls back to manual when not yet locked).
    /// `deviation` is the |followed − manual| BPM gap (display hint; 0.0 when not locked).
    ClockInStatus {
        locked: bool,
        tempo: f64,
        deviation: f64,
    },
}

/// Handle returned by `spawn_engine`.
pub struct EngineHandle {
    pub tx: crossbeam_channel::Sender<UiCommand>,
    pub rx: crossbeam_channel::Receiver<EngineEvent>,
    pub join: std::thread::JoinHandle<()>,
}

/// Emit a `LinkStatus` event roughly this often (in ticks) to avoid flooding.
const LINK_STATUS_EVERY: u64 = 200;

/// M10 T4: clock-in loss timeout, expressed in MIDI ticks of silence. 24 ticks = one
/// quarter note at 24 PPQN — a full beat with no incoming tick means the external clock
/// has stopped or vanished, so we halt + release. Tolerant of normal inter-tick jitter.
const CLOCK_IN_TIMEOUT_TICKS: u64 = 24;

/// Floor for the clock-in loss timeout (µs) used before any tempo is known (no lock yet),
/// or as a lower bound at very fast tempos. 500 ms ≈ one beat at 120 BPM.
const CLOCK_IN_TIMEOUT_FLOOR_MICROS: u64 = 500_000;

/// Derive the clock-in loss timeout (µs) from the currently-followed/expected BPM.
/// One MIDI tick ≈ `step_dur_micros(bpm) / 6` (6 ticks per 16th step). The timeout is
/// `CLOCK_IN_TIMEOUT_TICKS` such intervals, floored at `CLOCK_IN_TIMEOUT_FLOOR_MICROS`.
fn clock_in_timeout_micros(bpm: f64) -> u64 {
    let tick_interval = scheduler::step_dur_micros(bpm) / 6;
    (tick_interval * CLOCK_IN_TIMEOUT_TICKS).max(CLOCK_IN_TIMEOUT_FLOOR_MICROS)
}

/// Shared-timeline bar index for a Link `beat` at the given `quantum` (beats/bar):
/// `floor(beat / quantum)`. Negative for the pre-start count-in (a quantized
/// `request_start` runs beats up from a negative value to 0 at the next bar), so
/// a Link-gated start fires exactly when this index advances — the same test for
/// the local count-in and for joining an already-playing session at its next bar.
fn bar_index(beat: f64, quantum: f64) -> i64 {
    (beat / quantum).floor() as i64
}

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
    /// Shared bar index (`bar_index(beat)`) captured when `armed` was set. The
    /// sequencer starts once the Link beat crosses into a LATER bar than this, so
    /// both a local count-in and a remote-session join begin on a bar boundary.
    armed_at_bar: i64,
    /// Last observed Link session playing state (meaningful only while
    /// `link_enabled`). Used to follow remote start/stop transitions and to
    /// suppress command echo (our own start is recorded here, not re-followed).
    link_playing: bool,
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
    /// M10 T4: pure clock-input accumulator (tick→step boundary + tempo smoothing).
    /// Driven only while `transport.source == ClockIn` and ticks are draining in.
    clock_in_state: ClockInState,
    /// Smoothed clock-in BPM (None until enough ticks for a stable estimate). Passed to
    /// `Transport::effective_bpm` so the clock generator + scheduler follow the external tempo.
    clock_in_bpm: Option<f64>,
    /// Last received Song Position Pointer beat (SPP). PARSED + STORED ONLY — the engine does
    /// NOT reposition on it yet (deferred per spec; the field makes T-future SPP-ready).
    clock_in_song_position: Option<u16>,
    /// Whether the external clock is currently locked (ticks arriving). Tracks the last
    /// reported state so `ClockInStatus` is emitted only on a lock↔unlock transition.
    clock_in_locked: bool,
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
            armed_at_bar: 0,
            link_playing: false,
            last_step: None,
            bar: 0,
            tick_count: 0,
            route_dirty: false,
            mirror_on: false,
            clock_in_state: ClockInState::new(),
            clock_in_bpm: None,
            clock_in_song_position: None,
            clock_in_locked: false,
        }
    }
}

/// M10 T4: leave the ClockIn tempo source (if active), restoring internal-timing advance.
/// Idempotent. Disengages clock-driven step advance and forgets the followed tempo, but does
/// NOT stop the transport — the caller decides whether playback continues under the new
/// source. Used when switching to Manual/Link or clearing the clock-in port.
fn disengage_clock_in(st: &mut EngineState) {
    st.seq.set_clock_driven(false);
    st.clock_in_bpm = None;
    st.clock_in_locked = false;
    st.clock_in_state.reset();
}

/// Disable Ableton Link if engaged. Mirrors `disengage_clock_in`: used when an explicit
/// manual tempo (SetBpm / Tap) takes over, so tempo sources stay mutually exclusive —
/// manual BPM and Link must never both drive the playhead (Link would keep phase-syncing
/// the position even while the BPM reads manual). Flips the LinkClock session off, not just
/// the local flag; the periodic `LinkStatus` event then updates the UI.
fn disengage_link(st: &mut EngineState, link: &mut dyn LinkClock) {
    if st.link_enabled {
        st.link_enabled = false;
        link.set_enabled(false);
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
                //
                // Idempotent join (behavior 3/6): only issue a start when the shared
                // session is stopped. If a peer is already playing we simply arm and
                // join at the next bar — sending another start would remap the beat
                // under the peers' feet and echo back as a redundant transition.
                if !link.is_playing() {
                    link.request_start(now, 4.0);
                }
                st.clock.start(now);
                st.armed = true;
                st.armed_at_bar = bar_index(link.beat_at(now, 4.0), 4.0);
                // Record that the session is (about to be) playing on our account so
                // step_engine's transition check does not re-follow our own start.
                st.link_playing = true;
                events.push(EngineEvent::Armed);
            } else {
                // Manual mode: start immediately and confirm.
                // H4: `play` clears the sounding registry without emitting NoteOffs, so a
                // restart while notes are still held would hang them on hardware. Flush the
                // previous run's notes first (same all-notes-off path used by Stop/SetSet).
                if st.seq.is_playing() {
                    st.seq.release_all(now, sink);
                }
                st.seq.play(now);
                st.clock.start(now); // begin Clock ticks only — no MIDI Start (would run the device's own sequencer)
                events.push(EngineEvent::Started { at_step: 0 });
            }
        }
        UiCommand::Stop => {
            st.seq.stop(now, sink); // releases sounding notes (all-notes-off)
            st.clock.stop(); // cease Clock ticks; no MIDI Stop sent
            st.armed = false;
            // H2: publish the stop to Link so peers following our transport stop too
            // (mirrors the Play path's `if link.enabled()` guard). Reset the local latch
            // so the remote-follow transition check and the idempotent-join guard
            // (`if !link.is_playing()`) behave correctly on the next Play.
            if link.enabled() {
                link.request_stop(now);
                st.link_playing = false;
            }
            events.push(EngineEvent::Stopped);
        }
        UiCommand::SetBpm(bpm) => {
            // 3-way exclusivity: an explicit manual BPM takes over from Link AND ClockIn,
            // so the two never both drive the playhead (Link would keep phase-syncing).
            disengage_clock_in(st);
            disengage_link(st, link);
            st.transport.manual_bpm = bpm;
            st.transport.source = TempoSource::Manual(bpm);
            st.seq.set_bpm(bpm);
        }
        UiCommand::RestoreBpm(bpm) => {
            // Undo/redo path: refresh the stored manual BPM WITHOUT changing the active tempo
            // source (unlike SetBpm). Applies to playback only when already Manual; under Link
            // or ClockIn the external source keeps driving and only manual_bpm is updated (used
            // if the user later leaves that source). Prevents undo from dropping out of Link.
            st.transport.manual_bpm = bpm;
            if let TempoSource::Manual(_) = st.transport.source {
                st.transport.source = TempoSource::Manual(bpm);
                st.seq.set_bpm(bpm);
            }
        }
        UiCommand::Tap => {
            // 3-way exclusivity: tapping a tempo takes over from Link AND ClockIn.
            disengage_clock_in(st);
            disengage_link(st, link);
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
            // 3-way exclusivity: enabling Link takes over from ClockIn (and vice versa).
            if on {
                disengage_clock_in(st);
            }
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
            // Reset the remote-transport tracker on every toggle. Left false so that
            // enabling Link into an already-playing session is seen as a stopped→playing
            // transition next tick and joins at the next bar (behavior 3).
            st.link_playing = false;
        }
        UiCommand::LoadPattern { lane, pattern } => {
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.pattern = pattern;
                st.seq.update_lane(lane, l);
            }
        }
        UiCommand::UpdateLaneParams {
            lane,
            swing,
            clock_div,
        } => {
            // H1: mirror the lane's swing/clock_div into the engine lane state so the
            // scheduler (which reads `lanes[i].swing` / `.clock_div`) picks them up now.
            if let Some(existing) = st.seq.lane(lane) {
                let mut l = existing.clone();
                l.swing = swing;
                l.clock_div = clock_div;
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
            //
            // Undo/redo can also revert a lane's route or device profile (the device
            // picker and route editor both snapshot). For any lane whose effective
            // route changed, release its sounding notes BEFORE swapping in the restored
            // lane — so the NoteOffs go out on the still-active pre-undo channel — and
            // flag a port re-plan. Without this the engine keeps routing to the
            // post-change device and held notes hang on the wrong port. Lanes whose
            // routing is unchanged (e.g. undoing a step edit) are left untouched so a
            // plain edit-undo never needlessly cuts sounding notes.
            let mut routing_changed = false;
            for (i, lane) in lanes.into_iter().enumerate() {
                let route_changed = st
                    .seq
                    .lane(i)
                    .map(|cur| cur.effective_route() != lane.effective_route())
                    .unwrap_or(false);
                if route_changed {
                    st.seq.release_lanes(&[i], now, sink);
                    routing_changed = true;
                }
                st.seq.update_lane(i, lane);
            }
            if routing_changed {
                st.route_dirty = true;
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
        UiCommand::SetClockInPort(port) => {
            match port {
                Some(_) => {
                    // Enter ClockIn: disable Link (3-way exclusivity), engage clock-driven
                    // advance, and follow the external clock. The actual port (re)connection
                    // is handled by the spawn_engine loop, which intercepts this command and
                    // talks to the clock-input watcher; here we own the transport-source state.
                    if st.link_enabled {
                        st.link_enabled = false;
                        link.set_enabled(false);
                    }
                    // Fresh lock attempt: clear stale tick/tempo history so a previously-lost
                    // source does not immediately re-trigger a loss on the first new tick.
                    st.clock_in_state.reset();
                    st.clock_in_bpm = None;
                    st.clock_in_locked = false;
                    st.transport.source = TempoSource::ClockIn;
                    st.seq.set_clock_driven(true);
                }
                None => {
                    // Clear the clock-in port: leave ClockIn, revert to Manual. If we were
                    // playing under the external clock, the next internal tick resumes timing
                    // from the current position at manual_bpm.
                    disengage_clock_in(st);
                    if !st.link_enabled {
                        st.transport.source = TempoSource::Manual(st.transport.manual_bpm);
                    }
                }
            }
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
    clock_in_msgs: &mut Vec<ClockInMsg>,
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

    // 1b. M10 T4 — drain incoming external MIDI clock messages. They are acted upon ONLY
    //     while the tempo source is ClockIn; otherwise they are drained + discarded (a stale
    //     port may still be delivering until the watcher reconnects). `clock_advanced`
    //     captures the last step materialized by a tick-driven advance this iteration, so the
    //     Playhead emit below covers both the internal and clock-driven advance paths.
    let mut clock_advanced: Option<usize> = None;
    let following_clock = matches!(st.transport.source, TempoSource::ClockIn);
    for msg in clock_in_msgs.drain(..) {
        if !following_clock {
            continue;
        }
        match msg {
            ClockInMsg::Tick => {
                // Advance the step accumulator; on every 6th tick (one 16th step) drive the
                // sequencer forward by EXACTLY one step (tick-driven, NOT step_dur timing).
                let step_due = st.clock_in_state.on_tick(now);
                // Update the followed tempo from the smoothed estimate (when stable).
                if let Some(bpm) = st.clock_in_state.smoothed_bpm() {
                    st.clock_in_bpm = Some(bpm);
                }
                if step_due {
                    let bpm = st.transport.effective_bpm(None, st.clock_in_bpm);
                    if let Some(s) = st.seq.advance_clock_step(now, bpm, sink) {
                        clock_advanced = Some(s);
                    }
                    for lane in st.seq.take_launched() {
                        events.push(EngineEvent::Launched {
                            lane,
                            step: st.seq.current_step(),
                        });
                    }
                }
                // Newly locked? Emit a status transition (false -> true).
                if !st.clock_in_locked {
                    st.clock_in_locked = true;
                    let tempo = st.clock_in_bpm.unwrap_or(st.transport.manual_bpm);
                    events.push(EngineEvent::ClockInStatus {
                        locked: true,
                        tempo,
                        deviation: (tempo - st.transport.manual_bpm).abs(),
                    });
                }
            }
            ClockInMsg::Start => {
                // Restart the tick accumulator and play from the top (origin).
                st.clock_in_state.reset();
                st.clock_in_song_position = Some(0);
                // H4: an external Start while already playing must release the sounding
                // notes before `play` clears the registry — otherwise they hang (play
                // emits no NoteOffs). Reuse the all-notes-off path used by Stop.
                if st.seq.is_playing() {
                    st.seq.release_all(now, sink);
                }
                st.seq.play(now);
                events.push(EngineEvent::Started { at_step: 0 });
            }
            ClockInMsg::Continue => {
                // Resume at the current position (do NOT reset playhead/origin). If the
                // sequencer was stopped, mark it playing again from where it sits.
                if !st.seq.is_playing() {
                    st.seq.resume(now);
                    events.push(EngineEvent::Started {
                        at_step: st.seq.current_step(),
                    });
                }
            }
            ClockInMsg::Stop => {
                // Release all sounding notes via the existing stop path (M1 note-safety).
                st.seq.stop(now, sink);
                events.push(EngineEvent::Stopped);
                // Clear the lock so the loss-timeout guard does not fire spuriously after a
                // clean external Stop — a graceful Stop is not a clock loss.
                st.clock_in_locked = false;
            }
            ClockInMsg::SongPosition(pos) => {
                // SPP: parse + STORE only. DO NOT reposition (deferred per spec).
                st.clock_in_song_position = Some(pos);
            }
            ClockInMsg::Other => {}
        }
    }

    // 1c. M10 T4 — clock-in loss detection. While following an external clock that HAD locked
    //     (a tick was received), if no tick has arrived within the tempo-derived timeout, treat
    //     it as a Stop: halt + release all notes (existing path), report unlocked. No drift,
    //     no runaway (the suppressed internal-timing advance never fills the gap).
    if following_clock && st.clock_in_locked {
        let timeout = clock_in_timeout_micros(st.clock_in_bpm.unwrap_or(st.transport.manual_bpm));
        if st.clock_in_state.is_lost(now, timeout) {
            st.seq.stop(now, sink); // releases every sounding note (all-notes-off)
            st.clock_in_locked = false;
            st.clock_in_bpm = None;
            let tempo = st.transport.manual_bpm;
            events.push(EngineEvent::ClockInStatus {
                locked: false,
                tempo,
                deviation: 0.0,
            });
            events.push(EngineEvent::Stopped);
        }
    }

    // 2. Tempo source resolution via Transport::effective_bpm.
    //    `ToggleLink`/`SetClockInPort` keep `transport.source` in sync (Manual ↔ Link ↔
    //    ClockIn), so this call is the single authoritative BPM resolution path for both
    //    headless tests and the real engine thread.
    let link_tempo = if st.link_enabled {
        Some(link.tempo())
    } else {
        None
    };
    let bpm = st.transport.effective_bpm(link_tempo, st.clock_in_bpm);

    // Follow remote transport (Link start/stop sync). While Link is enabled, watch
    // the shared session's playing state and react to transitions only — a steady
    // state (and our own locally-initiated start, recorded in `link_playing`) does
    // nothing, so there is no command echo and no per-tick restart.
    if st.link_enabled {
        let remote_playing = link.is_playing();
        if remote_playing != st.link_playing {
            if remote_playing {
                // stopped → playing: a peer started (or we joined an already-playing
                // session via ToggleLink). Arm to join at the NEXT shared bar; do NOT
                // issue our own start — the session is already running.
                if !st.seq.is_playing() && !st.armed {
                    st.armed = true;
                    st.armed_at_bar = bar_index(link.beat_at(now, 4.0), 4.0);
                    st.clock.start(now);
                    events.push(EngineEvent::Armed);
                }
            } else {
                // playing → stopped: follow the remote stop immediately, releasing
                // every sounding note (all-notes-off via seq.stop) and disarming.
                if st.seq.is_playing() || st.armed {
                    st.seq.stop(now, sink);
                    st.clock.stop();
                    st.armed = false;
                    events.push(EngineEvent::Stopped);
                }
            }
            st.link_playing = remote_playing;
        }
    }

    // Link-gated start: fire the sequencer once the Link beat crosses into a bar
    // LATER than the one captured when we armed. This single test covers the local
    // count-in (request_start runs beats up from negative to 0 at the next bar) and
    // a remote join (start on the next bar after connecting mid-phrase).
    if st.armed && st.link_enabled && bar_index(link.beat_at(now, 4.0), 4.0) > st.armed_at_bar {
        // H4: if a run was somehow still playing when the armed-start fires, release its
        // sounding notes before `play` clears the registry (play emits no NoteOffs).
        if st.seq.is_playing() {
            st.seq.release_all(now, sink);
        }
        st.seq.play(now);
        st.armed = false;
        events.push(EngineEvent::Started { at_step: 0 });
    }

    if st.link_enabled {
        let beat = link.beat_at(now, 4.0);
        st.seq.sync_to_beat(beat, bpm, now);
    }

    // 3. Advance sequencer + clock. While following an external clock, `seq.tick` is
    //    clock-driven (its internal step_dur advance loop is suppressed — see
    //    Sequencer::set_clock_driven), so this call only runs the inaudible-note release +
    //    flush_due passes; the actual step advance happened above in the Tick handler. This
    //    is what guarantees no double-advance: the internal-timing path cannot also move a
    //    step while ClockIn is active.
    let advanced = st.seq.tick(now, sink).or(clock_advanced);
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
    run_engine_headless_clocked(set, link, sink, commands, vec![], total_micros, tick)
}

/// M10 T4: headless driver variant that ALSO feeds external MIDI clock-in messages at their
/// timestamps (simulating a connected clock source pushing `ClockInMsg`s into the engine's
/// clock-in channel). `clock_in` is a list of `(at_micros, ClockInMsg)`, delivered to the
/// engine on the first iteration where `now >= at_micros` (mirroring the `commands` queue).
/// `run_engine_headless` delegates here with an empty clock-in list, so existing callers are
/// unaffected.
pub fn run_engine_headless_clocked(
    set: Set,
    link: &mut dyn LinkClock,
    sink: &mut dyn MidiSink,
    commands: Vec<(u64, UiCommand)>,
    clock_in: Vec<(u64, ClockInMsg)>,
    total_micros: u64,
    tick: u64,
) -> Vec<EngineEvent> {
    let mut st = EngineState::new(set);
    let mut pending = commands;
    let mut pending_clock = clock_in;
    let mut events = Vec::new();
    let tick = tick.max(1);

    // Run now from 0 up to (but not including) total_micros so that events
    // scheduled exactly at total_micros (i.e. the first step of the *next* bar)
    // are not emitted. Commands timestamped at 0 fire on the first iteration.
    let mut now: u64 = 0;
    loop {
        // Drain due clock-in messages (at_micros <= now) into this step's batch, preserving
        // order, exactly as the real engine drains its clock-in channel each loop iteration.
        let mut clock_msgs: Vec<ClockInMsg> = Vec::new();
        let mut ci = 0;
        while ci < pending_clock.len() {
            if pending_clock[ci].0 <= now {
                clock_msgs.push(pending_clock.remove(ci).1);
            } else {
                ci += 1;
            }
        }
        if step_engine(
            &mut st,
            now,
            &mut pending,
            &mut clock_msgs,
            link,
            sink,
            &mut events,
        ) {
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

/// Port indices a CHANNEL message from `lane` targets, with the virtual mirror folded in.
/// Pure; UNIT-TESTED via the `send_lane` path.
///
/// Channel messages (NoteOn/NoteOff/ControlChange) route to the EMITTING lane's port
/// (`lane_to_port[lane]`) — NOT by channel — so two lanes sharing a MIDI channel on different
/// ports deliver independently. Non-channel messages fall back to `route_targets` (Clock →
/// clock-out ports). The mirror fold matches `route_targets_with_mirror` exactly.
fn route_targets_lane_with_mirror(
    msg: &crate::midi::message::MidiMessage,
    lane: usize,
    lane_to_port: &[usize],
    channel_to_port: &[Option<usize>; 16],
    clock_ports: &[usize],
    virtual_idx: Option<usize>,
    mirror_on: bool,
) -> Vec<usize> {
    use crate::midi::message::MidiMessage;
    let mut targets: Vec<usize> = match msg {
        MidiMessage::NoteOn { .. }
        | MidiMessage::NoteOff { .. }
        | MidiMessage::ControlChange { .. } => {
            lane_to_port.get(lane).copied().into_iter().collect()
        }
        _ => route_targets(msg, channel_to_port, clock_ports),
    };
    if mirror_on {
        if let Some(vidx) = virtual_idx {
            let mirrorable = matches!(
                msg,
                MidiMessage::NoteOn { .. }
                    | MidiMessage::NoteOff { .. }
                    | MidiMessage::ControlChange { .. }
                    | MidiMessage::Clock
            );
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
            // Wait up to one scan interval for a request, but wake IMMEDIATELY on
            // Quit/Reconnect. Teardown sends Quit then joins this thread; a plain
            // sleep here made that join (which runs on the timing loop) block for the
            // whole interval. recv_timeout returns the moment Quit arrives.
            match requests.recv_timeout(std::time::Duration::from_millis(WATCHER_SCAN_MS)) {
                Ok(PortRequest::Reconnect(idx)) => {
                    if let Some(c) = connected.get_mut(idx) {
                        *c = false;
                    }
                }
                Ok(PortRequest::Quit) => return,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        }
        first = false;

        // Drain any further queued requests without blocking (multiple Reconnects coalesce);
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

// ---------------------------------------------------------------------------
// MIDI clock-input watcher
// ---------------------------------------------------------------------------

/// Commands sent from the engine to the clock-input watcher.
enum ClockInCmd {
    /// (Re)connect to the given port match string, or disconnect when `None`.
    SetPort(Option<String>),
    /// Shut down the watcher thread.
    Quit,
}

/// Thread body for the clock-input watcher.
///
/// Owns the `MidirClockIn` connection. When `SetPort(Some(name))` arrives it
/// closes any existing connection and opens a new one; `SetPort(None)` closes
/// without reopening; `Quit` terminates. The callback of the open connection
/// forwards `ClockInMsg` values over `msg_tx` to the engine.
///
/// All MIDI input enumeration (`list_input_ports`) happens here — NEVER in the
/// timing loop.
fn run_clock_in_watcher(
    initial_port: Option<String>,
    msg_tx: crossbeam_channel::Sender<ClockInMsg>,
    cmds: crossbeam_channel::Receiver<ClockInCmd>,
) {
    // Attempt an initial connection if a port was specified.
    let mut _conn: Option<MidirClockIn> = initial_port
        .as_deref()
        .and_then(|name| connect_clock_in(name, msg_tx.clone()));

    for cmd in cmds {
        match cmd {
            ClockInCmd::SetPort(port_name) => {
                // Drop the old connection first (closing the port), then open the new one.
                _conn = None;
                _conn = port_name
                    .as_deref()
                    .and_then(|name| connect_clock_in(name, msg_tx.clone()));
            }
            ClockInCmd::Quit => break,
        }
    }
    // `_conn` drops here, closing any open input port.
}

/// Spawn the clock-input watcher thread.
///
/// Returns `(JoinHandle, Sender<ClockInCmd>, Receiver<ClockInMsg>)`.
/// The engine holds the `Sender` to send `SetPort`/`Quit` commands and holds the
/// `Receiver` to drain `ClockInMsg` values from the timing loop.
fn spawn_clock_in_watcher(
    initial_port: Option<String>,
) -> (
    std::thread::JoinHandle<()>,
    crossbeam_channel::Sender<ClockInCmd>,
    crossbeam_channel::Receiver<ClockInMsg>,
) {
    let (msg_tx, msg_rx) = crossbeam_channel::unbounded::<ClockInMsg>();
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<ClockInCmd>();
    let handle = std::thread::spawn(move || run_clock_in_watcher(initial_port, msg_tx, cmd_rx));
    (handle, cmd_tx, msg_rx)
}

// ---------------------------------------------------------------------------

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

        // Extract the initial clock-input port key BEFORE `set` is moved into EngineState.
        let initial_clock_in_key: Option<String> = set.clock_in_port.as_ref().map(|p| {
            if p.stable_key.is_empty() {
                p.name.clone()
            } else {
                p.stable_key.clone()
            }
        });

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

        // Spawn the clock-input watcher: opens a MidiInput connection to clock_in_port
        // (when set) and forwards ClockInMsg values over `clock_in_rx`. The engine loop
        // drains `clock_in_rx` non-blockingly each iteration and feeds the batch to
        // `step_engine`, which acts on it only while the tempo source is ClockIn (M10 T4).
        // Absent/None port → no connection, no panic (watcher starts idle).
        let (ci_watcher, ci_cmd_tx, clock_in_rx) =
            spawn_clock_in_watcher(initial_clock_in_key.clone());
        let mut ci_watcher = Some(ci_watcher);
        // Tracks the currently connected clock-in port key so SetSet reconnects only on change.
        let mut current_clock_in_key: Option<String> = initial_clock_in_key;

        // Emit initial (all-disconnected) per-lane DeviceStatus before the first tick;
        // the watcher's first scan (immediate, no initial sleep) flips present ports shortly.
        emit_lane_status(&port_sinks, &lane_to_port, &mut events);
        for ev in events.drain(..) {
            if evt_tx.send(ev).is_err() {
                let _ = request_tx.send(PortRequest::Quit);
                if let Some(w) = watcher.take() {
                    let _ = w.join();
                }
                let _ = ci_cmd_tx.send(ClockInCmd::Quit);
                if let Some(w) = ci_watcher.take() {
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
                                lane_to_port: &lane_to_port,
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
                            lane_to_port: &lane_to_port,
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
            // Drain the clock-input channel (NON-BLOCKING) into this iteration's batch. The
            // watcher's callback pushes ClockInMsg values from the MIDI input thread; we
            // collect them in arrival order and hand them to step_engine, which acts on them
            // only while following an external clock (M10 T4).
            let mut clock_msgs: Vec<ClockInMsg> = Vec::new();
            while let Ok(m) = clock_in_rx.try_recv() {
                clock_msgs.push(m);
            }

            let quit = {
                let mut fanout = PortFanoutSink {
                    ports: &mut port_sinks,
                    channel_to_port: &channel_to_port,
                    clock_ports: &clock_ports,
                    lane_to_port: &lane_to_port,
                    virtual_idx,
                    mirror_on: st.mirror_on,
                };
                step_engine(
                    &mut st,
                    now,
                    &mut pending,
                    &mut clock_msgs,
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
                        lane_to_port: &lane_to_port,
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
            // Intercept SetSet and SetClockInPort to (re)connect the clock-in watcher when the
            // selected clock-input port changes. The transport-source switch itself is applied
            // by `apply_command` when `pending` is processed; here we only manage the I/O side.
            while let Ok(cmd) = cmd_rx.try_recv() {
                let new_clock_key: Option<Option<String>> = match &cmd {
                    UiCommand::SetSet(set) => Some(set.clock_in_port.as_ref().map(|p| {
                        if p.stable_key.is_empty() {
                            p.name.clone()
                        } else {
                            p.stable_key.clone()
                        }
                    })),
                    UiCommand::SetClockInPort(port) => Some(port.as_ref().map(|p| {
                        if p.stable_key.is_empty() {
                            p.name.clone()
                        } else {
                            p.stable_key.clone()
                        }
                    })),
                    _ => None,
                };
                if let Some(new_key) = new_clock_key {
                    if new_key != current_clock_in_key {
                        current_clock_in_key = new_key.clone();
                        let _ = ci_cmd_tx.send(ClockInCmd::SetPort(new_key));
                    }
                }
                pending.push((now, cmd));
            }

            // Forward any events to the UI.
            if flush_events!() || quit {
                // Engine stopping (Quit or UI gone): shut both watchers down; don't leak them.
                let _ = request_tx.send(PortRequest::Quit);
                if let Some(w) = watcher.take() {
                    let _ = w.join();
                }
                let _ = ci_cmd_tx.send(ClockInCmd::Quit);
                if let Some(w) = ci_watcher.take() {
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

                if new_keys != port_keys {
                    // Release every sounding note through the OLD channel→port map and
                    // the OLD `port_sinks` BEFORE adopting the new maps, so each NoteOff
                    // reaches the live connection the note is actually sounding on.
                    // Adopting the new maps first would route these NoteOffs through the
                    // new channel map into the still-old port list, hanging hardware notes.
                    // Mirror is OFF for cleanup, so `virtual_idx` is unused here — pass None.
                    {
                        let mut fanout = PortFanoutSink {
                            ports: &mut port_sinks,
                            channel_to_port: &channel_to_port,
                            clock_ports: &clock_ports,
                            lane_to_port: &lane_to_port,
                            virtual_idx: None,
                            mirror_on: false,
                        };
                        st.seq.release_all(now, &mut fanout);
                    }
                }

                // Adopt the new channel/clock/lane maps AFTER releasing on the old
                // topology. This runs unconditionally: a route change can also move a
                // channel between existing ports without changing the key SET.
                channel_to_port = new_plan.channel_to_port;
                clock_ports = new_plan.clock_ports;
                lane_to_port = new_plan.lane_to_port;

                if new_keys != port_keys {
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
                        let _ = ci_cmd_tx.send(ClockInCmd::Quit);
                        if let Some(w) = ci_watcher.take() {
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
    /// Per-lane destination: `lane_to_port[lane]` is the port index a lane delivers to.
    /// Channel messages route by lane (not channel) via `send_lane`, so two lanes sharing a
    /// MIDI channel on different ports deliver independently. Clock keeps the `clock_ports`
    /// path through the unchanged `send`.
    lane_to_port: &'a [usize],
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

    /// Per-lane routing: a channel message is delivered to the EMITTING lane's port (via
    /// `lane_to_port`), so two lanes sharing a MIDI channel on different ports stay
    /// independent. The mirror is folded in identically to `send`.
    fn send_lane(&mut self, msg: crate::midi::message::MidiMessage, lane: usize, at_micros: u64) {
        for idx in route_targets_lane_with_mirror(
            &msg,
            lane,
            self.lane_to_port,
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
        assert_eq!(t.effective_bpm(None, None), bpm);
        // And effective_bpm ignores a link tempo when source is Manual.
        assert_eq!(t.effective_bpm(Some(140.0), None), bpm);
    }

    /// effective_bpm: after ToggleLink(true), transport.source == TempoSource::Link,
    /// so effective_bpm(Some(link_tempo)) returns link_tempo.
    #[test]
    fn effective_bpm_uses_link_tempo_when_source_is_link() {
        let mut t = Transport::new();
        t.manual_bpm = 120.0;
        t.source = TempoSource::Link;
        assert_eq!(t.effective_bpm(Some(140.0), None), 140.0);
        // Falls back to manual_bpm when link value absent.
        assert_eq!(t.effective_bpm(None, None), 120.0);
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
        let mut clock_msgs: Vec<ClockInMsg> = Vec::new();
        step_engine(
            &mut st,
            0,
            &mut pending,
            &mut clock_msgs,
            &mut link,
            &mut sink,
            &mut events,
        );

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
        let mut clock_msgs2: Vec<ClockInMsg> = Vec::new();
        step_engine(
            &mut st,
            1_000,
            &mut pending2,
            &mut clock_msgs2,
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

    // --- Remote Link transport (start/stop sync) ---------------------------

    /// Run one `step_engine` iteration at `now`, applying `cmds` (all timestamped at `now`).
    /// FakeLink's beat is static across `run_engine_headless`, so the remote-transport tests
    /// drive single steps and mutate the fake's beat/playing between them (like
    /// `link_play_starts_at_boundary`).
    fn step_at(
        st: &mut EngineState,
        link: &mut FakeLink,
        sink: &mut crate::midi::ports::RecordingSink,
        now: u64,
        cmds: Vec<UiCommand>,
    ) -> Vec<EngineEvent> {
        let mut pending: Vec<(u64, UiCommand)> = cmds.into_iter().map(|c| (now, c)).collect();
        let mut clock_msgs: Vec<ClockInMsg> = Vec::new();
        let mut events: Vec<EngineEvent> = Vec::new();
        step_engine(
            st,
            now,
            &mut pending,
            &mut clock_msgs,
            link,
            sink,
            &mut events,
        );
        events
    }

    /// Connect (enable Link) while the shared session is STOPPED: must not arm or start.
    #[test]
    fn link_connect_while_stopped_does_not_start() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        link.set_beat(2.0); // mid-timeline but session not playing

        let e0 = step_at(
            &mut st,
            &mut link,
            &mut sink,
            0,
            vec![UiCommand::ToggleLink(true)],
        );
        let mut e_rest = Vec::new();
        for t in [1_000u64, 2_000, 3_000] {
            e_rest.extend(step_at(&mut st, &mut link, &mut sink, t, vec![]));
        }

        assert!(!st.armed, "must not arm when session is stopped");
        assert!(
            !st.seq.is_playing(),
            "must not start when session is stopped"
        );
        assert!(
            !e0.iter()
                .chain(e_rest.iter())
                .any(|e| matches!(e, EngineEvent::Started { .. } | EngineEvent::Armed)),
            "no Armed/Started when connecting to a stopped session"
        );
        assert!(link.started_at.is_none(), "must not send a start request");
    }

    /// Connect while a peer is ALREADY playing: arm (no start request) and join on the NEXT bar.
    #[test]
    fn link_connect_while_playing_joins_next_bar() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        link.set_playing(true); // peer already playing
        link.set_beat(2.5); // mid-bar (bar 0)

        let e0 = step_at(
            &mut st,
            &mut link,
            &mut sink,
            0,
            vec![UiCommand::ToggleLink(true)],
        );
        assert!(st.armed, "joining a playing session must arm");
        assert!(!st.seq.is_playing(), "must not start mid-bar");
        assert!(
            e0.iter().any(|e| matches!(e, EngineEvent::Armed)),
            "Armed expected on join"
        );
        assert!(
            link.started_at.is_none(),
            "joining must NOT send a redundant start request"
        );

        // Still same bar → no start.
        link.set_beat(3.9);
        let e1 = step_at(&mut st, &mut link, &mut sink, 1_000, vec![]);
        assert!(!st.seq.is_playing());
        assert!(!e1.iter().any(|e| matches!(e, EngineEvent::Started { .. })));

        // Cross into the next shared bar (beat 4 → bar 1 > armed bar 0) → start once.
        link.set_beat(4.0);
        let e2 = step_at(&mut st, &mut link, &mut sink, 2_000, vec![]);
        assert!(st.seq.is_playing(), "must start on the next bar boundary");
        assert!(!st.armed);
        assert!(e2
            .iter()
            .any(|e| matches!(e, EngineEvent::Started { at_step: 0 })));
        assert!(link.started_at.is_none(), "join path never issues a start");
    }

    /// A remote start (peer presses play) schedules the sequencer exactly ONCE, at the next bar.
    #[test]
    fn link_remote_start_schedules_once() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        step_at(
            &mut st,
            &mut link,
            &mut sink,
            0,
            vec![UiCommand::ToggleLink(true)],
        );

        // Peer starts mid-bar.
        link.set_beat(2.5);
        link.set_playing(true);
        let e1 = step_at(&mut st, &mut link, &mut sink, 1_000, vec![]);
        assert!(st.armed, "remote start must arm");
        assert_eq!(
            e1.iter()
                .filter(|e| matches!(e, EngineEvent::Armed))
                .count(),
            1
        );

        // Same bar → no start, no re-arm.
        link.set_beat(3.0);
        let e2 = step_at(&mut st, &mut link, &mut sink, 2_000, vec![]);
        assert!(!e2
            .iter()
            .any(|e| matches!(e, EngineEvent::Started { .. } | EngineEvent::Armed)));

        // Next bar → start exactly once.
        link.set_beat(5.0);
        let e3 = step_at(&mut st, &mut link, &mut sink, 3_000, vec![]);
        assert_eq!(
            e3.iter()
                .filter(|e| matches!(e, EngineEvent::Started { .. }))
                .count(),
            1
        );

        // Keep advancing — no further Started/Armed (single schedule).
        link.set_beat(6.0);
        let e4 = step_at(&mut st, &mut link, &mut sink, 4_000, vec![]);
        assert!(!e4
            .iter()
            .any(|e| matches!(e, EngineEvent::Started { .. } | EngineEvent::Armed)));
        assert!(
            link.started_at.is_none(),
            "following a remote start must not issue our own start request"
        );
    }

    /// A remote stop halts the sequencer immediately and flushes (releases) sounding notes.
    #[test]
    fn link_remote_stop_stops_and_flushes() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        put_long_note(&mut set, 0, 0); // a sustained note on lane 0 step 0
        let mut st = EngineState::new(set);
        let mut sink = crate::midi::ports::RecordingSink::new();

        // Bootstrap a genuinely-playing sequencer with a sounding note.
        st.seq.play(0);
        st.seq.tick(0, &mut sink);
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { .. })),
            "precondition: a note must be sounding"
        );
        sink.events.clear();

        // We are following a playing Link session.
        let mut link = FakeLink::new();
        link.set_enabled(true);
        link.set_playing(true);
        link.set_beat(1.0);
        st.link_enabled = true;
        st.link_playing = true;

        // Peer stops.
        link.set_playing(false);
        let e = step_at(&mut st, &mut link, &mut sink, 1_000, vec![]);

        assert!(!st.seq.is_playing(), "remote stop must halt the sequencer");
        assert!(
            e.iter().any(|ev| matches!(ev, EngineEvent::Stopped)),
            "remote stop must emit Stopped"
        );
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOff { .. })),
            "remote stop must flush (release) sounding notes"
        );
    }

    /// H4: pressing Play (manual mode) while notes still sound must flush NoteOffs before the
    /// restart clears the sounding registry — otherwise the held notes hang on hardware.
    #[test]
    fn manual_play_while_playing_flushes_sounding_notes() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        put_long_note(&mut set, 0, 0); // sustained note on lane 0 step 0
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new(); // Link disabled → manual Play path
        let mut sink = crate::midi::ports::RecordingSink::new();

        // Bootstrap a genuinely-playing sequencer with a sounding note.
        st.seq.play(0);
        st.seq.tick(0, &mut sink);
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { .. })),
            "precondition: a note must be sounding"
        );
        sink.events.clear();

        // Restart via Play while the note still sounds: must emit NoteOff(s) (H4).
        step_at(&mut st, &mut link, &mut sink, 1_000, vec![UiCommand::Play]);
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOff { .. })),
            "restart while playing must release the previously sounding notes; got {:?}",
            sink.events
        );
    }

    /// H5: joining an already-playing Link session mid-song (session beat B > 0, no local
    /// request_start) must materialize the current step — pre-fix the re-anchor placed the
    /// step ~B*4*dur in the future so no note ever sounded.
    #[test]
    fn link_mid_session_join_materializes_steps() {
        use crate::midi::MidiMessage;
        let mut set = default_set();
        set.bpm = 120.0;
        put_long_note(&mut set, 0, 0); // note on step 0 → fires at steps 0, 16, 32, …
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();

        // A session that is ALREADY playing mid-song at beat 6.0 (bar 1, step 24).
        link.set_enabled(true);
        link.set_playing(true);
        link.set_tempo(120.0);
        link.set_beat(6.0);
        st.link_enabled = true;
        // link_playing left false → the running peer reads as stopped→playing and we arm.

        // First iteration: observe the peer playing → arm to join at the next bar.
        step_at(&mut st, &mut link, &mut sink, 0, vec![]);
        assert!(st.armed, "joining a running session must arm");

        // Cross into bar 2 (beat 8.0 → step 32). `now` is chosen to match the beat at
        // 120 BPM (beat 8 = 4 s = 4_000_000 µs) since FakeLink's beat is static.
        link.set_beat(8.0);
        step_at(&mut st, &mut link, &mut sink, 4_000_000, vec![]);
        assert!(st.seq.is_playing(), "armed-start must begin playback");
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { .. })),
            "a mid-session join must materialize the current step (notes must sound); got {:?}",
            sink.events
        );
    }

    /// Pressing local Play while the shared session is already playing sends NO redundant start.
    #[test]
    fn link_local_play_while_playing_no_redundant_start() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        link.set_playing(true); // peer already playing
        link.set_beat(2.0);

        step_at(
            &mut st,
            &mut link,
            &mut sink,
            0,
            vec![UiCommand::ToggleLink(true), UiCommand::Play],
        );
        assert!(
            link.started_at.is_none(),
            "local Play must not issue a start when the session already plays"
        );
        assert!(st.armed, "local Play while playing should arm to join");
    }

    /// A tempo change under Link preserves the phase-derived step (the beat→step mapping is
    /// tempo-independent), so changing BPM never jumps the playhead.
    #[test]
    fn link_tempo_change_preserves_phase() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        link.set_playing(true);
        link.set_beat(6.0); // bar 1
        step_at(
            &mut st,
            &mut link,
            &mut sink,
            0,
            vec![UiCommand::ToggleLink(true)],
        );
        // Cross into the next bar to start.
        link.set_beat(8.0); // bar 2 > armed bar 1
        step_at(&mut st, &mut link, &mut sink, 1_000, vec![]);
        assert!(st.seq.is_playing());
        let step_before = st.seq.current_step();

        // Peer changes tempo; beat position is unchanged.
        link.set_tempo(90.0);
        step_at(&mut st, &mut link, &mut sink, 2_000, vec![]);
        assert_eq!(
            st.seq.current_step(),
            step_before,
            "tempo change must not shift the phase-derived step"
        );
    }

    /// Forward beat correction jumps to the new step WITHOUT re-materializing (catching up)
    /// the steps it skipped over.
    #[test]
    fn link_forward_correction_skips_missed_steps_no_catchup() {
        use crate::midi::MidiMessage;
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0; // step_dur = 125_000 µs
                         // Drum hits on step 0 (note 36) and step 2 (note 38).
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        for (idx, note) in [(0usize, 36u8), (2, 38)] {
            steps[idx].push(DrumHit {
                note,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        set.lanes[0].pattern = Pattern {
            name: "fc".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };

        let mut st = EngineState::new(set);
        let mut sink = crate::midi::ports::RecordingSink::new();
        let mut link = FakeLink::new();
        link.set_enabled(true);
        link.set_playing(true);
        st.link_enabled = true;
        st.link_playing = true;

        // Start at beat 0 → materialize step 0 (note 36 fires).
        link.set_beat(0.0);
        st.seq.play(0);
        step_at(&mut st, &mut link, &mut sink, 0, vec![]);
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. })),
            "step 0 note must fire"
        );

        // Jump the beat forward past step 2 to step 3 (0.75 beats = step 3).
        link.set_beat(0.75);
        step_at(&mut st, &mut link, &mut sink, 100_000, vec![]);
        // Advance time well past step 2's would-be fire time.
        step_at(&mut st, &mut link, &mut sink, 400_000, vec![]);

        assert!(
            !sink
                .events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 38, .. })),
            "the skipped step-2 note must NOT fire (no catch-up burst)"
        );
    }

    /// With Link DISABLED, a playing peer state is ignored — behavior is unchanged.
    #[test]
    fn link_disabled_ignores_remote_playing() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        let mut sink = crate::midi::ports::RecordingSink::new();
        link.set_playing(true); // peer playing, but engine never enabled Link
        link.set_beat(4.0);

        let mut evs = Vec::new();
        for t in [0u64, 1_000, 2_000] {
            evs.extend(step_at(&mut st, &mut link, &mut sink, t, vec![]));
        }
        assert!(!st.armed, "disabled Link must not arm from remote state");
        assert!(!st.seq.is_playing(), "disabled Link must not start");
        assert!(!evs
            .iter()
            .any(|e| matches!(e, EngineEvent::Started { .. } | EngineEvent::Armed)));
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
                lane_to_port: &[],
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

    /// PER-LANE ROUTING (no hardware): two lanes share a MIDI channel but route to DIFFERENT
    /// ports. Each lane's notes must reach ONLY its own port. The old channel-keyed delivery
    /// (`channel_to_port[channel]`) collapsed both onto the LAST lane's port; `send_lane`
    /// routes by the emitting lane's `lane_to_port`, so the two lanes stay independent.
    #[test]
    fn fanout_send_lane_routes_same_channel_to_distinct_ports() {
        use crate::pattern::model::{LaneRoute, PortRef};
        // Two lanes, SAME channel 0, DIFFERENT ports.
        let mut set = default_set();
        set.lanes[0].route = Some(LaneRoute {
            port: PortRef {
                stable_key: "PortA".into(),
                name: "PortA".into(),
            },
            channel: 0,
            clock_out: false,
        });
        set.lanes[1].route = Some(LaneRoute {
            port: PortRef {
                stable_key: "PortB".into(),
                name: "PortB".into(),
            },
            channel: 0,
            clock_out: false,
        });
        // Only the two lanes under test (default_set's 3rd lane also defaults to channel 0,
        // which would otherwise claim channel_to_port[0]).
        set.lanes.truncate(2);
        let plan = build_route_plan(&set.lanes);
        // Sanity: lane_to_port keeps the lanes apart; the channel-keyed map collides on the
        // last lane (this collision is exactly the bug `send_lane` fixes).
        assert_eq!(plan.lane_to_port[0], 0, "lane 0 -> PortA (port index 0)");
        assert_eq!(plan.lane_to_port[1], 1, "lane 1 -> PortB (port index 1)");
        assert_eq!(
            plan.channel_to_port[0],
            Some(1),
            "channel-keyed map collides: ch0 -> last lane's port"
        );

        let probe_a = RecordingProbe::new();
        let probe_b = RecordingProbe::new();
        let mut ports = vec![
            PortSink {
                sink: Box::new(probe_a.clone()),
                connected: true,
                port_name: "PortA".into(),
            },
            PortSink {
                sink: Box::new(probe_b.clone()),
                connected: true,
                port_name: "PortB".into(),
            },
        ];
        {
            let mut fanout = PortFanoutSink {
                ports: &mut ports,
                channel_to_port: &plan.channel_to_port,
                clock_ports: &plan.clock_ports,
                lane_to_port: &plan.lane_to_port,
                virtual_idx: None,
                mirror_on: false,
            };
            fanout.send_lane(
                crate::midi::MidiMessage::NoteOn {
                    channel: 0,
                    note: 60,
                    vel: 100,
                },
                0,
                1,
            );
            fanout.send_lane(
                crate::midi::MidiMessage::NoteOn {
                    channel: 0,
                    note: 64,
                    vel: 100,
                },
                1,
                2,
            );
        }
        let a = probe_a.events.lock().unwrap();
        let b = probe_b.events.lock().unwrap();
        assert!(
            a.len() == 1
                && a.iter()
                    .any(|(_, m)| matches!(m, crate::midi::MidiMessage::NoteOn { note: 60, .. })),
            "PortA (lane 0) must receive ONLY note 60; got {a:?}"
        );
        assert!(
            b.len() == 1
                && b.iter()
                    .any(|(_, m)| matches!(m, crate::midi::MidiMessage::NoteOn { note: 64, .. })),
            "PortB (lane 1) must receive ONLY note 64; got {b:?}"
        );
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

    /// H1: an `UpdateLaneParams` command (the one the lane swing / clock-div handlers
    /// emit) must mirror swing + clock_div into the engine lane state, so the scheduler
    /// — which reads `lanes[i].swing` / `.clock_div` — picks up the edit immediately.
    #[test]
    fn update_lane_params_reaches_engine_lane_state() {
        let mut st = EngineState::new(default_set());
        // Precondition: lane 0 starts with no overrides.
        assert_eq!(st.seq.lane(0).unwrap().swing, None);
        assert_eq!(st.seq.lane(0).unwrap().clock_div, None);

        let mut events: Vec<EngineEvent> = Vec::new();
        apply_command(
            &mut st,
            UiCommand::UpdateLaneParams {
                lane: 0,
                swing: Some(0.7),
                clock_div: Some(4),
            },
            0,
            &mut FakeLink::default(),
            &mut RecordingSink::default(),
            &mut events,
        );

        let lane = st.seq.lane(0).unwrap();
        assert_eq!(lane.swing, Some(0.7), "swing must reach engine lane state");
        assert_eq!(
            lane.clock_div,
            Some(4),
            "clock_div must reach engine lane state"
        );
    }

    /// H2: a local Stop under enabled Link must publish the stop to Link (clearing the
    /// shared playing flag) AND reset the local `link_playing` latch, so the next Play
    /// re-issues `request_start` and the remote-follow transition check stays correct.
    #[test]
    fn stop_publishes_link_stop_and_clears_latch() {
        let mut st = EngineState::new(default_set());
        let mut link = FakeLink::new();
        link.set_enabled(true);
        let mut events: Vec<EngineEvent> = Vec::new();

        // Play under Link: arms, requests a start (marks the session playing) and
        // records our latch.
        apply_command(
            &mut st,
            UiCommand::Play,
            0,
            &mut link,
            &mut RecordingSink::default(),
            &mut events,
        );
        assert!(link.is_playing(), "Play under Link must start the session");
        assert!(
            st.link_playing,
            "Play must set the local link_playing latch"
        );

        // Stop must publish the stop to Link and clear the latch.
        apply_command(
            &mut st,
            UiCommand::Stop,
            1_000,
            &mut link,
            &mut RecordingSink::default(),
            &mut events,
        );
        assert!(
            !link.is_playing(),
            "Stop must publish the stop so peers follow"
        );
        assert!(
            !st.link_playing,
            "Stop must clear the local link_playing latch"
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

    // ---------------------------------------------------------------------------
    // M10 T4 — follow external MIDI clock (tick-driven advance, transport, loss,
    // source exclusivity). Driven through the headless harness with a fake clock-in
    // source pushing ClockInMsg values at virtual timestamps.
    // ---------------------------------------------------------------------------

    /// A set whose lane-0 drum pattern hits note 36 on EVERY step (so each step advance
    /// produces an observable NoteOn). Lanes 1/2 are emptied so only lane 0 sounds.
    fn clock_in_drum_set(bpm: f64) -> Set {
        use crate::pattern::model::{DrumHit, Pattern, PatternData};
        let mut set = default_set();
        set.bpm = bpm;
        let hit = DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        };
        let steps: Vec<Vec<DrumHit>> = (0..16).map(|_| vec![hit.clone()]).collect();
        set.lanes[0].pattern = Pattern {
            name: "ci".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        // Silence the other lanes so the test only sees lane-0 NoteOns.
        let empty = Pattern {
            name: "empty".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums((0..16).map(|_| Vec::new()).collect()),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        set.lanes[1].pattern = empty.clone();
        set.lanes[2].pattern = empty;
        set
    }

    fn note_on_count(sink: &RecordingSink) -> usize {
        sink.events
            .iter()
            .filter(|(_, m)| matches!(m, crate::midi::MidiMessage::NoteOn { vel, .. } if *vel > 0))
            .count()
    }

    /// 6 incoming ticks (one 16th) advance the sequencer EXACTLY one step — and the internal
    /// `step_dur` timer does NOT also advance it (no double-step). Drives ClockIn, plays from
    /// top, then feeds 6 ticks spread across virtual time; expects exactly ONE step's NoteOn.
    #[test]
    fn clock_in_six_ticks_advance_one_step() {
        let set = clock_in_drum_set(120.0); // internal step_dur = 125_000 µs
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // Tick interval at 120 BPM = 125_000/6 ≈ 20_833 µs. Place 6 ticks within the first
        // 125_000 µs (one internal step) so that, were the internal timer ALSO advancing, we
        // would see >1 step. Total run is short enough that internal timing alone would only
        // fire step 0 anyway — so to prove suppression we ALSO assert step count == 1 here and
        // rely on the dedicated no-double-advance test below for the long-run guarantee.
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        for i in 1..=6u64 {
            clock_in.push((i * 20_000, ClockInMsg::Tick));
        }
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            200_000,
            1_000,
        );

        // Exactly one step advance → exactly one lane-0 NoteOn.
        assert_eq!(
            note_on_count(&sink),
            1,
            "6 ticks must advance exactly ONE step (one NoteOn); got events {:?}",
            sink.events
        );
        // The advance produced a Playhead at step 0 (the first step materialized by the 6th tick).
        assert!(
            evs.iter()
                .any(|e| matches!(e, EngineEvent::Playhead { step: 0, .. })),
            "expected a Playhead for step 0; got {:?}",
            evs
        );
    }

    /// While ClockIn is active, the internal `step_dur` timer must NOT advance the sequencer —
    /// only ticks do. Run for many internal step-durations with ZERO ticks after Start: no step
    /// should fire (no NoteOn) beyond what ticks would drive (here: none).
    #[test]
    fn clock_in_internal_timer_does_not_advance() {
        let set = clock_in_drum_set(120.0); // internal step_dur = 125_000 µs
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // Start (play from top) but send NO ticks. Run for 10 internal steps' worth of time.
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            vec![(0, ClockInMsg::Start)],
            1_300_000, // > 10 * 125_000
            1_000,
        );

        assert_eq!(
            note_on_count(&sink),
            0,
            "internal timer must NOT advance steps while ClockIn active (no ticks → no NoteOn); \
             got {:?}",
            sink.events
        );
        // No spurious Playhead step advances either (only the initial step-0 emit is gated on
        // play; with clock-driven we never call play's step 0 via internal tick).
        let advances = evs
            .iter()
            .filter(|e| matches!(e, EngineEvent::Playhead { .. }))
            .count();
        assert!(
            advances <= 1,
            "at most the initial playhead; got {advances} Playhead events: {evs:?}"
        );
    }

    /// `Start` plays from the TOP (origin): after Start + 6 ticks, current_step is the first
    /// step (0) — a fresh run, not a resume.
    #[test]
    fn clock_in_start_plays_from_top() {
        let set = clock_in_drum_set(120.0);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        for i in 1..=6u64 {
            clock_in.push((i * 20_000, ClockInMsg::Tick));
        }
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            200_000,
            1_000,
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, EngineEvent::Started { at_step: 0 })),
            "Start must emit Started{{at_step:0}} (play from top); got {evs:?}"
        );
        assert!(
            evs.iter()
                .any(|e| matches!(e, EngineEvent::Playhead { step: 0, .. })),
            "first materialized step after Start is step 0; got {evs:?}"
        );
    }

    /// `Continue` resumes at the CURRENT position (does not rewind). Start + advance a few
    /// steps, Stop, then Continue: playback resumes from where it was, not from step 0.
    #[test]
    fn clock_in_continue_resumes_position() {
        let set = clock_in_drum_set(120.0);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // Start, advance 3 steps (18 ticks), Stop, then Continue + 1 more step (6 ticks).
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        let mut t = 10_000u64;
        for _ in 0..18 {
            clock_in.push((t, ClockInMsg::Tick));
            t += 20_000;
        }
        // Stop after 3 steps (current_step should be 2, next_step 3).
        clock_in.push((t, ClockInMsg::Stop));
        t += 20_000;
        // Continue then 6 more ticks → one more step. It must be step 3 (resume), not step 0.
        clock_in.push((t, ClockInMsg::Continue));
        for _ in 0..6 {
            t += 20_000;
            clock_in.push((t, ClockInMsg::Tick));
        }
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            t + 100_000,
            1_000,
        );

        // The LAST Playhead step must be 3 (resumed forward), proving no rewind to 0.
        let last_step = evs
            .iter()
            .rev()
            .find_map(|e| match e {
                EngineEvent::Playhead { step, .. } => Some(*step),
                _ => None,
            })
            .expect("at least one Playhead");
        assert_eq!(
            last_step, 3,
            "Continue must resume at the next position (step 3), not rewind; got {evs:?}"
        );
    }

    /// `Stop` halts playback AND releases all sounding notes via the existing stop path —
    /// the registry ends empty and every NoteOn is matched by a NoteOff.
    #[test]
    fn clock_in_stop_releases_all_notes() {
        use crate::midi::MidiMessage;
        use crate::pattern::model::{MelodicNote, MelodicStep, Pattern, PatternData};

        // Use a melodic lane holding a LONG note so a NoteOn is sounding when Stop arrives.
        let mut set = default_set();
        set.bpm = 120.0;
        let mut steps = vec![MelodicStep::default(); 16];
        steps[0] = MelodicStep::from(vec![MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 16.0, // holds across the whole pattern
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        set.lanes[0].pattern = Pattern {
            name: "held".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        // Start, 6 ticks (materialize step 0 → NoteOn sounding), then Stop.
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        let mut t = 10_000u64;
        for _ in 0..6 {
            clock_in.push((t, ClockInMsg::Tick));
            t += 20_000;
        }
        clock_in.push((t, ClockInMsg::Stop));
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            t + 100_000,
            1_000,
        );

        // A NoteOn fired, and Stop emitted a Stopped event.
        assert!(
            note_on_count(&sink) >= 1,
            "a held NoteOn must have sounded before Stop; got {:?}",
            sink.events
        );
        assert!(
            evs.iter().any(|e| matches!(e, EngineEvent::Stopped)),
            "Stop must emit Stopped; got {evs:?}"
        );
        // Note-safety: net per-(channel,note) balance is zero (every NoteOn released).
        use std::collections::HashMap;
        let mut net: HashMap<(u8, u8), i32> = HashMap::new();
        for (_, m) in &sink.events {
            match m {
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
            "Stop must release all notes (no hung notes); leftover {hung:?}; events {:?}",
            sink.events
        );
    }

    /// `SongPosition` is parsed + stored but NOT acted upon: position is unchanged (deferred).
    #[test]
    fn clock_in_song_position_does_not_reposition() {
        let set = clock_in_drum_set(120.0);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        // Start, advance 2 steps (12 ticks → steps 0,1), send an SPP, advance 1 more step
        // (6 ticks → step 2). The SPP must NOT move the playhead — steps stay monotonic.
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        let mut t = 10_000u64;
        for _ in 0..12 {
            clock_in.push((t, ClockInMsg::Tick));
            t += 20_000;
        }
        clock_in.push((t, ClockInMsg::SongPosition(64))); // far-away beat; must be ignored
        for _ in 0..6 {
            t += 20_000;
            clock_in.push((t, ClockInMsg::Tick));
        }
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            t + 100_000,
            1_000,
        );
        // Steps advanced 0,1,2 then 3 — the SPP(64) did NOT jump the playhead to beat 64.
        let last_step = evs
            .iter()
            .rev()
            .find_map(|e| match e {
                EngineEvent::Playhead { step, .. } => Some(*step),
                _ => None,
            })
            .expect("a Playhead");
        assert_eq!(
            last_step, 2,
            "SongPosition must NOT reposition (step stays monotonic at 2, not 64*4); got {evs:?}"
        );
    }

    /// No tick within the loss timeout → engine treats it as Stop: halts, releases all notes,
    /// and emits ClockInStatus{locked:false}.
    #[test]
    fn clock_in_loss_timeout_stops_and_releases() {
        use crate::midi::MidiMessage;
        use crate::pattern::model::{MelodicNote, MelodicStep, Pattern, PatternData};

        let mut set = default_set();
        set.bpm = 120.0;
        let mut steps = vec![MelodicStep::default(); 16];
        steps[0] = MelodicStep::from(vec![MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 16.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]);
        set.lanes[0].pattern = Pattern {
            name: "held".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };

        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        // Start + 6 ticks (a NoteOn sounds), then SILENCE. The loss timeout (~one beat) elapses
        // and the engine stops + releases. Run well past the timeout.
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        let mut t = 10_000u64;
        for _ in 0..6 {
            clock_in.push((t, ClockInMsg::Tick));
            t += 20_000;
        }
        // Last tick around t≈130_000. Loss timeout ~500_000 µs → lost by ~700_000. Run to 2 s.
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            2_000_000,
            1_000,
        );

        assert!(
            evs.iter()
                .any(|e| matches!(e, EngineEvent::ClockInStatus { locked: false, .. })),
            "loss must emit ClockInStatus{{locked:false}}; got {evs:?}"
        );
        // Note-safety: the held note is released by the loss-stop (no hung note).
        use std::collections::HashMap;
        let mut net: HashMap<(u8, u8), i32> = HashMap::new();
        for (_, m) in &sink.events {
            match m {
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
            "loss timeout must release all notes; leftover {hung:?}; events {:?}",
            sink.events
        );
    }

    /// A clean external MIDI Stop must NOT trigger the loss-timeout path afterwards.
    /// Sequence: SetClockInPort → Start → 6 Ticks (lock acquired) → Stop → silence for >500 ms.
    /// Expected: NO spurious ClockInStatus{locked:false} emitted after the Stop.
    #[test]
    fn clock_in_clean_stop_no_spurious_loss_event() {
        let set = clock_in_drum_set(120.0);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        // Build clock-in stream: Start, 6 ticks to acquire lock, then Stop. After Stop: silence.
        let mut clock_in = vec![(0, ClockInMsg::Start)];
        let mut t = 10_000u64;
        for _ in 0..6 {
            clock_in.push((t, ClockInMsg::Tick));
            t += 20_000;
        }
        // Stop arrives shortly after last tick (~130 000 µs). Silence follows for >500 ms.
        let stop_at = t + 5_000;
        clock_in.push((stop_at, ClockInMsg::Stop));

        // Run well past the loss timeout (500 000 µs) with no further ticks.
        let evs = run_engine_headless_clocked(
            set,
            &mut link,
            &mut sink,
            vec![(0, UiCommand::SetClockInPort(Some(test_port_ref())))],
            clock_in,
            2_000_000,
            1_000,
        );

        // The Stop itself may emit Stopped; what must NOT appear is a post-timeout
        // ClockInStatus{locked:false} (spurious loss event).
        //
        // Strategy: collect every ClockInStatus{locked:false} that appears AFTER the Stop
        // timestamp. A spurious loss would show up here; a clean stop must produce none.
        let stop_ts_approx = stop_at; // events are ordered; find index of Stopped first.
                                      // We only care that no ClockInStatus{locked:false} appears more than once
                                      // (the loss guard would emit it; a normal Stop does not emit it at all).
        let loss_events: Vec<_> = evs
            .iter()
            .filter(|e| matches!(e, EngineEvent::ClockInStatus { locked: false, .. }))
            .collect();
        assert!(
            loss_events.is_empty(),
            "clean external Stop must not emit ClockInStatus{{locked:false}}; got {loss_events:?}\nall events: {evs:?}"
        );
        let _ = stop_ts_approx; // suppress unused warning
    }

    /// Switching INTO ClockIn disables Link (3-way exclusivity); switching OUT (clear port)
    /// reverts to Manual. Verified through apply_command on EngineState.
    #[test]
    fn clock_in_switch_releases_other_sources() {
        let set = default_set();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut events = Vec::new();

        // Start in Link.
        apply_command(
            &mut st,
            UiCommand::ToggleLink(true),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert_eq!(st.transport.source, TempoSource::Link);
        assert!(st.link_enabled && link.enabled());

        // Switch to ClockIn → Link must be disabled, source ClockIn, clock-driven engaged.
        apply_command(
            &mut st,
            UiCommand::SetClockInPort(Some(test_port_ref())),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert_eq!(st.transport.source, TempoSource::ClockIn);
        assert!(
            !st.link_enabled,
            "Link must be disabled when entering ClockIn"
        );
        assert!(
            !link.enabled(),
            "Link clock must be disabled when entering ClockIn"
        );

        // Clear the port → revert to Manual (Link stays off).
        apply_command(
            &mut st,
            UiCommand::SetClockInPort(None),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert!(
            matches!(st.transport.source, TempoSource::Manual(_)),
            "clearing the clock-in port reverts to Manual; got {:?}",
            st.transport.source
        );
    }

    /// Undo/redo (SyncLanes) must flag a port re-plan when a lane's route changed, so MIDI
    /// follows the restored device/port instead of staying on the post-change one.
    #[test]
    fn sync_lanes_replans_when_a_lane_route_changed() {
        let set = default_set();
        let lanes = set.lanes.clone();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut events = Vec::new();

        // Restore a lane set where lane 0's route changed (different channel).
        let mut restored = lanes.clone();
        let mut r = restored[0].effective_route();
        r.channel = (r.channel + 1) % 16;
        restored[0].route = Some(r);

        st.route_dirty = false;
        apply_command(
            &mut st,
            UiCommand::SyncLanes(restored),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert!(
            st.route_dirty,
            "a route change via SyncLanes must flag a port re-plan"
        );
    }

    /// A SyncLanes that changes no routing (e.g. undoing a step edit) must NOT re-plan ports
    /// — that would needlessly cut sounding notes.
    #[test]
    fn sync_lanes_no_replan_when_routes_unchanged() {
        let set = default_set();
        let lanes = set.lanes.clone();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut events = Vec::new();

        st.route_dirty = false;
        apply_command(
            &mut st,
            UiCommand::SyncLanes(lanes),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert!(
            !st.route_dirty,
            "an unchanged SyncLanes must not trigger a re-plan"
        );
    }

    /// An explicit manual BPM disables Link (tempo sources are mutually exclusive).
    #[test]
    fn set_bpm_disables_link() {
        let set = default_set();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut events = Vec::new();

        apply_command(
            &mut st,
            UiCommand::ToggleLink(true),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert!(st.link_enabled && link.enabled());

        apply_command(
            &mut st,
            UiCommand::SetBpm(140.0),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert!(!st.link_enabled, "manual BPM must disable Link");
        assert!(!link.enabled(), "Link clock must be turned off");
        assert!(matches!(st.transport.source, TempoSource::Manual(_)));
    }

    /// Undo/redo's RestoreBpm refreshes the stored BPM but must NOT change the active tempo
    /// source — undoing an edit while Link is on must not drop the user out of Link.
    #[test]
    fn restore_bpm_preserves_active_source() {
        let set = default_set();
        let mut st = EngineState::new(set);
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();
        let mut events = Vec::new();

        apply_command(
            &mut st,
            UiCommand::ToggleLink(true),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert_eq!(st.transport.source, TempoSource::Link);

        apply_command(
            &mut st,
            UiCommand::RestoreBpm(140.0),
            0,
            &mut link,
            &mut sink,
            &mut events,
        );
        assert_eq!(
            st.transport.source,
            TempoSource::Link,
            "RestoreBpm must not change the tempo source"
        );
        assert!(st.link_enabled, "RestoreBpm must not disable Link");
        assert_eq!(
            st.transport.manual_bpm, 140.0,
            "stored manual BPM is still refreshed for when Link is later left"
        );
    }

    /// A `PortRef` for clock-in tests (the headless engine never opens it).
    fn test_port_ref() -> crate::pattern::model::PortRef {
        crate::pattern::model::PortRef {
            name: "TestClock".into(),
            stable_key: "TestClock".into(),
        }
    }
}
