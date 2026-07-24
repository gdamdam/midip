//! Focused tests for the fourth CHORDS lane and hardware-neutral lane roles:
//! fresh four-lane construction, old three-lane persistence migration, role-based
//! runtime targeting, cross-role (synth→chords) reference resolution, and the
//! CHORDS factory-library quality invariants.

use midip::app::App;
use midip::devices::profiles::{default_profiles, J6, S1, T8_BASS, T8_DRUMS};
use midip::pattern::library::{LibRole, Library};
use midip::pattern::model::{LaneKind, Pattern, PatternData, Set};
use midip::pattern::refs::PatternRef;

fn lib() -> Library {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
    Library::load(&dir).expect("library loads")
}

fn vendored(role: &str, genre: &str, name: &str) -> PatternRef {
    PatternRef::Vendored {
        role: role.to_string(),
        genre: genre.to_string(),
        name: name.to_string(),
    }
}

// ── Fresh four-lane template ────────────────────────────────────────────────

#[test]
fn fresh_set_has_four_role_labeled_lanes() {
    let set = Set::default_set(default_profiles());
    assert_eq!(set.lanes.len(), 4, "fresh set has DRUMS/BASS/CHORDS/SYNTH");
    let roles: Vec<LibRole> = set.lanes.iter().map(|l| l.role).collect();
    assert_eq!(
        roles,
        vec![LibRole::Drums, LibRole::Bass, LibRole::Chords, LibRole::Synth]
    );
    // Default devices per role — J-6 is the CHORDS default but not the lane identity.
    assert_eq!(set.lanes[0].profile.id, "t8-drums");
    assert_eq!(set.lanes[1].profile.id, "t8-bass");
    assert_eq!(set.lanes[2].profile.id, "j-6");
    assert_eq!(set.lanes[3].profile.id, "s1");
    // The CHORDS lane holds melodic (chord-capable) data, and its device is poly/4-voice.
    assert_eq!(set.lanes[2].role.lane_kind(), LaneKind::Melodic);
    assert!(matches!(set.lanes[2].pattern.data, PatternData::Melodic(_)));
    assert_eq!(J6.max_voices(), Some(4));
}

#[test]
fn chords_lane_role_is_independent_of_device() {
    // Swap the CHORDS lane's device to another poly synth — the role stays CHORDS.
    let mut set = Set::default_set(default_profiles());
    let poly = midip::devices::profiles::profile_by_id("minilogue-xd").expect("poly synth exists");
    set.lanes[2].profile = poly;
    assert_eq!(set.lanes[2].role, LibRole::Chords, "role is not device-derived");
    assert!(set.lanes[2].profile.poly, "replacement device is polyphonic");
}

// ── Old three-lane persistence migration ────────────────────────────────────

const LEGACY_V4_SET: &str = r#"{
    "version": 4,
    "id": "abcdef0123456789",
    "name": "legacy-three-lane",
    "bpm": 120.0,
    "swing": 0.5,
    "lanes": [
        {"profile_id": "t8-drums", "mute": false, "solo": false, "transpose": 0, "octave": 0, "pattern": {"name": "d", "length": 1, "data": {"Drums": [[]]}}},
        {"profile_id": "t8-bass",  "mute": false, "solo": false, "transpose": 0, "octave": 0, "pattern": {"name": "b", "length": 1, "data": {"Melodic": [null]}}},
        {"profile_id": "s1",       "mute": false, "solo": false, "transpose": 0, "octave": 0, "pattern": {"name": "s", "length": 1, "data": {"Melodic": [null]}}}
    ]
}"#;

