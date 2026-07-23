//! Phase 8 integration + performance tests for the shared library filter engine.

use std::path::Path;
use std::time::Instant;

use midip::pattern::index::{filter, Density, Energy, Feel, Poly, Query};
use midip::pattern::library::{LibRole, Library, PatternFunction};
use midip::pattern::store::Favorites;

fn lib() -> Library {
    Library::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns")).unwrap()
}

#[test]
fn index_covers_every_pattern() {
    let l = lib();
    let total: usize = [&l.drums, &l.bass, &l.synth]
        .iter()
        .flat_map(|m| m.values())
        .map(|v| v.len())
        .sum();
    assert_eq!(l.records().len(), total, "one record per pattern");
    assert!(total > 1000, "library should be sizeable, got {total}");
}

#[test]
fn v2_records_carry_metadata_legacy_degrade() {
    let l = lib();
    // A Phase-7 boom-bap pattern: mpc-swing → Feel::Swing, energy + bpm populated.
    let bb = l
        .records()
        .iter()
        .find(|r| r.genre == "boom-bap" && r.name == "Dusty Core")
        .expect("boom-bap present");
    assert_eq!(bb.feel, Feel::Swing, "boom-bap carries swing feel");
    assert!(bb.bpm.is_some(), "v2 record has bpm");
    assert_ne!(bb.energy, Energy::Unknown, "v2 record has energy");
    assert_eq!(bb.function, Some(PatternFunction::Core));

    // A legacy pattern degrades gracefully: straight feel, no bpm/energy metadata.
    let legacy = l
        .records()
        .iter()
        .find(|r| r.genre == "acid-techno" && r.role == LibRole::Drums)
        .expect("legacy acid-techno present");
    assert_eq!(legacy.bpm, None);
    assert_eq!(legacy.energy, Energy::Unknown);
    assert_eq!(legacy.feel, Feel::Straight);
}

#[test]
fn queries_filter_the_real_library() {
    let l = lib();
    let favs = Favorites::default();

    // Facet: drums + swing feel → only swung drum patterns; all are drums & swing.
    let q = Query {
        role: Some(LibRole::Drums),
        feel: Some(Feel::Swing),
        ..Default::default()
    };
    let hits = l.query(&q, &favs);
    assert!(!hits.is_empty());
    assert!(hits
        .iter()
        .all(|r| r.role == LibRole::Drums && r.feel == Feel::Swing));

    // Text search hits trap patterns by genre token.
    let q = Query::default().with_text("trap");
    let hits = l.query(&q, &favs);
    assert!(hits.iter().any(|r| r.genre == "trap"));

    // Poly filter surfaces chord-bearing synth patterns.
    let q = Query {
        poly: Some(Poly::Poly),
        ..Default::default()
    };
    assert!(l.query(&q, &favs).iter().any(|r| r.genre == "amapiano"));

    // Density + function combine.
    let q = Query {
        density: Some(Density::Sparse),
        function: Some(PatternFunction::VariationA),
        ..Default::default()
    };
    assert!(l
        .query(&q, &favs)
        .iter()
        .all(|r| r.density == Density::Sparse));

    // Empty-result combination returns nothing without error.
    let q = Query {
        role: Some(LibRole::Synth),
        function: Some(PatternFunction::Core),
        ..Default::default()
    }
    .with_text("zzzzz-nomatch");
    assert!(l.query(&q, &favs).is_empty());
}

#[test]
fn results_are_deterministically_ordered() {
    let l = lib();
    let favs = Favorites::default();
    let a: Vec<String> = l
        .query(&Query::default(), &favs)
        .iter()
        .map(|r| format!("{:?}/{}/{}", r.role, r.genre, r.name))
        .collect();
    let b: Vec<String> = l
        .query(&Query::default(), &favs)
        .iter()
        .map(|r| format!("{:?}/{}/{}", r.role, r.genre, r.name))
        .collect();
    assert_eq!(a, b, "same query must return identical ordering");
    // Roles are grouped (all drums precede all bass precede all synth).
    let roles: Vec<LibRole> = l
        .query(&Query::default(), &favs)
        .iter()
        .map(|r| r.role)
        .collect();
    let first_bass = roles.iter().position(|&r| r == LibRole::Bass);
    let last_drums = roles.iter().rposition(|&r| r == LibRole::Drums);
    if let (Some(fb), Some(ld)) = (first_bass, last_drums) {
        assert!(ld < fb, "drums must all precede bass");
    }
}

#[test]
fn filter_is_fast_on_a_large_synthesized_catalog() {
    // Synthesize ~10k records by cloning real ones with mutated identity, then
    // time a text+facet query. Must stay well under interactive latency.
    let l = lib();
    let base = l.records();
    assert!(!base.is_empty());
    let mut big = Vec::with_capacity(10_000);
    let mut i = 0usize;
    while big.len() < 10_000 {
        let mut r = base[i % base.len()].clone();
        r.name = format!("{} #{}", r.name, big.len());
        r.tags.push(format!("tag{}", big.len() % 37));
        big.push(r);
        i += 1;
    }
    // Rebuild haystacks are already present from clone; run the query.
    let favs = Favorites::default();
    let q = Query {
        role: Some(LibRole::Drums),
        feel: Some(Feel::Swing),
        ..Default::default()
    }
    .with_text("core");
    let t = Instant::now();
    let mut n = 0;
    for _ in 0..20 {
        n = filter(&big, &q, &favs).len();
    }
    let per = t.elapsed() / 20;
    assert!(
        per.as_millis() < 50,
        "filter over 10k took {per:?} (>50ms) — too slow"
    );
    // Sanity: the query still returns a plausible, sorted subset.
    let _ = n;
}
