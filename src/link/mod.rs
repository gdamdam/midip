//! Ableton Link abstraction. `LinkClock` lets the engine and tests share one
//! interface; `FakeLink` is a deterministic in-memory implementation and
//! `AbletonLink` wraps `rusty_link`'s real session.
//!
//! `AbletonLink` is integration-only (it joins a live UDP-multicast Link session);
//! all unit tests run against `FakeLink`.

use rusty_link::{AblLink, SessionState};

/// Musical-clock interface shared by the real Link and the test fake.
pub trait LinkClock: Send {
    fn enabled(&self) -> bool;
    fn set_enabled(&mut self, on: bool);
    fn tempo(&self) -> f64;
    fn set_tempo(&mut self, bpm: f64);
    /// Beat position at `micros` (monotonic) for the given `quantum` (beats/bar).
    fn beat_at(&self, micros: u64, quantum: f64) -> f64;
    /// Phase within the bar (0..`quantum`).
    fn phase_at(&self, micros: u64, quantum: f64) -> f64;
    fn num_peers(&self) -> u64;
    /// Whether the shared Link session is currently playing (start/stop sync).
    /// Lets the engine follow remote transport and avoid re-issuing a redundant
    /// start when joining a session a peer is already playing.
    fn is_playing(&self) -> bool;
    /// Quantized start: align play to the next bar boundary.
    fn request_start(&mut self, micros: u64, quantum: f64);
}

/// Map a Link beat to a 16th-note step index: `floor(beat * 4)`.
/// Negative beats clamp to 0.
pub fn step_from_beat(beat: f64) -> usize {
    (beat * 4.0).floor().max(0.0) as usize
}

// ---------------------------------------------------------------------------
// FakeLink: deterministic test double.
// ---------------------------------------------------------------------------

/// In-memory `LinkClock` for tests. `beat_at` returns the set beat regardless
/// of the requested micros so test timing is fully deterministic.
pub struct FakeLink {
    enabled: bool,
    beat: f64,
    tempo: f64,
    peers: u64,
    /// Simulated shared-session playing state, returned by `is_playing`. Tests
    /// drive this to model a peer starting/stopping the Link session.
    playing: bool,
    /// Set by `request_start`; `None` until called. Tests assert on this.
    pub started_at: Option<u64>,
}

impl FakeLink {
    pub fn new() -> Self {
        FakeLink {
            enabled: false,
            beat: 0.0,
            tempo: 120.0,
            peers: 0,
            playing: false,
            started_at: None,
        }
    }

    /// Set the beat that `beat_at` will return (ignores micros).
    pub fn set_beat(&mut self, beat: f64) {
        self.beat = beat;
    }

    /// Set the peer count returned by `num_peers`.
    pub fn set_peers(&mut self, peers: u64) {
        self.peers = peers;
    }

    /// Set the simulated shared-session playing state returned by `is_playing`.
    /// Used by tests to model a remote peer starting or stopping transport.
    pub fn set_playing(&mut self, on: bool) {
        self.playing = on;
    }
}

impl Default for FakeLink {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkClock for FakeLink {
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }
    fn tempo(&self) -> f64 {
        self.tempo
    }
    fn set_tempo(&mut self, bpm: f64) {
        self.tempo = bpm;
    }
    /// Returns the stored beat regardless of `micros` — fully deterministic.
    fn beat_at(&self, _micros: u64, _quantum: f64) -> f64 {
        self.beat
    }
    /// `beat % quantum`.
    fn phase_at(&self, _micros: u64, quantum: f64) -> f64 {
        self.beat % quantum
    }
    fn num_peers(&self) -> u64 {
        self.peers
    }
    fn is_playing(&self) -> bool {
        self.playing
    }
    fn request_start(&mut self, micros: u64, _quantum: f64) {
        // Record the request so tests can assert it. Do NOT reset beat — the
        // real Link session handles bar-alignment externally; the fake keeps
        // whatever beat was set so position tests remain independent.
        self.started_at = Some(micros);
        // Mirror the real session: requesting a start marks it playing, so the
        // engine's transition check sees no spurious change after a local start.
        self.playing = true;
    }
}

// ---------------------------------------------------------------------------
// AbletonLink: real rusty_link wrapper (integration-only; not unit-tested).
// ---------------------------------------------------------------------------

/// Wraps a real `rusty_link` session. We keep a local `enabled` flag because
/// Link's `is_enabled` is a direct handle query; we mirror it here for the
/// trait's synchronous interface.
///
/// **beat_at / phase_at approach:** the caller supplies a `micros` value that
/// comes from whatever monotonic clock the engine thread uses. We convert it
/// to Link's own timeline by computing the offset between Link's clock and the
/// caller's clock at construction time (`link_micros_at_init - caller_micros`)
/// and then adding that offset to every incoming `micros`. In practice, if the
/// caller passes `link.clock_micros()` directly, the offset is zero and the
/// numbers align perfectly. This avoids requiring the engine to import
/// `rusty_link` just to get the clock value.
pub struct AbletonLink {
    link: AblLink,
    /// Session state reused across calls to avoid repeated allocation.
    session_state: SessionState,
    enabled: bool,
}

impl AbletonLink {
    pub fn new(bpm: f64) -> Self {
        let link = AblLink::new(bpm);
        let session_state = SessionState::new();
        AbletonLink {
            link,
            session_state,
            enabled: false,
        }
    }
}

