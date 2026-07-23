//! Compatibility, validation and round-trip tests for the versioned factory
//! pattern format **v2** (`src/pattern/format_v2.rs`).
//!
//! Covers: parsing the three representative fixtures (drums / mono bass / poly
//! synth) into the runtime `Pattern` model; schema-version and validation rejection
//! with actionable messages; JSON round-trips; deterministic legacy→v2 conversion;
//! additive library merge with stable-id lookup; alias-based resolution for renamed
//! patterns; name-collision skipping; and the guarantee that a purely-legacy library
//! (no v2 dir) is unchanged.

use std::path::{Path, PathBuf};

use midip::pattern::format_v2::{self, parse_pattern_v2, to_v2_json, LoadedV2};
use midip::pattern::library::{LibRole, Library};
use midip::pattern::model::{PatternData, TrigCond};
use midip::pattern::refs::{resolve_pattern_ref, PatternRef};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pattern-v2")
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_dir().join(name)).expect("read fixture")
}

fn parse_fixture(name: &str) -> LoadedV2 {
    parse_pattern_v2(&read_fixture(name), name).expect("fixture parses")
}

// ── parsing the representative fixtures ─────────────────────────────────────

#[test]
fn drums_fixture_parses_into_model() {
    let l = parse_fixture("drums-techno-four-on-floor.json");
    assert_eq!(l.role, LibRole::Drums);
    assert_eq!(l.genre, "techno");
    assert_eq!(l.factory_id, "drums.techno.four-on-floor-v2");
    assert_eq!(l.pattern.length, 8);
    let PatternData::Drums(steps) = &l.pattern.data else {
        panic!("expected drums");
    };
    // Chord step (kick + hat simultaneously).
    assert_eq!(steps[2].len(), 2);
    // Ratchet carried through.
    assert_eq!(steps[3][0].ratchet, 4);
    // Micro offset.
    assert_eq!(steps[1][0].micro, -6);
    // Fill trig condition.
    assert_eq!(steps[7][0].cond, TrigCond::Fill);
    // Per-step CC lock lands on step 0, length-synced to 8.
    assert_eq!(l.pattern.cc.len(), 8);
    assert_eq!(
        l.pattern.step_cc(0),
        &[midip::pattern::model::CcLock { cc: 74, val: 64 }]
    );
    // Absent fields defaulted.
    assert_eq!(steps[0][0].prob, 1.0);
    assert_eq!(steps[0][0].cond, TrigCond::Always);
}

#[test]
fn mono_bass_fixture_parses_into_model() {
    let l = parse_fixture("bass-techno-octave-drive.json");
    assert_eq!(l.role, LibRole::Bass);
    let PatternData::Melodic(steps) = &l.pattern.data else {
        panic!("expected melodic");
    };
    // Rests map to empty steps; no step holds more than one note (mono).
    assert!(steps[1].is_empty());
    assert!(steps.iter().all(|s| s.len() <= 1));
    // Slide + explicit len.
    assert!(steps[2][0].slide);
    assert_eq!(steps[2][0].len, 1.0);
    // prob<1 and Ratio cond preserved.
    assert_eq!(steps[3][0].prob, 0.8);
    assert_eq!(steps[6][0].cond, TrigCond::Ratio { x: 1, y: 2 });
    // Alias recorded.
    assert_eq!(l.aliases, vec!["Octave Pulse V2 (old)".to_string()]);
}

#[test]
fn poly_synth_fixture_parses_chord_step() {
    let l = parse_fixture("synth-house-rhodes-maj7.json");
    assert_eq!(l.role, LibRole::Synth);
    let PatternData::Melodic(steps) = &l.pattern.data else {
        panic!("expected melodic");
    };
    // A three-note maj7 chord on step 0.
    let chord: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
    assert_eq!(chord, vec![0, 4, 7]);
    assert_eq!(steps[0][1].micro, -8);
    assert_eq!(steps[0][2].ratchet, 2);
    assert_eq!(steps[0][2].cond, TrigCond::Ratio { x: 1, y: 2 });
    assert_eq!(l.pattern.step_cc(0).len(), 1);
    // Metadata + provenance carried on the load record (not the model).
    assert_eq!(
        l.metadata.get("voicing").and_then(|v| v.as_str()),
        Some("maj7")
    );
    assert!(l.provenance.is_some());
}

