//! MIDI output sinks and port matching. All MIDI output flows through the
//! [`MidiSink`] trait so tests can substitute a [`RecordingSink`] (no hardware).
//!
//! `connect()` and `list_output_ports()` talk to real CoreMIDI via `midir` and are
//! exercised only in manual acceptance, never in CI. Only `match_port` (pure) and
//! `RecordingSink` are unit-tested.

use crate::midi::MidiMessage;
use anyhow::{anyhow, Context, Result};
use midir::{MidiOutput, MidiOutputConnection};

/// A destination for MIDI messages. `at_micros` is the intended monotonic send time;
/// [`RecordingSink`] stores it for assertions while [`MidirSink`] ignores it because
/// the engine only calls `send()` when an event is already due.
pub trait MidiSink: Send {
    fn send(&mut self, msg: MidiMessage, at_micros: u64);
    /// Returns `false` after the first failed `send` (hardware disconnect / buffer full).
    /// Default: always healthy (used by `NullSink` and `RecordingSink`).
    fn health(&self) -> bool {
        true
    }
}

/// In-memory sink that records every `(at_micros, msg)` it receives, in call order.
pub struct RecordingSink {
    pub events: Vec<(u64, MidiMessage)>,
}

impl RecordingSink {
    pub fn new() -> Self {
        RecordingSink { events: Vec::new() }
    }
}

impl Default for RecordingSink {
    fn default() -> Self {
        RecordingSink::new()
    }
}

impl MidiSink for RecordingSink {
    fn send(&mut self, msg: MidiMessage, at_micros: u64) {
        self.events.push((at_micros, msg));
    }
}

/// No-op sink: discards every message. Used when no hardware device matched a profile
/// so the app runs without connected hardware (avoids unbounded memory growth of RecordingSink).
pub struct NullSink;

impl MidiSink for NullSink {
    fn send(&mut self, _msg: MidiMessage, _at_micros: u64) {}
}

/// Hardware sink wrapping a `midir` output connection. Writes bytes immediately and
/// ignores `at_micros`. Tracks `healthy`: flipped to `false` on the first send error
/// so the engine hot-plug loop can swap it out for a `NullSink`.
pub struct MidirSink {
    conn: MidiOutputConnection,
    healthy: bool,
}

impl MidirSink {
    pub fn new(conn: MidiOutputConnection) -> Self {
        MidirSink {
            conn,
            healthy: true,
        }
    }
}

impl MidiSink for MidirSink {
    fn send(&mut self, msg: MidiMessage, _at_micros: u64) {
        let bytes = msg.to_bytes();
        // Best-effort: a failed write must not crash the engine thread.
        // Flip `healthy` so the hot-plug loop can detect the loss and emit DeviceStatus.
        if self.conn.send(&bytes).is_err() {
            self.healthy = false;
        }
    }

    fn health(&self) -> bool {
        self.healthy
    }
}

/// First index whose name contains `needle` (case-insensitive). Pure; unit-tested.
pub fn match_port(port_names: &[String], needle: &str) -> Option<usize> {
    let needle = needle.to_lowercase();
    port_names
        .iter()
        .position(|name| name.to_lowercase().contains(&needle))
}

/// Enumerate output port names. Not unit-tested (touches real MIDI hardware).
pub fn list_output_ports() -> Vec<String> {
    let out = match MidiOutput::new("midip-enumerate") {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// Connect to the first output port whose name matches `port_match`. Not unit-tested.
pub fn connect(port_match: &str) -> Result<MidirSink> {
    let out = MidiOutput::new("midip").context("create MIDI output")?;
    let ports = out.ports();
    let port_pairs: Vec<_> = ports
        .iter()
        .filter_map(|p| out.port_name(p).ok().map(|name| (p, name)))
        .collect();
    let names: Vec<String> = port_pairs.iter().map(|(_, n)| n.clone()).collect();
    let idx = match_port(&names, port_match)
        .ok_or_else(|| anyhow!("no MIDI output port matching {:?}", port_match))?;
    let conn = out
        .connect(port_pairs[idx].0, "midip-out")
        .map_err(|e| anyhow!("failed to connect to MIDI port: {}", e))?;
    Ok(MidirSink::new(conn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiMessage;

    fn names() -> Vec<String> {
        vec![
            "IAC Driver Bus 1".to_string(),
            "Roland T-8".to_string(),
            "Roland S-1".to_string(),
        ]
    }

    #[test]
    fn match_port_finds_first_substring() {
        assert_eq!(match_port(&names(), "T-8"), Some(1));
        assert_eq!(match_port(&names(), "S-1"), Some(2));
    }

    #[test]
    fn match_port_is_case_insensitive() {
        assert_eq!(match_port(&names(), "t-8"), Some(1));
        assert_eq!(match_port(&names(), "iac"), Some(0));
    }

    #[test]
    fn match_port_returns_none_on_miss() {
        assert_eq!(match_port(&names(), "J-6"), None);
    }

    #[test]
    fn recording_sink_records_in_order() {
        let mut sink = RecordingSink::new();
        sink.send(
            MidiMessage::NoteOn {
                channel: 0,
                note: 60,
                vel: 100,
            },
            1000,
        );
        sink.send(
            MidiMessage::NoteOff {
                channel: 0,
                note: 60,
            },
            1500,
        );
        sink.send(MidiMessage::Clock, 1600);
        assert_eq!(
            sink.events,
            vec![
                (
                    1000,
                    MidiMessage::NoteOn {
                        channel: 0,
                        note: 60,
                        vel: 100
                    }
                ),
                (
                    1500,
                    MidiMessage::NoteOff {
                        channel: 0,
                        note: 60
                    }
                ),
                (1600, MidiMessage::Clock),
            ]
        );
    }

    // --- health() tests ---

    #[test]
    fn recording_sink_is_always_healthy() {
        let mut sink = RecordingSink::new();
        assert!(sink.health());
        sink.send(MidiMessage::Clock, 0);
        assert!(sink.health());
    }

    #[test]
    fn null_sink_is_always_healthy() {
        let sink = NullSink;
        assert!(sink.health());
    }

    /// Test double that simulates a failing hardware connection without opening real MIDI.
    /// Mirrors the MidirSink contract: starts healthy, flips unhealthy on a failed send.
    struct FailingSink {
        healthy: bool,
        should_fail: bool,
    }
    impl MidiSink for FailingSink {
        fn send(&mut self, _msg: MidiMessage, _at_micros: u64) {
            if self.should_fail {
                self.healthy = false;
            }
        }
        fn health(&self) -> bool {
            self.healthy
        }
    }

    #[test]
    fn sink_test_double_starts_healthy_and_flips_on_error() {
        let mut sink = FailingSink {
            healthy: true,
            should_fail: false,
        };
        assert!(sink.health(), "should start healthy");
        sink.send(MidiMessage::Clock, 0);
        assert!(sink.health(), "still healthy when send succeeds");

        sink.should_fail = true;
        sink.send(MidiMessage::Clock, 0);
        assert!(!sink.health(), "should be unhealthy after failed send");
    }
}