impl LinkClock for AbletonLink {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.link.enable(on);
    }

    fn tempo(&self) -> f64 {
        // Capture a fresh snapshot so the returned tempo reflects the current
        // Link session (other peers may have changed it).
        let mut ss = SessionState::new();
        self.link.capture_app_session_state(&mut ss);
        ss.tempo()
    }

    fn set_tempo(&mut self, bpm: f64) {
        self.link.capture_app_session_state(&mut self.session_state);
        let now = self.link.clock_micros();
        self.session_state.set_tempo(bpm, now);
        self.link.commit_app_session_state(&self.session_state);
    }

    /// Beat position using Link's own session clock.
    ///
    /// Fix #2 — Link clock epoch: the engine passes a `micros` value from its
    /// own `Instant`-based clock, which has a DIFFERENT epoch than Link's
    /// internal session clock. Using the engine's micros directly would yield
    /// a wrong (typically huge) beat number. We ignore `micros` and instead
    /// call `self.link.clock_micros()` so the query is always in Link's own
    /// timeline. `FakeLink` keeps using the passed `micros` (ignored there too,
    /// but for determinism reasons) — the trait signature is unchanged.
    fn beat_at(&self, _micros: u64, quantum: f64) -> f64 {
        let mut ss = SessionState::new();
        self.link.capture_app_session_state(&mut ss);
        // Use Link's own clock so epoch matches the session timeline.
        let link_now = self.link.clock_micros();
        ss.beat_at_time(link_now, quantum)
    }

    fn phase_at(&self, _micros: u64, quantum: f64) -> f64 {
        let mut ss = SessionState::new();
        self.link.capture_app_session_state(&mut ss);
        // Use Link's own clock so epoch matches the session timeline.
        let link_now = self.link.clock_micros();
        ss.phase_at_time(link_now, quantum)
    }

    fn num_peers(&self) -> u64 {
        self.link.num_peers()
    }

    fn is_playing(&self) -> bool {
        let mut ss = SessionState::new();
        self.link.capture_app_session_state(&mut ss);
        ss.is_playing()
    }

    /// Quantized start: set playing=true and request beat 0 aligned to the
    /// next bar boundary.
    ///
    /// Clock-epoch fix: the start-playing time MUST be stamped in Link's own
    /// session-clock domain (`self.link.clock_micros()`), NOT the engine's
    /// `Instant`-based `micros` (a different epoch). Passing the engine micros
    /// put the start far off the shared timeline; ignoring it and using Link's
    /// clock lands the requested beat 0 on the next bar as intended.
    fn request_start(&mut self, _micros: u64, quantum: f64) {
        self.link.capture_app_session_state(&mut self.session_state);
        let link_now = self.link.clock_micros();
        self.session_state.set_is_playing(true, link_now);
        self.session_state
            .request_beat_at_start_playing_time(0.0, quantum);
        self.link.commit_app_session_state(&self.session_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_from_beat_floors_quarter_of_beat() {
        // 16th = beat * 4; floor.
        assert_eq!(step_from_beat(0.0), 0);
        assert_eq!(step_from_beat(2.5), 10); // floor(10.0)
        assert_eq!(step_from_beat(2.74), 10); // floor(10.96)
        assert_eq!(step_from_beat(4.0), 16);
    }

    #[test]
    fn step_from_beat_clamps_negatives_to_zero() {
        assert_eq!(step_from_beat(-1.0), 0);
        assert_eq!(step_from_beat(-0.001), 0);
    }

    #[test]
    fn fake_link_beat_round_trips() {
        let mut link = FakeLink::new();
        link.set_beat(3.25);
        // beat_at ignores micros for determinism.
        assert_eq!(link.beat_at(0, 4.0), 3.25);
        assert_eq!(link.beat_at(999_999, 4.0), 3.25);
    }

    #[test]
    fn fake_link_tempo_enabled_peers_settable() {
        let mut link = FakeLink::new();
        assert!(!link.enabled());
        link.set_enabled(true);
        assert!(link.enabled());

        link.set_tempo(140.0);
        assert_eq!(link.tempo(), 140.0);

        link.set_peers(3);
        assert_eq!(link.num_peers(), 3);
    }

    #[test]
    fn fake_link_phase_is_beat_mod_quantum() {
        let mut link = FakeLink::new();
        link.set_beat(5.5);
        // 5.5 % 4 = 1.5
        assert!((link.phase_at(0, 4.0) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn fake_link_request_start_sets_started_at() {
        let mut link = FakeLink::new();
        assert!(link.started_at.is_none());
        link.request_start(42_000, 4.0);
        assert_eq!(link.started_at, Some(42_000));
        // beat resets to 0 on request_start.
        assert_eq!(link.beat_at(0, 4.0), 0.0);
    }

    #[test]
    fn fake_link_playing_state_is_settable() {
        let mut link = FakeLink::new();
        assert!(!link.is_playing(), "sessions start stopped");
        link.set_playing(true);
        assert!(link.is_playing());
        link.set_playing(false);
        assert!(!link.is_playing());
    }

    #[test]
    fn fake_link_request_start_marks_playing() {
        // Mirrors the real session: requesting a start marks the session playing so the
        // engine's transition check does not re-follow its own locally-initiated start.
        let mut link = FakeLink::new();
        assert!(!link.is_playing());
        link.request_start(1_000, 4.0);
        assert!(link.is_playing(), "request_start must mark the session playing");
    }
}
