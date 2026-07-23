//! Phase 7 semantic timing lint.
//!
//! Guarantees that a v2 pattern's timing CLAIM is backed by real encoded
//! microtiming: every note's baked `micro` must equal the offset its declared
//! `metadata.timing` template produces (`pattern::timing`), a "straight" pattern
//! must carry no microtiming, and a `feel`/`desc` that names a groove must not sit
//! on a straight pattern. This makes "swing"/"laid-back"/"triplet" claims
//! non-fakeable — they can only appear when the data actually encodes them.

use std::path::Path;

use midip::pattern::format_v2::parse_pattern_v2;
use midip::pattern::model::PatternData;
use midip::pattern::timing::{offset_permille, Timing};

/// Must match `tools/packgen/helpers.py` SEED.
const SEED: u32 = 1337;

fn v2_files() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns/v2");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .expect("v2 dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    v.sort();
    v
}

fn micros_by_step(data: &PatternData) -> Vec<Vec<i16>> {
    match data {
        PatternData::Drums(steps) => steps
            .iter()
            .map(|s| s.iter().map(|h| h.micro).collect())
            .collect(),
        PatternData::Melodic(steps) => steps
            .iter()
            .map(|s| s.iter().map(|n| n.micro).collect())
            .collect(),
    }
}

#[test]
fn v2_microtiming_matches_declared_timing_template() {
    let mut checked = 0usize;
    let mut with_timing = 0usize;
    for path in v2_files() {
        let json = std::fs::read_to_string(&path).unwrap();
        let label = path.file_name().unwrap().to_string_lossy().to_string();
        let loaded = parse_pattern_v2(&json, &label).expect("v2 parses");

        let timing_name = loaded
            .metadata
            .get("timing")
            .and_then(|v| v.as_str())
            .unwrap_or("straight");
        let t = Timing::parse(timing_name)
            .unwrap_or_else(|| panic!("{label}: unknown timing template {timing_name:?}"));
        if !t.is_straight() {
            with_timing += 1;
        }

        // The real guarantee: every baked `micro` equals what the declared template
        // produces for that step. A mismatch (incl. a forgotten bake showing 0 where
        // the template is non-zero) fails here. Patterns that only hit on-grid
        // downbeats legitimately carry zero offset under a swing template.
        let per_step = micros_by_step(&loaded.pattern.data);
        for (i, micros) in per_step.iter().enumerate() {
            let expected = offset_permille(t, i, SEED);
            for &got in micros {
                assert_eq!(
                    got, expected,
                    "{label}: step {i} micro {got} != template {timing_name} expected {expected}"
                );
            }
        }
        checked += 1;
    }
    assert!(
        checked >= 100,
        "expected the full v2 set, checked {checked}"
    );
    assert!(
        with_timing >= 20,
        "expected many timing-upgraded patterns, got {with_timing}"
    );
}

#[test]
fn v2_feel_claims_require_a_nonstraight_timing_template() {
    // If the human-readable `feel` names a groove, the `timing` template must be
    // non-straight (which the test above then proves is actually encoded).
    const GROOVE_WORDS: &[&str] = &[
        "swing",
        "swung",
        "shuffle",
        "triplet",
        "laid-back",
        "pushed",
        "humaniz",
    ];
    for path in v2_files() {
        let json = std::fs::read_to_string(&path).unwrap();
        let label = path.file_name().unwrap().to_string_lossy().to_string();
        let loaded = parse_pattern_v2(&json, &label).unwrap();
        let feel = loaded
            .metadata
            .get("feel")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let timing = loaded
            .metadata
            .get("timing")
            .and_then(|v| v.as_str())
            .unwrap_or("straight");
        if GROOVE_WORDS.iter().any(|w| feel.contains(w)) {
            assert_ne!(
                timing, "straight",
                "{label}: feel {feel:?} claims a groove but timing is straight"
            );
        }
    }
}
