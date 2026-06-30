use crate::pattern::model::LaneKind;
use std::path::Path;
use std::sync::OnceLock;

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
    /// Whether this lane can hold more than one note per step (chord-capable).
    /// false → mono: the edit layer enforces at most one note per step.
    /// true  → poly: stacking is allowed (chord entry added in M5b Task 4).
    /// Drum profiles: always false here — drum steps are already poly via
    /// Vec<DrumHit> and this field is irrelevant for drum lanes.
    pub poly: bool,
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
    // Irrelevant for drum lanes — drum steps are poly via Vec<DrumHit>.
    poly: false,
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
    // Mono: T-8 BASS has slide and is always single-note per step.
    poly: false,
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
    // Poly: S-1 SYNTH supports chords (chord-stacking added in M5b Task 4).
    poly: true,
};

pub fn default_profiles() -> [DeviceProfile; 3] {
    [T8_DRUMS, T8_BASS, S1]
}

/// The three built-in profiles, always present and reserved: their ids cannot
/// be shadowed by the catalog file. A fresh set is `[T8_DRUMS, T8_BASS, S1]`.
const BUILTINS: [DeviceProfile; 3] = [T8_DRUMS, T8_BASS, S1];

/// Additional shipped device profiles, embedded at compile time so the catalog
/// is always available — including in unit tests and when the assets dir is
/// missing — without touching the filesystem.
const EMBEDDED_CATALOG: &str = include_str!("../../assets/devices/catalog.json");

#[derive(serde::Deserialize)]
struct CatalogFile {
    profiles: Vec<DeviceProfileDto>,
}

#[derive(serde::Deserialize)]
struct DeviceProfileDto {
    id: String,
    label: String,
    port_match: String,
    kind: String, // "drums" | "melodic"
    channel: u8,
    #[serde(default)]
    root_note: u8,
    #[serde(default)]
    gate_fraction: f32,
    #[serde(default)]
    drum_gate_fraction: f32,
    #[serde(default = "default_true")]
    send_clock: bool,
    #[serde(default)]
    drum_voices: Vec<DrumVoiceDto>,
    #[serde(default)]
    poly: bool,
}

#[derive(serde::Deserialize)]
struct DrumVoiceDto {
    label: String,
    note: u8,
}

fn default_true() -> bool {
    true
}

/// Leak an owned string to obtain a `'static` reference. Used only while
/// building the process-lifetime device catalog — a small, one-time leak that
/// lets `DeviceProfile` stay `Copy` and free of lifetime parameters, so the
/// ~200 existing call sites that pass profiles by value are untouched.
fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn to_profile(dto: DeviceProfileDto) -> DeviceProfile {
    let kind = match dto.kind.to_ascii_lowercase().as_str() {
        "drums" => LaneKind::Drums,
        _ => LaneKind::Melodic,
    };
    let voices: Vec<DrumVoice> = dto
        .drum_voices
        .into_iter()
        .map(|v| DrumVoice {
            label: leak_str(v.label),
            note: v.note,
        })
        .collect();
    DeviceProfile {
        id: leak_str(dto.id),
        label: leak_str(dto.label),
        port_match: leak_str(dto.port_match),
        kind,
        channel: dto.channel,
        root_note: dto.root_note,
        gate_fraction: dto.gate_fraction,
        drum_gate_fraction: dto.drum_gate_fraction,
        send_clock: dto.send_clock,
        drum_voices: Box::leak(voices.into_boxed_slice()),
        poly: dto.poly,
    }
}

/// Parse a catalog JSON document (`{ "profiles": [...] }`) into profiles.
fn parse_catalog(json: &str) -> anyhow::Result<Vec<DeviceProfile>> {
    let file: CatalogFile = serde_json::from_str(json)?;
    Ok(file.profiles.into_iter().map(to_profile).collect())
}

static CATALOG: OnceLock<Vec<DeviceProfile>> = OnceLock::new();

/// Build the full catalog: built-ins first (reserved), then user-supplied
/// profiles, then the embedded shipped profiles. Entries whose id is already
/// present are skipped — so built-ins always win, and a user entry shadows the
/// embedded one with the same id.
fn build_catalog(user_json: Option<&str>) -> Vec<DeviceProfile> {
    let mut out: Vec<DeviceProfile> = BUILTINS.to_vec();
    let push = |p: DeviceProfile, out: &mut Vec<DeviceProfile>| {
        if !out.iter().any(|e| e.id == p.id) {
            out.push(p);
        }
    };
    if let Some(json) = user_json {
        // A malformed user file must not take the app down: ignore on parse error.
        if let Ok(profiles) = parse_catalog(json) {
            for p in profiles {
                push(p, &mut out);
            }
        }
    }
    let embedded = parse_catalog(EMBEDDED_CATALOG).expect("embedded device catalog must parse");
    for p in embedded {
        push(p, &mut out);
    }
    out
}

