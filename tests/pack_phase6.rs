//! Phase 6 genre-pack validation: representative musical-structure tests,
//! v2 loading/merge integrity, family/function surfacing, and metadata carriage.
//!
//! These assert the *shape* of the generated content, not every pattern. The
//! generator lives in the project scratchpad; this locks the invariants the
//! shipped `assets/patterns/v2/*.json` must satisfy.

use std::path::Path;

use midip::pattern::format_v2::parse_pattern_v2;
use midip::pattern::library::{LibRole, Library};
use midip::pattern::model::PatternData;

fn lib() -> Library {
    Library::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns")).unwrap()
}

fn drum_hits(lib: &Library, genre: &str, name: &str) -> Vec<Vec<(u8, u8)>> {
    let p = lib
        .find("drums", genre, name)
        .expect("drum pattern present");
    match &p.data {
        PatternData::Drums(steps) => steps
            .iter()
            .map(|s| s.iter().map(|h| (h.note, h.vel)).collect())
            .collect(),
        _ => panic!("expected drums"),
    }
}

#[test]
fn all_v2_packs_load_without_being_skipped() {
    let l = lib();
    // Every generated v2 file registers a stable factory id (none skipped/rejected).
    assert!(
        l.v2_index.ids.len() >= 90,
        "expected the full v2 pack set to load, got {}",
        l.v2_index.ids.len()
    );
    // All six packs' genres are present across the roles.
    for (role, genre) in [
        (LibRole::Drums, "boom-bap"),
        (LibRole::Drums, "trap"),
        (LibRole::Drums, "funk"),
        (LibRole::Drums, "disco"),
        (LibRole::Drums, "reggae"),
        (LibRole::Drums, "dancehall"),
        (LibRole::Drums, "afro-house"),
        (LibRole::Drums, "amapiano"),
        (LibRole::Drums, "reggaeton"),
        (LibRole::Drums, "baile-funk"),
        (LibRole::Drums, "tech-house"),
        (LibRole::Drums, "melodic-techno"),
        (LibRole::Drums, "hard-techno"),
        (LibRole::Drums, "footwork"),
    ] {
        assert!(
            l.genres(role).contains(&genre),
            "genre {genre} missing for {role:?}"
        );
    }
}

#[test]
fn reggae_one_drop_has_empty_beat_one_and_drop_on_three() {
    let steps = drum_hits(&lib(), "reggae", "One Drop");
    // The "drop": no kick or snare on beat 1 (hats may tick). Kick lands only on 3.
    assert!(
        !steps[0].iter().any(|(n, _)| *n == 36 || *n == 38),
        "one-drop beat 1 must carry no kick or snare"
    );
    // Kick (36) and cross-stick rim (37) land together on beat 3 (step 8).
    let notes: Vec<u8> = steps[8].iter().map(|(n, _)| *n).collect();
    assert!(notes.contains(&36) && notes.contains(&37), "drop on beat 3");
}

#[test]
fn disco_is_four_on_the_floor() {
    let steps = drum_hits(&lib(), "disco", "Disco Floor");
    for beat in [0, 4, 8, 12] {
        assert!(
            steps[beat].iter().any(|(n, _)| *n == 36),
            "kick on step {beat}"
        );
    }
}

#[test]
fn reggaeton_has_canonical_dembow_snare_figure() {
    let steps = drum_hits(&lib(), "reggaeton", "Dembow Core");
    // Snare (38) on the dembow sixteenths: steps 4,7,12,15 (0-based 3,6,11,14).
    for s in [3, 6, 11, 14] {
        assert!(
            steps[s].iter().any(|(n, _)| *n == 38),
            "dembow snare at step {s}"
        );
    }
}

#[test]
fn trap_and_footwork_use_real_ratchets() {
    let l = lib();
    // Trap core: at least one hi-hat step carries a ratchet > 1 (real sub-step roll).
    let trap = l.find("drums", "trap", "Trap Core").unwrap();
    let has_ratchet = |p: &midip::pattern::model::Pattern| match &p.data {
        PatternData::Drums(st) => st.iter().flatten().any(|h| h.ratchet > 1),
        _ => false,
    };
    assert!(has_ratchet(trap), "trap core must use hat ratchets");
    // Footwork skitter: the snare backbeat is a triplet ratchet (==3).
    let fw = l.find("drums", "footwork", "Footwork Skitter").unwrap();
    let triplet = match &fw.data {
        PatternData::Drums(st) => st.iter().flatten().any(|h| h.note == 38 && h.ratchet == 3),
        _ => false,
    };
    assert!(triplet, "footwork snare must be a triplet ratchet");
}

#[test]
fn amapiano_log_drum_is_a_gliding_bass_line() {
    let l = lib();
    let p = l
        .find("bass", "amapiano", "Log Drum")
        .expect("log drum present");
    match &p.data {
        PatternData::Melodic(steps) => {
            let notes: Vec<_> = steps.iter().flat_map(|s| s.iter()).collect();
            assert!(!notes.is_empty());
            assert!(notes.iter().any(|n| n.slide), "log drum must glide");
            assert!(notes.iter().all(|n| n.len > 0.0), "real note lengths");
        }
        _ => panic!("log drum must be on the melodic bass lane"),
    }
}

#[test]
fn poly_synth_packs_carry_chords() {
    let l = lib();
    // Amapiano jazzy keys must contain a genuine chord step (2+ simultaneous notes).
    let p = l.find("synth", "amapiano", "Jazzy Keys").unwrap();
    let has_chord = match &p.data {
        PatternData::Melodic(steps) => steps.iter().any(|s| s.len() >= 2),
        _ => false,
    };
    assert!(has_chord, "amapiano keys must contain chords");
}

#[test]
fn pack_patterns_are_enrolled_in_families_with_functions() {
    use midip::pattern::library::PatternFunction;
    let l = lib();
    let (fam, func) = l
        .family_of(LibRole::Drums, "trap", "Trap Core")
        .expect("trap core in a family");
    assert_eq!(fam.label, "Trap Hats");
    assert_eq!(func, PatternFunction::Core);
    // A fill member maps to the Fill function.
    let (_, func) = l
        .family_of(LibRole::Drums, "trap", "Hat Roll Fill")
        .expect("trap fill enrolled");
    assert_eq!(func, PatternFunction::Fill);
}

#[test]
fn v2_files_carry_full_metadata_and_provenance() {
    // Parse a representative file directly and confirm the metadata vocabulary +
    // provenance are recorded (they live in the v2 envelope).
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns/v2/drums-trap-trap-core.json");
    let json = std::fs::read_to_string(&path).expect("fixture present");
    let loaded = parse_pattern_v2(&json, "trap-core").unwrap();
    let m = &loaded.metadata;
    for key in [
        "bpm_min",
        "bpm_max",
        "feel",
        "energy",
        "density",
        "tags",
        "compatible_devices",
    ] {
        assert!(m.contains_key(key), "metadata missing {key}");
    }
    let prov = loaded.provenance.expect("provenance present");
    assert!(prov.contains_key("source") && prov.contains_key("references"));
    // Honest timing: Phase 6 does not claim swing/triplet feel it cannot yet render.
    let feel = m.get("feel").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !feel.contains("swing") && !feel.contains("shuffle"),
        "Phase 6 feel must not claim swing (deferred to Phase 7): {feel}"
    );
}
