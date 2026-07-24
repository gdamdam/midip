//! Phase 9: multi-bar / alternate-meter / polymeter compatibility tests.
//! Persistence round-trips retain meter + arbitrary pattern length, and old sets
//! (no `steps_per_bar`) load as 4/4.

use std::path::PathBuf;

use midip::devices::profiles::default_profiles;
use midip::pattern::model::{Pattern, PatternData, Set};
use midip::pattern::store::{load_set, save_set};

fn tmp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("midip-meter-{tag}-{nanos}"));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn set_meter_and_long_odd_lengths_round_trip() {
    let dir = tmp("roundtrip");
    let mut set = Set::default_set(default_profiles());
    set.steps_per_bar = 12; // 3/4 transport
                            // A 32-step (2-bar) drum pattern and a 20-step (5/4) pattern.
    set.lanes[0].pattern = Pattern::empty_drums(32);
    set.lanes[1].pattern = Pattern::empty_melodic(20);
    set.lanes[2].pattern = Pattern::empty_melodic(48);

    let path = save_set(&dir, &mut set).unwrap();
    let back = load_set(&path).unwrap();

    assert_eq!(back.steps_per_bar, 12, "meter must survive save/load");
    assert_eq!(back.lanes[0].pattern.length, 32, "2-bar length preserved");
    assert_eq!(back.lanes[1].pattern.length, 20, "5/4 length preserved");
    assert_eq!(back.lanes[2].pattern.length, 48, "3-bar length preserved");
    // Data vectors are length-synced.
    if let PatternData::Drums(s) = &back.lanes[0].pattern.data {
        assert_eq!(s.len(), 32);
    } else {
        panic!("drums");
    }
}

#[test]
fn old_set_without_steps_per_bar_loads_as_4_4() {
    // A pre-Phase-9 set file has no `steps_per_bar` key → serde default 16.
    let dir = tmp("legacy");
    let json = r#"{
        "version": 4,
        "id": "00000000000000a1",
        "name": "legacy",
        "bpm": 120.0,
        "swing": 0.5,
        "lanes": [
          {"profile_id":"t8-drums","pattern":{"name":"k","length":16,"data":{"Drums":[[],[],[],[],[],[],[],[],[],[],[],[],[],[],[],[]]}},"mute":false,"solo":false,"transpose":0,"octave":0}
        ]
    }"#;
    let path = dir.join("legacy-00000000000000a1.json");
    std::fs::write(&path, json).unwrap();
    let set = load_set(&path).unwrap();
    assert_eq!(set.steps_per_bar, 16, "missing meter defaults to 4/4");
}

#[test]
fn shipped_library_has_multibar_and_alt_meter_content() {
    use midip::pattern::library::{LibRole, Library};
    let lib =
        Library::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns"))
            .unwrap();
    // New alt-meter genres are present and carry non-16 lengths.
    let by = |role: LibRole, genre: &str| -> Vec<usize> {
        let m = match role {
            LibRole::Drums => &lib.drums,
            LibRole::Bass => &lib.bass,
            LibRole::Chords => &lib.chords,
            LibRole::Synth => &lib.synth,
        };
        m.get(genre)
            .map(|v| v.iter().map(|p| p.length).collect())
            .unwrap_or_default()
    };
    assert!(
        by(LibRole::Drums, "waltz").contains(&12),
        "3/4 waltz (12-step) shipped"
    );
    assert!(
        by(LibRole::Drums, "five-four").contains(&20),
        "5/4 (20-step) shipped"
    );
    assert!(by(LibRole::Drums, "polymeter")
        .iter()
        .any(|&l| l == 12 || l == 15 || l == 20));
    // Two-bar (32-step) drum content exists in at least a few genres.
    let two_bar_genres = lib
        .drums
        .iter()
        .filter(|(_, v)| v.iter().any(|p| p.length == 32))
        .count();
    assert!(
        two_bar_genres >= 5,
        "several genres have 2-bar patterns, got {two_bar_genres}"
    );
    // 4-bar (64-step) melodic phrases exist. The chordal 4-bar progressions now
    // ship under the polyphonic `chords` role (reclassified from `synth`).
    assert!(
        lib.chords.values().flatten().any(|p| p.length == 64),
        "4-bar chords phrase shipped"
    );
    // Every record still indexed (search works over long/odd patterns).
    assert_eq!(
        lib.records().len(),
        // chords is a v2-only role but is still indexed into `records()`, so include
        // its in-memory map here to keep the count identity true.
        [&lib.drums, &lib.bass, &lib.chords, &lib.synth]
            .iter()
            .flat_map(|m| m.values())
            .map(|v| v.len())
            .sum::<usize>()
    );
}

#[test]
fn v2_alt_meter_pattern_parses_and_indexes_meter() {
    // A v2 factory pattern may declare meter/steps_per_bar in its free-form metadata
    // (no schema change); the length carries the real step count.
    let json = r#"{
      "schema":"midip.pattern","version":2,"factory_id":"drums.waltz.three-four",
      "role":"drums","kind":"drums","genre":"waltz","name":"Waltz Core","length":12,
      "steps":[
        [{"note":36,"vel":110}],[],[],[],
        [{"note":38,"vel":100}],[],[],[],
        [{"note":38,"vel":100}],[],[],[]
      ],
      "metadata":{"meter":"3/4","steps_per_bar":12,"bars":1},
      "provenance":{"source":"factory"}
    }"#;
    let loaded = midip::pattern::format_v2::parse_pattern_v2(json, "waltz").unwrap();
    assert_eq!(loaded.pattern.length, 12);
    assert_eq!(
        loaded.metadata.get("meter").and_then(|v| v.as_str()),
        Some("3/4")
    );
}
