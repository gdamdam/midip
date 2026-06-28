use crate::pattern::model::LaneKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrumVoice {
    pub label: &'static str,
    pub note: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceProfile {
    pub id: &'static str,         // "t8-drums" | "t8-bass" | "s1"
    pub label: &'static str,      // "T-8 DRUM" | "T-8 BASS" | "S-1 SYNTH"
    pub port_match: &'static str, // "T-8" | "S-1"
    pub kind: LaneKind,
    pub channel: u8,                       // 0-indexed
    pub root_note: u8,                     // melodic base (45); 0 for drums
    pub gate_fraction: f32,                // melodic default len (0.5 bass, 0.9 s1); 0 for drums
    pub drum_gate_fraction: f32,           // 0.1 for drums; 0 for melodic
    pub send_clock: bool,                  // true for all three
    pub drum_voices: &'static [DrumVoice], // non-empty for drums, empty for melodic
}

/// Standard T-8 kit voices, in editor-row order, derived from notes present in the library.
pub const DRUM_VOICES: &[DrumVoice] = &[
    DrumVoice {
        label: "BD",
        note: 36,
    },
    DrumVoice {
        label: "RS",
        note: 37,
    },
    DrumVoice {
        label: "SD",
        note: 38,
    },
    DrumVoice {
        label: "CH",
        note: 42,
    },
    DrumVoice {
        label: "OH",
        note: 46,
    },
    DrumVoice {
        label: "MT",
        note: 47,
    },
    DrumVoice {
        label: "CC",
        note: 49,
    },
    DrumVoice {
        label: "HT",
        note: 50,
    },
    DrumVoice {
        label: "RC",
        note: 51,
    },
    DrumVoice {
        label: "CB",
        note: 56,
    },
];

pub const T8_DRUMS: DeviceProfile = DeviceProfile {
    id: "t8-drums",
    label: "T-8 DRUM",
    port_match: "T-8",
    kind: LaneKind::Drums,
    channel: 9,
    root_note: 0,
    gate_fraction: 0.0,
    drum_gate_fraction: 0.1,
    send_clock: true,
    drum_voices: DRUM_VOICES,
};

pub const T8_BASS: DeviceProfile = DeviceProfile {
    id: "t8-bass",
    label: "T-8 BASS",
    port_match: "T-8",
    kind: LaneKind::Melodic,
    channel: 1,
    root_note: 45,
    gate_fraction: 0.5,
    drum_gate_fraction: 0.0,
    send_clock: true,
    drum_voices: &[],
};

pub const S1: DeviceProfile = DeviceProfile {
    id: "s1",
    label: "S-1 SYNTH",
    port_match: "S-1",
    kind: LaneKind::Melodic,
    channel: 0,
    root_note: 45,
    gate_fraction: 0.9,
    drum_gate_fraction: 0.0,
    send_clock: true,
    drum_voices: &[],
};

pub fn default_profiles() -> [DeviceProfile; 3] {
    [T8_DRUMS, T8_BASS, S1]
}

pub fn profile_by_id(id: &str) -> Option<DeviceProfile> {
    default_profiles().into_iter().find(|p| p.id == id)
}

/// Playback pitch for a melodic note: root + semi + transpose + 12*octave, clamped 0..=127.
pub fn resolve_melodic_pitch(root: u8, semi: i8, transpose: i8, octave: i8) -> u8 {
    let pitch = root as i32 + semi as i32 + transpose as i32 + 12 * octave as i32;
    pitch.clamp(0, 127) as u8
}

/// Melodic velocity multiplier -> MIDI velocity: clamp(round(mult * 100), 1, 127).
pub fn melodic_velocity(mult: f32) -> u8 {
    let v = (mult * 100.0).round() as i32;
    v.clamp(1, 127) as u8
}

/// Human label for a drum note within a profile; "N<note>" when not in the kit.
pub fn drum_label(profile: &DeviceProfile, note: u8) -> String {
    profile
        .drum_voices
        .iter()
        .find(|v| v.note == note)
        .map(|v| v.label.to_string())
        .unwrap_or_else(|| format!("N{}", note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_voices_cover_the_standard_t8_kit() {
        // 10 voices, in order, with the expected notes.
        let notes: Vec<u8> = DRUM_VOICES.iter().map(|v| v.note).collect();
        assert_eq!(notes, vec![36, 37, 38, 42, 46, 47, 49, 50, 51, 56]);
        assert_eq!(DRUM_VOICES[0].label, "BD");
        assert_eq!(DRUM_VOICES[3].label, "CH");
        assert_eq!(DRUM_VOICES[9].label, "CB");
    }

    #[test]
    fn profiles_have_expected_channels_and_kinds() {
        assert_eq!(T8_DRUMS.channel, 9);
        assert_eq!(T8_DRUMS.kind, LaneKind::Drums);
        assert_eq!(T8_DRUMS.drum_gate_fraction, 0.1);
        assert!(!T8_DRUMS.drum_voices.is_empty());

        assert_eq!(T8_BASS.channel, 1);
        assert_eq!(T8_BASS.root_note, 45);
        assert_eq!(T8_BASS.gate_fraction, 0.5);
        assert_eq!(T8_BASS.kind, LaneKind::Melodic);
        assert!(T8_BASS.drum_voices.is_empty());

        assert_eq!(S1.channel, 0);
        assert_eq!(S1.root_note, 45);
        assert_eq!(S1.gate_fraction, 0.9);

        const { assert!(T8_DRUMS.send_clock && T8_BASS.send_clock && S1.send_clock) }
    }

    #[test]
    fn profile_by_id_resolves_and_rejects() {
        assert_eq!(profile_by_id("t8-drums"), Some(T8_DRUMS));
        assert_eq!(profile_by_id("t8-bass"), Some(T8_BASS));
        assert_eq!(profile_by_id("s1"), Some(S1));
        assert_eq!(profile_by_id("nope"), None);
    }

    #[test]
    fn resolve_melodic_pitch_adds_offsets() {
        // root 45 + semi 0 + transpose 0 + 12*0 = 45
        assert_eq!(resolve_melodic_pitch(45, 0, 0, 0), 45);
        // root 45 + semi 7 + transpose 2 + 12*1 = 66
        assert_eq!(resolve_melodic_pitch(45, 7, 2, 1), 66);
        // negative offsets: 45 - 12 - 12 = 21
        assert_eq!(resolve_melodic_pitch(45, -12, 0, -1), 21);
    }

    #[test]
    fn resolve_melodic_pitch_clamps_to_midi_range() {
        // way over the top clamps to 127
        assert_eq!(resolve_melodic_pitch(45, 24, 64, 10), 127);
        // way under the bottom clamps to 0
        assert_eq!(resolve_melodic_pitch(0, -100, -100, -10), 0);
    }

    #[test]
    fn melodic_velocity_scales_and_clamps() {
        assert_eq!(melodic_velocity(0.5), 50);
        assert_eq!(melodic_velocity(1.0), 100);
        assert_eq!(melodic_velocity(1.3), 127); // round(130) clamps to 127
        assert_eq!(melodic_velocity(0.0), 1); // floor clamps up to 1
    }

    #[test]
    fn drum_label_looks_up_voices_with_fallback() {
        assert_eq!(drum_label(&T8_DRUMS, 36), "BD");
        assert_eq!(drum_label(&T8_DRUMS, 56), "CB");
        // unknown note within a drum profile -> "N<note>"
        assert_eq!(drum_label(&T8_DRUMS, 99), "N99");
        // melodic profile has no voices -> always fallback
        assert_eq!(drum_label(&T8_BASS, 36), "N36");
    }
}
