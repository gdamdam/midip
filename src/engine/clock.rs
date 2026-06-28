//! 24 PPQN MIDI clock generation. While running, emits a steady stream of `Clock` ticks
//! derived from the active tempo. It deliberately does NOT send realtime transport
//! messages (Start/Stop/Continue): those start/stop an external device's OWN internal
//! sequencer, making it play its stored pattern. midip is the sequencer — it drives the
//! device's sounds via note messages and only syncs the device's tempo via Clock.

use crate::engine::scheduler::PPQN;
use crate::midi::ports::MidiSink;
use crate::midi::MidiMessage;

/// Microseconds between consecutive 24-PPQN clock ticks at `bpm`.
pub fn clock_interval_micros(bpm: f64) -> u64 {
    (60_000_000.0 / (bpm * PPQN as f64)).round() as u64
}

/// Generates MIDI clock ticks on demand. `tick` emits every `Clock` whose scheduled
/// time has passed since `start`, recomputing the interval each call so tempo changes
/// take effect immediately.
pub struct ClockGen {
    running: bool,
    /// Absolute time of the next clock tick to emit.
    next_tick_at: u64,
}

impl ClockGen {
    pub fn new() -> Self {
        ClockGen {
            running: false,
            next_tick_at: 0,
        }
    }

    /// Begin clocking: schedule the first tick at `at_micros`. Does NOT emit a realtime
    /// `Start` — that would run the receiving device's internal sequencer.
    pub fn start(&mut self, at_micros: u64) {
        self.running = true;
        self.next_tick_at = at_micros;
    }

    /// Stop clocking. Does NOT emit a realtime `Stop` (which would stop the device's
    /// internal sequencer); hanging notes are released by the sequencer's own stop path.
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// Emit every `Clock` whose scheduled time is <= `now_micros`.
    /// The boundary condition is inclusive: a tick scheduled exactly at `now_micros`
    /// is emitted, ensuring no gaps and no double-emission across successive calls.
    pub fn tick(&mut self, now_micros: u64, bpm: f64, sink: &mut dyn MidiSink) {
        if !self.running {
            return;
        }
        let interval = clock_interval_micros(bpm).max(1);
        while self.next_tick_at <= now_micros {
            sink.send(MidiMessage::Clock, self.next_tick_at);
            self.next_tick_at += interval;
        }
    }
}

impl Default for ClockGen {
    fn default() -> Self {
        ClockGen::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::scheduler::PPQN;
    use crate::midi::ports::RecordingSink;
    use crate::midi::MidiMessage;

    #[test]
    fn interval_at_120_bpm() {
        // 60_000_000 / (120 * 24) = 20_833.33 -> 20_833 (rounded).
        assert_eq!(clock_interval_micros(120.0), 20_833);
    }

    #[test]
    fn start_and_stop_never_emit_transport_messages() {
        // midip is the sequencer: it drives the device's SOUNDS via notes and syncs
        // TEMPO via Clock, but must NEVER send realtime Start/Stop/Continue — those run
        // the device's own internal pattern. Regression for the "device plays its stored
        // pattern when I press space" bug.
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0);
        gen.tick(20_833, 120.0, &mut sink); // some Clock ticks while running
        gen.stop();
        assert!(
            !sink.events.is_empty(),
            "expected Clock ticks while running"
        );
        assert!(
            sink.events.iter().all(|(_, m)| *m == MidiMessage::Clock),
            "only Clock may be sent; found a transport/realtime message"
        );
    }

    #[test]
    fn stop_halts_further_clock_ticks() {
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0);
        gen.stop();
        gen.tick(1_000_000, 120.0, &mut sink);
        assert!(sink.events.is_empty());
    }

    #[test]
    fn emits_24_clocks_over_one_quarter_note() {
        let interval = clock_interval_micros(120.0); // 20_833
        let quarter = 60_000_000u64 / 120; // 500_000 µs
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0);

        let mut now = 0u64;
        while now < quarter {
            gen.tick(now, 120.0, &mut sink);
            now += interval / 4; // tick finer than the clock interval
        }

        let clocks = sink
            .events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::Clock)
            .count();
        // Exactly PPQN ticks fit in one quarter note.
        assert_eq!(clocks, PPQN as usize);
    }
}
