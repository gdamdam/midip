//! MIDI output sinks and port matching. All MIDI output flows through the
//! [`MidiSink`] trait so tests can substitute a [`RecordingSink`] (no hardware).
//!
//! `connect()` and `list_output_ports()` talk to real CoreMIDI via `midir` and are
//! exercised only in manual acceptance, never in CI. Only `match_port` (pure) and
//! `RecordingSink` are unit-tested.
//!
//! # MIDI clock input
//!
//! [`connect_clock_in`] opens a `midir::MidiInput` connection. Its callback parses
//! incoming bytes via [`crate::engine::clock_in::parse_realtime`] and forwards
//! `ClockInMsg` values over a `crossbeam_channel` sender to the engine. The callback
//! does NOTHING else — no filesystem access, no enumeration, no allocation beyond the
//! channel send (M1 timing-loop purity).
//!
//! The test seam is a [`crossbeam_channel`] pair: tests push `ClockInMsg` directly into
//! the sender end; the engine reads from the receiver end. No hardware is needed in CI.

use crate::engine::clock_in::{parse_realtime, ClockInMsg};
use crate::midi::MidiMessage;
use anyhow::{anyhow, Context, Result};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

/// A destination for MIDI messages. `at_micros` is the intended monotonic send time;
/// [`RecordingSink`] stores it for assertions while [`MidirSink`] ignores it because
/// the engine only calls `send()` when an event is already due.
pub trait MidiSink: Send {
    fn send(&mut self, msg: MidiMessage, at_micros: u64);
    /// Route-aware send: deliver `msg` on behalf of `lane`. The default ignores the lane
    /// and delegates to `send`, so byte-writing sinks (MidirSink, NullSink, RecordingSink)
    /// need no per-lane logic. `PortFanoutSink` overrides this to route by the lane's mapped
    /// port, so two lanes sharing a MIDI channel on different ports deliver independently.
    fn send_lane(&mut self, msg: MidiMessage, _lane: usize, at_micros: u64) {
        self.send(msg, at_micros);
    }
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

/// Create a virtual CoreMIDI output named `name` so other apps on the machine can
/// subscribe to midip as a MIDI source. Not unit-tested (touches CoreMIDI).
///
/// The virtual port is created ONCE at engine startup, outside the hot loop.
/// Returns `None` if the platform does not support virtual outputs or if creation fails.
#[cfg(unix)]
pub fn create_virtual_output(name: &str) -> Option<MidirSink> {
    use midir::os::unix::VirtualOutput;
    MidiOutput::new("midip-virtual")
        .ok()?
        .create_virtual(name)
        .ok()
        .map(MidirSink::new)
}

/// Stub for non-Unix platforms: virtual MIDI outputs are not supported.
#[cfg(not(unix))]
pub fn create_virtual_output(_name: &str) -> Option<MidirSink> {
    None
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

// ---------------------------------------------------------------------------
// MIDI clock input connection
// ---------------------------------------------------------------------------

/// An active MIDI clock-input connection. Dropping this closes the port.
///
/// Wraps a `midir::MidiInputConnection<()>`. The connection's callback parses
/// each incoming byte via `parse_realtime` and forwards `ClockInMsg` values over
/// a `crossbeam_channel::Sender`. The callback is intentionally minimal: parse +
/// forward, nothing else.
pub struct MidirClockIn {
    // Held for Drop: closing the connection stops the callback thread.
    _conn: MidiInputConnection<()>,
}

/// Open a MIDI input connection to the first port whose name contains `port_match`
/// (case-insensitive substring). The connection's callback parses each incoming
/// byte slice via `parse_realtime` and sends matching `ClockInMsg` values to `tx`.
///
/// Returns `None` if no matching port is found or if `MidiInput::new` fails.
/// The returned `MidirClockIn` MUST be kept alive for as long as the connection
/// should remain open — dropping it closes the port.
pub fn connect_clock_in(
    port_match: &str,
    tx: crossbeam_channel::Sender<ClockInMsg>,
) -> Option<MidirClockIn> {
    let input = MidiInput::new("midip-clock-in").ok()?;
    let ports = input.ports();
    let port_pairs: Vec<_> = ports
        .iter()
        .filter_map(|p| input.port_name(p).ok().map(|name| (p, name)))
        .collect();
    let names: Vec<String> = port_pairs.iter().map(|(_, n)| n.clone()).collect();
    let idx = match_port(&names, port_match)?;
    let port = port_pairs[idx].0;

    // The callback is the ONLY place bytes from the hardware arrive. It must do
    // as little as possible: parse the byte slice into ClockInMsg and forward.
    // `parse_bytes_to_channel` handles the framing (single-byte realtime vs. SPP).
    let conn = input
        .connect(
            port,
            "midip-clock-in",
            move |_stamp, bytes, _| {
                parse_bytes_to_channel(bytes, &tx);
            },
            (),
        )
        .ok()?;

    Some(MidirClockIn { _conn: conn })
}

/// Enumerate available MIDI input port names. Not unit-tested (touches real hardware).
pub fn list_input_ports() -> Vec<String> {
    let input = match MidiInput::new("midip-enumerate-in") {
        Ok(i) => i,
        Err(_) => return Vec::new(),
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// Parse a raw MIDI byte slice from a `midir` callback into zero or one `ClockInMsg`
/// and send it to `tx`. This is the test-seam function: tests can call it directly
/// without opening real hardware.
///
/// # Framing
/// - 1-byte slices: single-byte realtime messages (Tick/Start/Continue/Stop).
/// - 3-byte slices starting with `0xF2`: Song Position Pointer (status + lsb + msb).
/// - All other slices: ignored (channel messages, SysEx, etc.).
pub fn parse_bytes_to_channel(bytes: &[u8], tx: &crossbeam_channel::Sender<ClockInMsg>) {
    let msg = match bytes {
        [status] => parse_realtime(*status, None),
        [0xF2, lsb, msb] => parse_realtime(0xF2, Some((*lsb, *msb))),
        _ => None,
    };
    if let Some(m) = msg {
        // Best-effort: if the engine has gone away (Disconnected), ignore the error.
        let _ = tx.send(m);
    }
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

    // ---------------------------------------------------------------------------
    // Clock-in parser + channel forwarding tests (NO hardware — fake source)
    // ---------------------------------------------------------------------------

    /// Helper: push raw bytes through `parse_bytes_to_channel` and collect results.
    fn parse_all(byte_slices: &[&[u8]]) -> Vec<ClockInMsg> {
        let (tx, rx) = crossbeam_channel::unbounded();
        for bytes in byte_slices {
            parse_bytes_to_channel(bytes, &tx);
        }
        drop(tx);
        rx.try_iter().collect()
    }

    #[test]
    fn clock_in_tick_forwarded() {
        let msgs = parse_all(&[&[0xF8]]);
        assert_eq!(msgs, vec![ClockInMsg::Tick]);
    }

    #[test]
    fn clock_in_start_stop_forwarded() {
        let msgs = parse_all(&[&[0xFA], &[0xFC]]);
        assert_eq!(msgs, vec![ClockInMsg::Start, ClockInMsg::Stop]);
    }

    #[test]
    fn clock_in_continue_forwarded() {
        let msgs = parse_all(&[&[0xFB]]);
        assert_eq!(msgs, vec![ClockInMsg::Continue]);
    }

    #[test]
    fn clock_in_spp_forwarded() {
        // SPP: lsb=64, msb=1 → position = (1 << 7) | 64 = 192
        let msgs = parse_all(&[&[0xF2, 64, 1]]);
        assert_eq!(msgs, vec![ClockInMsg::SongPosition(192)]);
    }

    #[test]
    fn clock_in_spp_max_position() {
        // 14-bit max: lsb=0x7F, msb=0x7F → 16383
        let msgs = parse_all(&[&[0xF2, 0x7F, 0x7F]]);
        assert_eq!(msgs, vec![ClockInMsg::SongPosition(16383)]);
    }

    #[test]
    fn clock_in_sequence_tick_start_stop_spp() {
        let msgs = parse_all(&[&[0xFA], &[0xF8], &[0xF8], &[0xF2, 0, 0], &[0xFC]]);
        assert_eq!(
            msgs,
            vec![
                ClockInMsg::Start,
                ClockInMsg::Tick,
                ClockInMsg::Tick,
                ClockInMsg::SongPosition(0),
                ClockInMsg::Stop,
            ]
        );
    }

    #[test]
    fn clock_in_ignores_channel_messages() {
        // NoteOn, NoteOff, CC — not realtime, should produce nothing on the channel.
        let msgs = parse_all(&[&[0x90, 60, 100], &[0x80, 60, 0], &[0xB0, 7, 64]]);
        assert!(
            msgs.is_empty(),
            "channel messages must not produce ClockInMsg"
        );
    }

    #[test]
    fn clock_in_ignores_active_sensing_and_system_reset() {
        let msgs = parse_all(&[&[0xFE], &[0xFF]]);
        assert!(
            msgs.is_empty(),
            "active sensing / system reset must be ignored"
        );
    }

    #[test]
    fn clock_in_absent_port_no_connection_no_panic() {
        // No hardware available; connect_clock_in must return None, not panic.
        let (tx, _rx) = crossbeam_channel::unbounded();
        let result = connect_clock_in("__no_such_port_xyz__", tx);
        assert!(
            result.is_none(),
            "absent port must return None, not panic or error"
        );
    }

    #[test]
    fn clock_in_fake_source_direct_channel_push() {
        // Test-seam: engine code only holds the Receiver end; tests push directly
        // into the Sender end — no hardware needed.
        let (tx, rx) = crossbeam_channel::unbounded::<ClockInMsg>();
        tx.send(ClockInMsg::Start).unwrap();
        tx.send(ClockInMsg::Tick).unwrap();
        tx.send(ClockInMsg::Stop).unwrap();
        drop(tx);
        let collected: Vec<_> = rx.iter().collect();
        assert_eq!(
            collected,
            vec![ClockInMsg::Start, ClockInMsg::Tick, ClockInMsg::Stop]
        );
    }
}