#[test]
fn legacy_three_lane_set_loads_as_drums_bass_synth_no_chords() {
    let dir = std::env::temp_dir().join(format!(
        "midip-chords-mig-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("legacy-abcdef01.json");
    std::fs::write(&path, LEGACY_V4_SET).unwrap();

    let (set, _notes) = midip::pattern::store::load_set_with_report(&path).unwrap();
    assert_eq!(set.lanes.len(), 3, "no CHORDS lane is inserted into old sets");
    let roles: Vec<LibRole> = set.lanes.iter().map(|l| l.role).collect();
    assert_eq!(
        roles,
        vec![LibRole::Drums, LibRole::Bass, LibRole::Synth],
        "roles inferred from device: drums/bass/synth (never chords)"
    );
    assert!(
        !set.lanes.iter().any(|l| l.role == LibRole::Chords),
        "an old three-lane set must never gain a CHORDS lane"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ── Role-based runtime targeting ────────────────────────────────────────────

#[test]
fn chords_ref_targets_the_chords_lane_by_role() {
    let app = App::new(Set::default_set(default_profiles()), Library::empty());
    let pat = Pattern::empty_melodic(16);
    // Chords → CHORDS lane (index 2), synth → SYNTH lane (index 3), by role not index.
    assert_eq!(
        app.target_lane_for(&vendored("chords", "amapiano", "Jazzy Keys"), &pat),
        Some(2)
    );
    assert_eq!(
        app.target_lane_for(&vendored("synth", "techno", "x"), &pat),
        Some(3)
    );
    assert_eq!(app.lane_for_role(LibRole::Chords), Some(2));
    assert_eq!(app.lane_for_role(LibRole::Synth), Some(3));
}

#[test]
fn chords_ref_into_three_lane_set_reports_no_lane() {
    // A set with no CHORDS lane: targeting a chords ref must return None (the caller
    // reports a clear status) rather than clamping to an unrelated lane.
    let set = Set::default_set(vec![
        (LibRole::Drums, T8_DRUMS),
        (LibRole::Bass, T8_BASS),
        (LibRole::Synth, S1),
    ]);
    let app = App::new(set, Library::empty());
    let pat = Pattern::empty_melodic(16);
    assert_eq!(
        app.target_lane_for(&vendored("chords", "amapiano", "Jazzy Keys"), &pat),
        None,
        "no CHORDS lane → no target (clear status, not a clamp)"
    );
    // The old SYNTH lane still resolves for synth refs (index 2 here).
    assert_eq!(
        app.target_lane_for(&vendored("synth", "techno", "x"), &pat),
        Some(2),
        "old three-lane sets still audition/load synth patterns into their SYNTH lane"
    );
}

// ── Cross-role reference resolution (synth → chords migration bridge) ────────

#[test]
fn old_synth_ref_to_moved_chord_pattern_still_resolves() {
    let l = lib();
    // "Jazzy Keys" was reclassified synth → chords. A saved ref with the OLD synth
    // role must still resolve (via find_aliased's cross-role bridge).
    assert!(
        l.find("chords", "amapiano", "Jazzy Keys").is_some(),
        "the pattern now lives under the chords role"
    );
    assert!(
        l.find_aliased("synth", "amapiano", "Jazzy Keys").is_some(),
        "an old synth-role reference to it must still resolve"
    );
}

// ── CHORDS factory-library quality invariants ───────────────────────────────

#[test]
fn every_factory_chord_step_has_at_most_four_notes_and_a_real_chord() {
    let l = lib();
    assert!(!l.chords.is_empty(), "the chords role ships patterns");
    let root = J6.root_note as i32; // apply the J-6 root when checking pitch validity
    for (genre, pats) in &l.chords {
        for p in pats {
            let PatternData::Melodic(steps) = &p.data else {
                panic!("chords pattern {genre}/{} must be melodic", p.name);
            };
            let mut has_chord = false;
            for (i, step) in steps.iter().enumerate() {
                assert!(
                    step.len() <= 4,
                    "chords {genre}/{} step {i} has {} notes (>4 exceeds the J-6)",
                    p.name,
                    step.len()
                );
                if step.len() >= 2 {
                    has_chord = true;
                }
                for n in step.iter() {
                    let pitch = root + n.semi as i32;
                    assert!(
                        (0..=127).contains(&pitch),
                        "chords {genre}/{} step {i}: pitch {pitch} out of MIDI range",
                        p.name
                    );
                }
            }
            assert!(
                has_chord,
                "chords {genre}/{} must contain at least one real chord (2+ notes)",
                p.name
            );
            assert!(
                (16..=64).contains(&p.length),
                "chords {genre}/{} length {} out of 16..=64",
                p.name,
                p.length
            );
        }
    }
}

#[test]
fn chords_factory_ids_are_unique() {
    let l = lib();
    let ids: Vec<&str> = l
        .v2_index
        .ids
        .iter()
        .filter(|e| e.role == LibRole::Chords)
        .map(|e| e.factory_id.as_str())
        .collect();
    assert!(!ids.is_empty(), "chords patterns carry factory ids");
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "chords factory ids must be unique");
}

#[test]
fn chords_are_excluded_from_synth_results_and_mono_synth_survives() {
    use midip::pattern::index::Query;
    use midip::pattern::store::Favorites;
    let l = lib();
    let favs = Favorites::default();

    // A synth-role query must return ONLY synth records — never a chords pattern.
    let mut q = Query::default();
    q.role = Some(LibRole::Synth);
    let synth_hits = l.query(&q, &favs);
    assert!(
        synth_hits.iter().all(|r| r.role == LibRole::Synth),
        "synth-only results must not include chords-role patterns"
    );
    assert!(
        !synth_hits.is_empty(),
        "monophonic synth patterns remain available under the SYNTH role"
    );

    // And the chords role has its own results.
    let mut qc = Query::default();
    qc.role = Some(LibRole::Chords);
    assert!(
        l.query(&qc, &favs).iter().all(|r| r.role == LibRole::Chords),
        "chords query returns only chords-role patterns"
    );
    assert!(!l.query(&qc, &favs).is_empty(), "chords role has patterns");
}
