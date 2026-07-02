//! Pure MIDI clock-input core: realtime-message parser + tick accumulator.
//!
//! **No I/O, no wall-clock reads.** Every time value is passed in as `now_micros: u64`.
//! This module is entirely deterministic and has no side-effects beyond mutating `ClockInState`.
//!
//! # Parser signature choice
//!
//! `parse_realtime(status: u8, data: Option<(u8, u8)>) -> Option<ClockInMsg>`
//!
//! - Single-byte realtime messages (`0xF8`, `0xFA`, `0xFB`, `0xFC`) pass `data: None`.
//! - Song Position Pointer (`0xF2`) passes `data: Some((lsb, msb))` — the caller already
//!   assembled the two data bytes (standard MIDI framing layer responsibility).
//! - Returns `None` for unrecognised status bytes; callers may treat `None` as `Other` and
//!   ignore it.
//!
//! # Smoothing approach
//!
//! `ClockInState` holds a fixed-capacity ring buffer of recent inter-tick intervals.
//! `smoothed_bpm` computes a simple arithmetic mean of all buffered intervals and converts:
//!   `bpm = 60_000_000.0 / (mean_interval_micros * 24.0)`
//! Returns `None` until the ring is full (enough samples for a stable estimate).

/// A decoded MIDI realtime / system-common message relevant to clock input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockInMsg {
    /// `0xF8` — MIDI Timing Clock (24 PPQN).
    Tick,
    /// `0xFA` — MIDI Start.
    Start,
    /// `0xFB` — MIDI Continue.
    Continue,
    /// `0xFC` — MIDI Stop.
    Stop,
    /// `0xF2` — Song Position Pointer (14-bit beat count).
    SongPosition(u16),
    /// Any other / unhandled byte.
    Other,
}

/// Parse a single MIDI realtime or system-common byte into a [`ClockInMsg`].
///
/// - Single-byte messages (`0xF8`, `0xFA`, `0xFB`, `0xFC`): pass `data: None`.
/// - Song Position Pointer (`0xF2`): pass `data: Some((lsb, msb))` where `lsb` and `msb`
///   are the two 7-bit data bytes that follow the status byte in the MIDI stream.
///   The 14-bit beat value is `(msb as u16) << 7 | (lsb as u16)`.
/// - Returns `None` for unrecognised status bytes.
pub fn parse_realtime(status: u8, data: Option<(u8, u8)>) -> Option<ClockInMsg> {
    match status {
        0xF8 => Some(ClockInMsg::Tick),
        0xFA => Some(ClockInMsg::Start),
        0xFB => Some(ClockInMsg::Continue),
        0xFC => Some(ClockInMsg::Stop),
        0xF2 => {
            let (lsb, msb) = data?;
            let position = (msb as u16) << 7 | (lsb as u16);
            Some(ClockInMsg::SongPosition(position))
        }
        _ => None,
    }
}

/// Number of ticks per 16th-note step at 24 PPQN (24 / 4 = 6).
const TICKS_PER_STEP: u64 = 6;

/// Capacity of the inter-tick interval ring buffer. 48 samples = 2 quarter-notes at 24 PPQN.
const RING_CAPACITY: usize = 48;

/// Minimum number of samples required before `smoothed_bpm` returns `Some`.
/// Using half the ring capacity gives a reasonable early estimate while still filtering noise.
const MIN_SAMPLES: usize = RING_CAPACITY / 2;

/// Pure MIDI clock-input accumulator.
///
/// Tracks tick count, records inter-tick intervals in a ring buffer for tempo smoothing,
/// and signals step advances (every 6 ticks = one 16th-note at 24 PPQN).
///
/// All time is passed in by the caller (`now_micros: u64`). No system clock is read here.
pub struct ClockInState {
    /// Total tick count since the last reset / construction.
    tick_count: u64,
    /// Timestamp of the most recent tick, if any.
    last_tick_micros: Option<u64>,
    /// Fixed-capacity ring buffer of recent inter-tick intervals (in microseconds).
    recent_intervals: [u64; RING_CAPACITY],
    /// Write index into `recent_intervals` (wraps modulo `RING_CAPACITY`).
    ring_head: usize,
    /// How many valid samples are currently in the ring (saturates at `RING_CAPACITY`).
    ring_count: usize,
}

impl ClockInState {
    /// Create a new, empty accumulator.
    pub fn new() -> Self {
        Self {
            tick_count: 0,
            last_tick_micros: None,
            recent_intervals: [0u64; RING_CAPACITY],
            ring_head: 0,
            ring_count: 0,
        }
    }