// ── rejection / validation with actionable errors ───────────────────────────

fn expect_err_contains(json: &str, needle: &str) {
    let err = parse_pattern_v2(json, "test.json").expect_err("should reject");
    let msg = format!("{err}");
    assert!(
        msg.contains(needle),
        "error {msg:?} should mention {needle:?}"
    );
}

#[test]
fn rejects_unknown_schema() {
    expect_err_contains(
        r#"{"schema":"other","version":2,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[]]}"#,
        "unknown schema",
    );
}

#[test]
fn rejects_newer_version_clearly() {
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":99,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[]]}"#,
        "newer midip",
    );
}

#[test]
fn rejects_unknown_field() {
    // deny_unknown_fields → a stray top-level key is an error, not silent loss.
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[]],"bogus":1}"#,
        "not a valid v2 pattern",
    );
}

#[test]
fn rejects_role_kind_mismatch() {
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"a.b.c","role":"bass","kind":"drums","genre":"g","name":"n","length":1,"steps":[[]]}"#,
        "kind 'drums' is only valid for role 'drums'",
    );
}

#[test]
fn rejects_length_mismatch() {
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":4,"steps":[[]]}"#,
        "declared length 4 != step count 1",
    );
}

#[test]
fn rejects_out_of_range_velocity_with_context() {
    // Drum vel must be 1..=127; 0 is rejected with step/field context.
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[{"note":36,"vel":0}]]}"#,
        "vel 0 must be 1..=127",
    );
}

#[test]
fn rejects_bad_factory_id() {
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"Bad ID!","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[{"note":36,"vel":100}]]}"#,
        "factory_id",
    );
}

#[test]
fn rejects_bad_ratio_cond() {
    expect_err_contains(
        r#"{"schema":"midip.pattern","version":2,"factory_id":"a.b.c","role":"drums","kind":"drums","genre":"g","name":"n","length":1,"steps":[[{"note":36,"vel":100,"cond":{"type":"Ratio","x":3,"y":2}}]]}"#,
        "1<=x<=y",
    );
}

// ── JSON round-trip and deterministic conversion ────────────────────────────

#[test]
fn v2_round_trips_through_json() {
    for name in [
        "drums-techno-four-on-floor.json",
        "bass-techno-octave-drive.json",
        "synth-house-rhodes-maj7.json",
    ] {
        let first = parse_fixture(name);
        let json = to_v2_json(first.role, &first.genre, &first.pattern).expect("to v2 json");
        let second = parse_pattern_v2(&json, "roundtrip").expect("reparse");
        // The musical model must survive a full serialize→parse cycle.
        assert_eq!(first.pattern, second.pattern, "round-trip changed {name}");
    }
}

#[test]
fn legacy_conversion_is_deterministic_and_reparsable() {
    // Convert a real legacy factory pattern to v2 and prove it is stable + valid.
    let lib = Library::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns"))
        .expect("library loads");
    let pat = lib
        .find("drums", "techno", "Four on Floor")
        .expect("known pattern");

    let a = to_v2_json(LibRole::Drums, "techno", pat).unwrap();
    let b = to_v2_json(LibRole::Drums, "techno", pat).unwrap();
    assert_eq!(
        a, b,
        "conversion must be byte-identical for identical input"
    );

    let loaded = parse_pattern_v2(&a, "converted").expect("converted v2 parses");
    assert_eq!(loaded.factory_id, "drums.techno.four-on-floor");
    // The converted pattern equals the legacy one (id is nil in both; cc length-synced).
    assert_eq!(loaded.pattern.name, pat.name);
    assert_eq!(loaded.pattern.data, pat.data);
    assert_eq!(loaded.pattern.length, pat.length);
}

// ── additive library merge + stable-id + alias resolution ───────────────────

#[test]
fn load_v2_dir_reads_all_fixtures() {
    let (loaded, warnings) = format_v2::load_v2_dir(&fixture_dir());
    assert_eq!(loaded.len(), 3, "three fixtures");
    assert!(warnings.is_empty(), "no warnings: {warnings:?}");
}