/// The full device catalog: built-ins + shipped profiles + any user additions.
/// Lazily built from the embedded catalog on first use; call
/// [`init_user_catalog`] once at startup to layer in the user's `devices.json`.
pub fn catalog() -> &'static [DeviceProfile] {
    CATALOG.get_or_init(|| build_catalog(None)).as_slice()
}

/// Layer the user's `devices.json` (in `data_dir`) on top of the shipped
/// profiles. Call once at startup before any catalog use; a no-op if the
/// catalog was already built.
pub fn init_user_catalog(data_dir: &Path) {
    let user = std::fs::read_to_string(data_dir.join("devices.json")).ok();
    let _ = CATALOG.set(build_catalog(user.as_deref()));
}

pub fn profile_by_id(id: &str) -> Option<DeviceProfile> {
    catalog().iter().copied().find(|p| p.id == id)
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

    #[test]
    fn bass_profile_is_mono_s1_is_poly() {
        // T-8 BASS has slide; it is a mono lane (one note per step).
        // S-1 SYNTH supports chords. T-8 DRUMS: poly is irrelevant for drum
        // lanes (drum steps are already poly via Vec<DrumHit>); value is false.
        const {
            assert!(!T8_BASS.poly, "T-8 BASS must be mono (poly == false)");
            assert!(S1.poly, "S-1 SYNTH must be poly (poly == true)");
            assert!(
                !T8_DRUMS.poly,
                "T-8 DRUMS poly is false (irrelevant for drum lanes)"
            );
        }
    }

    // --- catalog (data-driven profiles) ---

    #[test]
    fn embedded_catalog_parses() {
        // The bundled catalog.json must always be well-formed; build_catalog
        // expects this and panics otherwise.
        let profiles = parse_catalog(EMBEDDED_CATALOG).expect("embedded catalog parses");
        assert!(!profiles.is_empty());
    }

    #[test]
    fn catalog_includes_builtins_and_shipped_with_unique_ids() {
        let cat = catalog();
        let ids: Vec<&str> = cat.iter().map(|p| p.id).collect();
        // Built-ins present and first.
        assert_eq!(&ids[..3], &["t8-drums", "t8-bass", "s1"]);
        // A sampling of shipped devices and the generic fallbacks.
        for id in [
            "j-6",
            "rd-8",
            "drumbrute-impact",
            "td-3",
            "monologue",
            "microfreak",
            "minilogue-xd",
            "digitakt",
            "circuit-tracks-synth",
            "circuit-tracks-drums",
            "generic-gm-drums",
            "generic-mono-synth",
            "generic-poly-synth",
        ] {
            assert!(ids.contains(&id), "catalog missing {id}; got {ids:?}");
        }
        // No duplicate ids.
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate ids in catalog: {ids:?}");
    }

    #[test]
    fn profile_by_id_resolves_shipped_devices() {
        // RD-8 drum map was verified against the manual: snare is note 40 (not 38).
        let rd8 = profile_by_id("rd-8").expect("rd-8 in catalog");
        assert_eq!(rd8.kind, LaneKind::Drums);
        assert!(!rd8.drum_voices.is_empty());
        assert_eq!(drum_label(&rd8, 40), "SD");
        assert_eq!(drum_label(&rd8, 36), "BD");

        // TD-3 is a mono bass synth rooted at C2 (36).
        let td3 = profile_by_id("td-3").expect("td-3 in catalog");
        assert_eq!(td3.kind, LaneKind::Melodic);
        assert!(!td3.poly);
        assert_eq!(td3.root_note, 36);
        assert!(td3.drum_voices.is_empty());

        // minilogue xd is polyphonic.
        assert!(profile_by_id("minilogue-xd").unwrap().poly);

        assert_eq!(profile_by_id("does-not-exist"), None);
    }

    #[test]
    fn user_profiles_add_but_cannot_shadow_builtins() {
        let user = r#"{ "profiles": [
            { "id": "t8-drums", "label": "HACKED", "port_match": "X", "kind": "melodic", "channel": 5 },
            { "id": "my-synth", "label": "MY SYNTH", "port_match": "MySynth", "kind": "melodic", "channel": 3, "root_note": 50, "poly": true }
        ] }"#;
        let cat = build_catalog(Some(user));

        // Built-in id is reserved: the real T-8 DRUM profile wins, not the user's.
        let t8 = cat.iter().find(|p| p.id == "t8-drums").unwrap();
        assert_eq!(t8.label, "T-8 DRUM");
        assert_eq!(t8.channel, 9);

        // A genuinely new user profile is added.
        let mine = cat
            .iter()
            .find(|p| p.id == "my-synth")
            .expect("user profile added");
        assert_eq!(mine.channel, 3);
        assert_eq!(mine.root_note, 50);
        assert!(mine.poly);
    }

    #[test]
    fn malformed_user_catalog_is_ignored() {
        // Garbage user JSON must not panic or drop the shipped catalog.
        let cat = build_catalog(Some("{ not json"));
        assert!(cat.iter().any(|p| p.id == "t8-drums"));
        assert!(cat.iter().any(|p| p.id == "td-3"));
    }
}
