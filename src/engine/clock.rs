//! 24 PPQN MIDI clock generation. Emits `Start`/`Stop` on transport changes and a
//! steady stream of `Clock` ticks derived from the active tempo.

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

    /// Begin clocking: emit `Start` and schedule the first tick at `at_micros`.
    pub fn start(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        self.running = true;
        self.next_tick_at = at_micros;
        sink.send(MidiMessage::Start, at_micros);
    }

    /// Stop clocking: emit `Stop`.
    pub fn stop(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        self.running = false;
        sink.send(MidiMessage::Stop, at_micros);
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
    fn start_emits_start_message() {
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0, &mut sink);
        assert_eq!(sink.events.first().map(|(_, m)| m.clone()), Some(MidiMessage::Start));
    }

    #[test]
    fn stop_emits_stop_message() {
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0, &mut sink);
        gen.stop(1_000, &mut sink);
        assert_eq!(sink.events.last().map(|(_, m)| m.clone()), Some(MidiMessage::Stop));
    }

    #[test]
    fn emits_24_clocks_over_one_quarter_note() {
        let interval = clock_interval_micros(120.0); // 20_833
        let quarter = 60_000_000u64 / 120; // 500_000 µs
        let mut gen = ClockGen::new();
        let mut sink = RecordingSink::new();
        gen.start(0, &mut sink);

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
