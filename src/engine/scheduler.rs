//! Step sequencer: pure timing math (this task) plus the stateful `Sequencer`
//! (Task 9). All time is `u64` microseconds on a monotonic timeline.

use crate::midi::MidiMessage;

/// Pulses Per Quarter Note for the MIDI clock.
pub const PPQN: u64 = 24;

/// Duration of one 16th-note step in microseconds at `bpm`.
/// A quarter note is `60_000_000 / bpm` µs; a 16th is a quarter of that.
pub fn step_dur_micros(bpm: f64) -> u64 {
    (60_000_000.0 / (bpm * 4.0)).round() as u64
}

/// Swing offset for `step_index` given a `swing` ratio (0.5 = straight) and the
/// straight step duration. Even (down-beat) steps are unshifted; odd (off-beat)
/// steps are delayed by `(swing - 0.5) * step_dur * 2`. Signed so off-steps can be
/// pulled earlier if `swing < 0.5`.
pub fn swing_offset_micros(step_index: usize, swing: f32, step_dur: u64) -> i64 {
    if step_index % 2 == 0 {
        0
    } else {
        ((swing as f64 - 0.5) * step_dur as f64 * 2.0).round() as i64
    }
}

/// Note length in microseconds: `round(len * step_dur)`.
pub fn note_len_micros(len: f32, step_dur: u64) -> u64 {
    (len as f64 * step_dur as f64).round() as u64
}

/// A MIDI message scheduled for a specific lane at a specific monotonic time.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub at_micros: u64,
    pub lane: usize,
    pub msg: MidiMessage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_dur_at_120_bpm_is_125000_micros() {
        // 60_000_000 / (120 * 4) = 125_000
        assert_eq!(step_dur_micros(120.0), 125_000);
    }

    #[test]
    fn step_dur_at_60_bpm_is_250000_micros() {
        assert_eq!(step_dur_micros(60.0), 250_000);
    }

    #[test]
    fn swing_even_steps_have_zero_offset() {
        let dur = step_dur_micros(120.0);
        assert_eq!(swing_offset_micros(0, 0.6, dur), 0);
        assert_eq!(swing_offset_micros(2, 0.6, dur), 0);
        assert_eq!(swing_offset_micros(4, 0.6, dur), 0);
    }

    #[test]
    fn swing_half_is_zero_on_odd_steps() {
        let dur = step_dur_micros(120.0);
        assert_eq!(swing_offset_micros(1, 0.5, dur), 0);
        assert_eq!(swing_offset_micros(3, 0.5, dur), 0);
    }

    #[test]
    fn swing_above_half_delays_odd_steps() {
        let dur = step_dur_micros(120.0); // 125_000
        // (0.6 - 0.5) * 125_000 * 2 = 25_000
        assert_eq!(swing_offset_micros(1, 0.6, dur), 25_000);
        assert_eq!(swing_offset_micros(3, 0.6, dur), 25_000);
        assert!(swing_offset_micros(1, 0.6, dur) > 0);
    }

    #[test]
    fn note_len_micros_rounds_fractional_steps() {
        // 0.5 * 125_000 = 62_500
        assert_eq!(note_len_micros(0.5, 125_000), 62_500);
        // 1.5 * 125_000 = 187_500
        assert_eq!(note_len_micros(1.5, 125_000), 187_500);
        // rounding: 0.1 * 125_000 = 12_500
        assert_eq!(note_len_micros(0.1, 125_000), 12_500);
    }

    #[test]
    fn ppqn_is_24() {
        assert_eq!(PPQN, 24);
    }

    #[test]
    fn scheduled_event_holds_time_lane_msg() {
        let ev = ScheduledEvent {
            at_micros: 1000,
            lane: 2,
            msg: MidiMessage::Clock,
        };
        assert_eq!(ev.at_micros, 1000);
        assert_eq!(ev.lane, 2);
        assert_eq!(ev.msg, MidiMessage::Clock);
    }
}
