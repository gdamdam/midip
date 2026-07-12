//! MIDI wire-format messages. Channels are 0-indexed internally and masked to the
//! low nibble (`0..=15`) on encode so an out-of-range channel can never corrupt the
//! status byte. Data bytes (note/velocity/controller/value) are likewise masked to
//! 7 bits (`0..=127`) — a data byte with the high bit set would be parsed as a new
//! status byte by the receiver, corrupting the stream (M8 defense in depth).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        vel: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    /// Control Change (used for panic: CC 123 All Notes Off, CC 120 All Sound Off).
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    Clock,
    Start,
    Stop,
    Continue,
}

impl MidiMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        match *self {
            MidiMessage::NoteOn { channel, note, vel } => {
                vec![0x90 | (channel & 0x0F), note & 0x7F, vel & 0x7F]
            }
            // NoteOff always sends velocity 0 (release velocity is unused here).
            MidiMessage::NoteOff { channel, note } => {
                vec![0x80 | (channel & 0x0F), note & 0x7F, 0]
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                vec![0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F]
            }
            MidiMessage::Clock => vec![0xF8],
            MidiMessage::Start => vec![0xFA],
            MidiMessage::Stop => vec![0xFC],
            MidiMessage::Continue => vec![0xFB],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_encodes_status_note_vel() {
        let m = MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            vel: 100,
        };
        assert_eq!(m.to_bytes(), vec![0x90, 60, 100]);
    }

    #[test]
    fn note_on_channel_is_or_ed_into_status() {
        let m = MidiMessage::NoteOn {
            channel: 9,
            note: 36,
            vel: 127,
        };
        assert_eq!(m.to_bytes(), vec![0x99, 36, 127]);
    }

    #[test]
    fn note_off_uses_0x80_and_zero_velocity() {
        let m = MidiMessage::NoteOff {
            channel: 1,
            note: 45,
        };
        assert_eq!(m.to_bytes(), vec![0x81, 45, 0]);
    }

    #[test]
    fn channel_is_masked_to_low_nibble() {
        // channel 16 (out of range) must not bleed into the status byte.
        let on = MidiMessage::NoteOn {
            channel: 16,
            note: 64,
            vel: 64,
        };
        assert_eq!(on.to_bytes(), vec![0x90, 64, 64]);
        let off = MidiMessage::NoteOff {
            channel: 0xFF,
            note: 64,
        };
        assert_eq!(off.to_bytes(), vec![0x8F, 64, 0]);
    }

    #[test]
    fn data_bytes_are_masked_to_seven_bits() {
        // M8: an out-of-range data byte (>= 0x80) would read as a status byte on the
        // wire — every data byte must be masked to 0..=127.
        let on = MidiMessage::NoteOn {
            channel: 0,
            note: 200,
            vel: 200,
        };
        assert_eq!(on.to_bytes(), vec![0x90, 200 & 0x7F, 200 & 0x7F]);
        let off = MidiMessage::NoteOff {
            channel: 0,
            note: 0xFF,
        };
        assert_eq!(off.to_bytes(), vec![0x80, 0x7F, 0]);
        let cc = MidiMessage::ControlChange {
            channel: 0,
            controller: 200,
            value: 255,
        };
        assert_eq!(cc.to_bytes(), vec![0xB0, 200 & 0x7F, 0x7F]);
        for m in [on, off, cc] {
            for b in &m.to_bytes()[1..] {
                assert!(*b <= 0x7F, "data byte {b:#04x} exceeds 7 bits");
            }
        }
    }

    #[test]
    fn realtime_messages_are_single_status_bytes() {
        assert_eq!(MidiMessage::Clock.to_bytes(), vec![0xF8]);
        assert_eq!(MidiMessage::Start.to_bytes(), vec![0xFA]);
        assert_eq!(MidiMessage::Stop.to_bytes(), vec![0xFC]);
        assert_eq!(MidiMessage::Continue.to_bytes(), vec![0xFB]);
    }

    #[test]
    fn control_change_encodes_status_controller_value() {
        // CC 123 (All Notes Off) on channel 9 -> [0xB9, 123, 0].
        let cc = MidiMessage::ControlChange {
            channel: 9,
            controller: 123,
            value: 0,
        };
        assert_eq!(cc.to_bytes(), vec![0xB9, 123, 0]);
        // Channel masked to the low nibble like the others.
        let cc2 = MidiMessage::ControlChange {
            channel: 16,
            controller: 120,
            value: 0,
        };
        assert_eq!(cc2.to_bytes(), vec![0xB0, 120, 0]);
    }
}