    /// Record a tick at `now_micros`.
    ///
    /// Appends the inter-tick interval to the ring buffer (when a previous tick exists),
    /// increments the internal tick counter, and returns `true` exactly once per 6 ticks
    /// to signal a 16th-note step advance.
    ///
    /// H3: the boundary is evaluated on the PRE-increment (0-based) count, so the FIRST
    /// clock after a `reset` (i.e. the first F8 following a MIDI Start) fires step 0.
    /// MIDI spec: the downbeat sounds on the first clock after Start, not the 6th —
    /// gating on the post-increment count made playback permanently one 16th late.
    /// Cadence stays 6-ticks-per-16th (fires on counts 0, 6, 12, …).
    pub fn on_tick(&mut self, now_micros: u64) -> bool {
        // Record interval since previous tick.
        if let Some(prev) = self.last_tick_micros {
            let interval = now_micros.saturating_sub(prev);
            self.recent_intervals[self.ring_head] = interval;
            self.ring_head = (self.ring_head + 1) % RING_CAPACITY;
            if self.ring_count < RING_CAPACITY {
                self.ring_count += 1;
            }
        }
        self.last_tick_micros = Some(now_micros);

        // Signal a step advance on counts 0, 6, 12, … (0-based): the first tick after a
        // reset fires step 0, then every TICKS_PER_STEP thereafter.
        let step_due = self.tick_count.is_multiple_of(TICKS_PER_STEP);
        self.tick_count += 1;
        step_due
    }

    /// Moving-average BPM estimate from recent inter-tick intervals.
    ///
    /// Returns `None` until at least `MIN_SAMPLES` intervals have been recorded.
    /// Formula: `bpm = 60_000_000.0 / (mean_interval_micros × 24.0)`
    pub fn smoothed_bpm(&self) -> Option<f64> {
        if self.ring_count < MIN_SAMPLES {
            return None;
        }
        let sum: u64 = self.recent_intervals[..self.ring_count].iter().sum();
        let mean = sum as f64 / self.ring_count as f64;
        if mean == 0.0 {
            return None;
        }
        Some(60_000_000.0 / (mean * 24.0))
    }

    /// Returns `true` if no tick has been received within `timeout_micros` of `now_micros`.
    ///
    /// If no tick has ever been received, returns `true` (the source is considered lost).
    pub fn is_lost(&self, now_micros: u64, timeout_micros: u64) -> bool {
        match self.last_tick_micros {
            None => true,
            Some(last) => now_micros.saturating_sub(last) > timeout_micros,
        }
    }

    /// Reset tick counter and interval history (e.g. on MIDI Start/Continue).
    pub fn reset(&mut self) {
        self.tick_count = 0;
        self.last_tick_micros = None;
        self.ring_head = 0;
        self.ring_count = 0;
    }

    /// Total number of ticks received since construction or last reset.
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

impl Default for ClockInState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Parser tests -------------------------------------------------------

    #[test]
    fn parse_tick() {
        assert_eq!(parse_realtime(0xF8, None), Some(ClockInMsg::Tick));
    }

    #[test]
    fn parse_start() {
        assert_eq!(parse_realtime(0xFA, None), Some(ClockInMsg::Start));
    }

    #[test]
    fn parse_continue() {
        assert_eq!(parse_realtime(0xFB, None), Some(ClockInMsg::Continue));
    }

    #[test]
    fn parse_stop() {
        assert_eq!(parse_realtime(0xFC, None), Some(ClockInMsg::Stop));
    }

    #[test]
    fn parse_spp_zero() {
        // SPP beat 0: lsb=0, msb=0 → position 0
        assert_eq!(
            parse_realtime(0xF2, Some((0, 0))),
            Some(ClockInMsg::SongPosition(0))
        );
    }

    #[test]
    fn parse_spp_lsb_only() {
        // lsb=63 (0x3F), msb=0 → position 63
        assert_eq!(
            parse_realtime(0xF2, Some((63, 0))),
            Some(ClockInMsg::SongPosition(63))
        );
    }

    #[test]
    fn parse_spp_msb_only() {
        // lsb=0, msb=1 → position = 1 << 7 = 128
        assert_eq!(
            parse_realtime(0xF2, Some((0, 1))),
            Some(ClockInMsg::SongPosition(128))
        );
    }

    #[test]
    fn parse_spp_both_bytes() {
        // lsb=64, msb=1 → position = (1 << 7) | 64 = 192
        assert_eq!(
            parse_realtime(0xF2, Some((64, 1))),
            Some(ClockInMsg::SongPosition(192))
        );
    }

