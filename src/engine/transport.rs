//! Transport state: tempo source, play/stop, and tap-tempo.

/// Where the engine gets its BPM.
#[derive(Debug, Clone, PartialEq)]
pub enum TempoSource {
    /// Use `manual_bpm` directly.
    Manual(f64),
    /// Follow the Ableton Link session tempo (falls back to `manual_bpm` when
    /// no peers or Link is disabled).
    Link,
}

/// Top-level transport state owned by the engine reducer.
pub struct Transport {
    pub source: TempoSource,
    pub manual_bpm: f64,
    pub playing: bool,
    /// Ring of recent tap timestamps (µs). Capped at 8 taps.
    taps: Vec<u64>,
}

impl Transport {
    /// Default: Manual 120 BPM, stopped.
    pub fn new() -> Self {
        Transport {
            source: TempoSource::Manual(120.0),
            manual_bpm: 120.0,
            playing: false,
            taps: Vec::with_capacity(8),
        }
    }

    /// Resolve the effective BPM given an optional Link-reported tempo.
    /// Returns the Link tempo when `source == Link` and one is present;
    /// otherwise returns `manual_bpm`.
    pub fn effective_bpm(&self, link_tempo: Option<f64>) -> f64 {
        match self.source {
            TempoSource::Link => link_tempo.unwrap_or(self.manual_bpm),
            TempoSource::Manual(_) => self.manual_bpm,
        }
    }

    /// Record a tap at `now_micros` and update `manual_bpm` from the average
    /// inter-tap interval. Keeps up to 8 recent taps in a ring; needs ≥2 taps
    /// before it can compute a tempo.
    pub fn tap(&mut self, now_micros: u64) {
        self.taps.push(now_micros);
        // Keep only the most recent 8 taps.
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        if self.taps.len() < 2 {
            return;
        }
        // Average the intervals between consecutive taps.
        let intervals: Vec<u64> = self.taps.windows(2).map(|w| w[1] - w[0]).collect();
        let avg_micros = intervals.iter().sum::<u64>() as f64 / intervals.len() as f64;
        // µs per beat → BPM: 60_000_000 / avg_µs
        self.manual_bpm = 60_000_000.0 / avg_micros;
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_manual_120_stopped() {
        let t = Transport::new();
        assert_eq!(t.source, TempoSource::Manual(120.0));
        assert_eq!(t.manual_bpm, 120.0);
        assert!(!t.playing);
    }

    #[test]
    fn effective_bpm_uses_manual_when_source_manual() {
        let mut t = Transport::new();
        t.manual_bpm = 128.0;
        // Even if a Link tempo is present, Manual source ignores it.
        assert_eq!(t.effective_bpm(Some(140.0)), 128.0);
    }

    #[test]
    fn effective_bpm_uses_link_when_source_link_and_present() {
        let mut t = Transport::new();
        t.source = TempoSource::Link;
        t.manual_bpm = 128.0;
        assert_eq!(t.effective_bpm(Some(140.0)), 140.0);
        // Falls back to manual if Link tempo is absent.
        assert_eq!(t.effective_bpm(None), 128.0);
    }

    #[test]
    fn tap_tempo_over_known_intervals_yields_expected_bpm() {
        // 4 taps spaced 500_000 µs apart -> 0.5 s/beat -> 120 BPM.
        let mut t = Transport::new();
        t.tap(0);
        t.tap(500_000);
        t.tap(1_000_000);
        t.tap(1_500_000);
        assert!((t.manual_bpm - 120.0).abs() < 1.0, "got {}", t.manual_bpm);
    }
}