#[test]
fn merge_v2_is_additive_and_registers_ids() {
    let (loaded, _) = format_v2::load_v2_dir(&fixture_dir());
    let mut lib = Library::empty();
    let warnings = lib.merge_v2(loaded);
    assert!(warnings.is_empty(), "clean merge: {warnings:?}");

    // Patterns appear in their genre maps.
    assert!(lib.find("drums", "techno", "Four on Floor V2").is_some());
    assert!(lib.find("synth", "house", "Rhodes Maj7 V2").is_some());

    // Stable-id lookups both directions.
    assert_eq!(
        lib.factory_id_of(LibRole::Synth, "house", "Rhodes Maj7 V2"),
        Some("synth.house.rhodes-maj7-v2")
    );
    let (role, genre, name) = lib
        .resolve_factory_id("bass.techno.octave-drive-v2")
        .unwrap();
    assert_eq!(
        (role, genre, name),
        (LibRole::Bass, "techno", "Octave Drive V2")
    );
}

#[test]
fn alias_lets_old_name_resolve_after_rename() {
    let (loaded, _) = format_v2::load_v2_dir(&fixture_dir());
    let mut lib = Library::empty();
    lib.merge_v2(loaded);

    // The bass fixture renamed "Octave Pulse V2 (old)" → "Octave Drive V2".
    // A saved PatternRef using the OLD name must still resolve via the alias map.
    let old_ref = PatternRef::Vendored {
        role: "bass".into(),
        genre: "techno".into(),
        name: "Octave Pulse V2 (old)".into(),
    };
    let user_dir = std::env::temp_dir().join("midip-v2-alias-none");
    let resolved = resolve_pattern_ref(&old_ref, &lib, &user_dir);
    assert!(resolved.is_some(), "old name must resolve through alias");
    assert_eq!(resolved.unwrap().name, "Octave Drive V2");

    // Direct (non-alias) lookup of the old name still misses.
    assert!(lib
        .find("bass", "techno", "Octave Pulse V2 (old)")
        .is_none());
    assert!(lib
        .find_aliased("bass", "techno", "Octave Pulse V2 (old)")
        .is_some());
}

#[test]
fn merge_v2_skips_name_collision() {
    let (loaded, _) = format_v2::load_v2_dir(&fixture_dir());
    let mut lib = Library::empty();
    lib.merge_v2(loaded.clone());
    // Merging the same set again must skip every colliding name (never duplicate).
    let warnings = lib.merge_v2(loaded);
    assert_eq!(warnings.len(), 3, "all three collide on second merge");
    assert!(warnings.iter().all(|w| w.contains("already exists")));
    assert_eq!(
        lib.find("drums", "techno", "Four on Floor V2")
            .into_iter()
            .count(),
        1
    );
    assert_eq!(
        lib.drums["techno"]
            .iter()
            .filter(|p| p.name == "Four on Floor V2")
            .count(),
        1
    );
}

// ── legacy library stays intact alongside shipped v2 packs ──────────────────

#[test]
fn shipped_v2_packs_load_and_legacy_intact() {
    let lib = Library::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns"))
        .expect("library loads");
    // Phase 6 ships v2 genre packs under assets/patterns/v2 → registry populated.
    assert!(
        !lib.v2_index.ids.is_empty(),
        "shipped v2 genre packs must register factory ids"
    );
    // The 20 legacy genres are still present (packs are additive, never replace).
    for g in ["techno", "house", "trance", "dub-techno", "acid-techno"] {
        assert!(
            lib.genres(LibRole::Drums).contains(&g),
            "legacy genre {g} kept"
        );
    }
    assert!(lib.genres(LibRole::Drums).len() >= 20);
    // A legacy vendored ref still resolves.
    let r = PatternRef::Vendored {
        role: "drums".into(),
        genre: "techno".into(),
        name: "Four on Floor".into(),
    };
    let user_dir = std::env::temp_dir().join("midip-v2-legacy-none");
    assert!(resolve_pattern_ref(&r, &lib, &user_dir).is_some());
}
