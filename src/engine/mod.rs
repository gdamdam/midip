//! Engine thread entry point and the deterministic headless driver.
//!
//! The real engine (`spawn_engine`) runs `step_engine` in a loop on a monotonic clock;
//! the test driver (`run_engine_headless`) runs the *same* `step_engine` over a virtual
//! clock. Only the headless driver is unit-tested — the threaded one is not deterministic.

pub mod clock;
pub mod scheduler;
pub mod transport;

use crate::link::LinkClock;
use crate::midi::ports::MidiSink;
use crate::pattern::model::{Pattern, Set};
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
            st.clock.start(now, sink);
            if link.enabled() {
                link.request_start(now, 4.0); // quantized start: align to next bar
            }
        }
        UiCommand::Stop => {
            st.seq.stop(now, sink);
            st.clock.stop(now, sink);
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
        UiCommand::Quit => return true,
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

    // 2. Tempo source resolution.
    let bpm = if st.link_enabled {
        let beat = link.beat_at(now, 4.0);
        let tempo = link.tempo();
        st.seq.sync_to_beat(beat, tempo);
        tempo
    } else {
        st.transport.manual_bpm
    };

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

/// Spawn the real engine on its own thread, driven by a monotonic clock. NOT unit-tested
/// (non-deterministic timing); shares `step_engine` with the headless driver.
pub fn spawn_engine(
    set: Set,
    mut link: Box<dyn LinkClock>,
    mut sinks: Vec<Box<dyn MidiSink>>,
) -> EngineHandle {
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<UiCommand>();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded::<EngineEvent>();

    let join = std::thread::spawn(move || {
        let mut st = EngineState::new(set);
        let mut pending: Vec<(u64, UiCommand)> = Vec::new();
        let mut events: Vec<EngineEvent> = Vec::new();
        let start = std::time::Instant::now();

        loop {
            let now = start.elapsed().as_micros() as u64;

            // Drain channel into the pending queue (timestamped at `now`).
            while let Ok(cmd) = cmd_rx.try_recv() {
                pending.push((now, cmd));
            }

            let mut fanout = FanoutSink { sinks: &mut sinks };
            let quit = step_engine(&mut st, now, &mut pending, link.as_mut(), &mut fanout, &mut events);

            // Forward any events to the UI; drop on disconnect.
            for ev in events.drain(..) {
                if evt_tx.send(ev).is_err() {
                    return;
                }
            }
            if quit {
                return;
            }

            // ~1 ms loop; cheap and keeps timing tight enough for 24 PPQN.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    EngineHandle { tx: cmd_tx, rx: evt_rx, join }
}

/// Fans a single `send` out to every wrapped sink (used by the real engine to drive
/// multiple hardware ports + clock from one `step_engine` call).
struct FanoutSink<'a> {
    sinks: &'a mut Vec<Box<dyn MidiSink>>,
}

impl<'a> MidiSink for FanoutSink<'a> {
    fn send(&mut self, msg: crate::midi::message::MidiMessage, at_micros: u64) {
        for s in self.sinks.iter_mut() {
            s.send(msg.clone(), at_micros);
        }
    }
}
