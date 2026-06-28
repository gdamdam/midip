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

/// Hardware sink wrapping a `midir` output connection. Writes bytes immediately and
/// ignores `at_micros`.
pub struct MidirSink {
    conn: MidiOutputConnection,
}

impl MidirSink {
    pub fn new(conn: MidiOutputConnection) -> Self {
        MidirSink { conn }
    }
}

impl MidiSink for MidirSink {
    fn send(&mut self, msg: MidiMessage, _at_micros: u64) {
        let bytes = msg.to_bytes();
        // Best-effort: a failed write must not crash the engine thread. The error is
        // dropped here; device presence is surfaced separately via DeviceStatus.
        let _ = self.conn.send(&bytes);
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
    let names: Vec<String> = ports
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect();
    let idx = match_port(&names, port_match)
        .ok_or_else(|| anyhow!("no MIDI output port matching {:?}", port_match))?;
    let conn = out
        .connect(&ports[idx], "midip-out")
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
        sink.send(MidiMessage::NoteOn { channel: 0, note: 60, vel: 100 }, 1000);
        sink.send(MidiMessage::NoteOff { channel: 0, note: 60 }, 1500);
        sink.send(MidiMessage::Clock, 1600);
        assert_eq!(
            sink.events,
            vec![
                (1000, MidiMessage::NoteOn { channel: 0, note: 60, vel: 100 }),
                (1500, MidiMessage::NoteOff { channel: 0, note: 60 }),
                (1600, MidiMessage::Clock),
            ]
        );
    }
}