    #[test]
    fn parse_spp_max() {
        // 14-bit max: lsb=127 (0x7F), msb=127 (0x7F) → (127 << 7) | 127 = 16383
        assert_eq!(
            parse_realtime(0xF2, Some((0x7F, 0x7F))),
            Some(ClockInMsg::SongPosition(16383))
        );
    }

    #[test]
    fn parse_spp_requires_data() {
        // 0xF2 without data bytes → None (can't decode)
        assert_eq!(parse_realtime(0xF2, None), None);
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(parse_realtime(0x90, None), None); // NoteOn — not a realtime msg
        assert_eq!(parse_realtime(0xFE, None), None); // Active Sensing — not handled
        assert_eq!(parse_realtime(0xFF, None), None); // System Reset — not handled
        assert_eq!(parse_realtime(0xF0, None), None); // SysEx — not handled
    }

    // --- on_tick / step advance tests ---------------------------------------

    #[test]
    fn on_tick_returns_true_every_6th_call() {
        let mut st = ClockInState::new();
        let mut advances = 0usize;
        for i in 0..24u64 {
            if st.on_tick(i * 1000) {
                advances += 1;
            }
        }
        // 24 ticks → exactly 4 step advances (0-based counts 0, 6, 12, 18)
        assert_eq!(advances, 4, "24 ticks should produce 4 step advances");
    }

    #[test]
    fn on_tick_advance_fires_on_first_then_every_6th() {
        let mut st = ClockInState::new();
        // Tick 1 (first F8 after Start) fires step 0.
        assert!(st.on_tick(0), "first clock after Start must fire step 0");
        // Ticks 2..6: no advance
        for i in 1..6u64 {
            assert!(!st.on_tick(i * 1000), "tick {} should not advance", i + 1);
        }
        // Tick 7: advance (step 1)
        assert!(st.on_tick(6000), "tick 7 must advance");
        // Ticks 8..12: no advance
        for i in 7..12u64 {
            assert!(!st.on_tick(i * 1000), "tick {} should not advance", i + 1);
        }
        // Tick 13: advance (step 2)
        assert!(st.on_tick(12000), "tick 13 must advance");
    }

    #[test]
    fn on_tick_single_step_boundary() {
        let mut st = ClockInState::new();
        let results: Vec<bool> = (0..6).map(|i| st.on_tick(i * 500)).collect();
        assert_eq!(
            results,
            vec![true, false, false, false, false, false],
            "the first call (first clock after Start) returns true"
        );
    }

    #[test]
    fn first_tick_after_reset_fires_step_0() {
        // H3 regression: MIDI Start resets, then the FIRST F8 must fire step 0.
        // Previously step 0 waited for the 6th clock → playback one 16th late.
        let mut st = ClockInState::new();
        // Warm up (simulate a prior run) then reset, as the Start handler does.
        for i in 0..10u64 {
            st.on_tick(i * 1000);
        }
        st.reset();
        assert!(
            st.on_tick(100_000),
            "first tick after reset (Start) must fire step 0"
        );
        // The next 5 ticks are silent; step 1 lands on the 6th tick after Start.
        for i in 1..6u64 {
            assert!(!st.on_tick(100_000 + i * 1000));
        }
        assert!(st.on_tick(106_000), "step 1 fires 6 ticks after Start");
    }

    // --- smoothed_bpm tests ------------------------------------------------

    /// 120 BPM → inter-tick interval = 60_000_000 / (120 * 24) ≈ 20833 µs.
    const BPM_120_INTERVAL: u64 = 20833;

    fn feed_ticks(st: &mut ClockInState, count: usize, interval: u64) {
        let mut t = 0u64;
        for _ in 0..count {
            st.on_tick(t);
            t += interval;
        }
    }

    #[test]
    fn smoothed_bpm_none_until_min_samples() {
        let mut st = ClockInState::new();
        // Need MIN_SAMPLES intervals = MIN_SAMPLES + 1 ticks (first tick has no interval).
        for i in 0..(MIN_SAMPLES as u64) {
            st.on_tick(i * BPM_120_INTERVAL);
            // After MIN_SAMPLES ticks we have MIN_SAMPLES - 1 intervals (< MIN_SAMPLES).
        }
        // Exactly MIN_SAMPLES - 1 intervals recorded; should still be None.
        assert!(
            st.smoothed_bpm().is_none(),
            "not enough samples yet: ring_count = {}",
            st.ring_count
        );
    }

    #[test]
    fn smoothed_bpm_steady_120() {
        let mut st = ClockInState::new();
        // Feed MIN_SAMPLES + 1 ticks so we have MIN_SAMPLES intervals.
        feed_ticks(&mut st, MIN_SAMPLES + 1, BPM_120_INTERVAL);
        let bpm = st.smoothed_bpm().expect("should have enough samples");
        assert!((bpm - 120.0).abs() < 1.0, "expected ~120 BPM, got {bpm:.2}");
    }

    #[test]
    fn smoothed_bpm_full_ring_steady_120() {
        let mut st = ClockInState::new();
        // Fill the ring completely.
        feed_ticks(&mut st, RING_CAPACITY + 1, BPM_120_INTERVAL);
        let bpm = st.smoothed_bpm().expect("ring full");
        assert!(
            (bpm - 120.0).abs() < 0.5,
            "full-ring steady 120 BPM: got {bpm:.2}"
        );
    }

    #[test]
    fn smoothed_bpm_jitter_stays_near_true_tempo() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut st = ClockInState::new();
        // Simulate ±5% jitter deterministically using a hash-based pseudo-random sequence.
        let mut t = 0u64;
        let jitter_pct = 0.05_f64;
        for i in 0..(RING_CAPACITY + 5) as u64 {
            let mut h = DefaultHasher::new();
            i.hash(&mut h);
            let hash = h.finish();
            // Map hash to [-jitter, +jitter] range.
            let factor = 1.0 + jitter_pct * (((hash % 1000) as f64 / 500.0) - 1.0);
            let interval = (BPM_120_INTERVAL as f64 * factor) as u64;
            st.on_tick(t);
            t += interval;
        }
        let bpm = st.smoothed_bpm().expect("enough samples after jitter run");
        assert!(
            (bpm - 120.0).abs() < 6.0,
            "jittered 120 BPM: smoothed estimate {bpm:.2} should stay within 6 BPM of true tempo"
        );
    }

    #[test]
    fn smoothed_bpm_different_tempo() {
        // 90 BPM: interval = 60_000_000 / (90 * 24) = 27778 µs
        let interval_90 = 60_000_000u64 / (90 * 24);
        let mut st = ClockInState::new();
        feed_ticks(&mut st, RING_CAPACITY + 1, interval_90);
        let bpm = st.smoothed_bpm().expect("enough samples");
        assert!((bpm - 90.0).abs() < 1.0, "expected ~90 BPM, got {bpm:.2}");
    }

    // --- is_lost tests ------------------------------------------------------

    #[test]
    fn is_lost_true_before_any_tick() {
        let st = ClockInState::new();
        assert!(st.is_lost(0, 1_000_000), "no ticks yet → always lost");
        assert!(st.is_lost(1_000_000, 500_000), "no ticks yet → always lost");
    }

    #[test]
    fn is_lost_false_right_after_tick() {
        let mut st = ClockInState::new();
        st.on_tick(1_000_000);
        // now_micros == last_tick → gap is 0, not > timeout
        assert!(
            !st.is_lost(1_000_000, 500_000),
            "just ticked → not lost (gap 0)"
        );
        // A small gap well within timeout
        assert!(
            !st.is_lost(1_100_000, 500_000),
            "100ms gap < 500ms timeout → not lost"
        );
    }

    #[test]
    fn is_lost_true_after_timeout_expires() {
        let mut st = ClockInState::new();
        st.on_tick(1_000_000);
        // Exactly at the boundary: gap == timeout → NOT lost (strictly greater-than)
        assert!(
            !st.is_lost(1_500_000, 500_000),
            "gap == timeout is NOT lost (strict >)"
        );
        // One µs past: gap > timeout → lost
        assert!(
            st.is_lost(1_500_001, 500_000),
            "gap 500001µs > 500000µs timeout → lost"
        );
    }

    #[test]
    fn is_lost_resets_after_new_tick() {
        let mut st = ClockInState::new();
        st.on_tick(0);
        // Far in the future — lost
        assert!(st.is_lost(2_000_000, 500_000), "should be lost after 2s");
        // New tick arrives; immediately not lost
        st.on_tick(2_000_000);
        assert!(
            !st.is_lost(2_000_000, 500_000),
            "new tick resets lost state"
        );
    }

    // --- reset test ---------------------------------------------------------

    #[test]
    fn reset_clears_state() {
        let mut st = ClockInState::new();
        feed_ticks(&mut st, RING_CAPACITY + 1, BPM_120_INTERVAL);
        assert!(st.smoothed_bpm().is_some());
        st.reset();
        assert_eq!(st.tick_count(), 0);
        assert!(st.smoothed_bpm().is_none());
        assert!(st.is_lost(0, 1), "after reset, no last tick → lost");
    }
}
