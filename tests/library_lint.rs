//! Deterministic lint over the vendored factory pattern catalog.
//!
//! Phase 1 of the pattern-library improvement work. This guards data hygiene
//! without changing musical behaviour: it never mutates patterns, it only
//! asserts invariants over `assets/patterns/{catalog.json, patterns-*.json}`.
//!
//! Checks:
//!   1. catalog genre/count alignment with the data files
//!   2. unique pattern names within each role+genre (identity is role+genre+name)
//!   3. no empty descriptions
//!   4. descriptions do not claim features the mono / straight-16 / no-automation
//!      data cannot encode (chords, swing, triplets, stutter/ratchet, probability,
//!      CC/LFO, sustain), except a small justified allowlist of representable uses
//!   5. exact-duplicate pattern content is confined to a documented allowlist
//!      (vendored cross-genre aliases kept for saved-`PatternRef` stability)
//!   6. duplicate descriptions confined to a documented allowlist
//!   7. valid note/velocity/semitone ranges
//!   8. pattern length/data consistency (all 16 steps)
//!
//! Known failure examples are exercised by the unit tests at the bottom.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use midip::devices::profiles::{S1, T8_BASS};
use midip::pattern::library::{parse_catalog, parse_drum_file, parse_melodic_file, LibRole};
use midip::pattern::model::{Pattern, PatternData};

// The `chords` role is deliberately absent here: this lint operates over the
// legacy `patterns-*.json` data files, and chords is a v2-only role with no
// legacy file to lint. Its in-memory content is covered by other tests.
const ROLES: [(&str, LibRole); 3] = [
    ("drums", LibRole::Drums),
    ("bass", LibRole::Bass),
    ("synth", LibRole::Synth),
];

/// Substrings (lowercased) that claim a feature the vendored data cannot encode.
const BANNED_CLAIMS: &[&str] = &[
    // simultaneity — melodic data is monophonic
    "chord",
    "triad",
    "voicing",
    "dyad",
    // duration — note length is a fixed authoring gate
    "sustain",
    "drone",
    "whole note",
    "pedal tone",
    // timing — the legacy grid is straight 16ths with no per-note microtiming
    // (micro is forced to 0 by the loader). Pattern-authored feel lives only in
    // v2 patterns via timing templates (Phase 7); legacy descriptions may not
    // claim a groove they cannot encode.
    "swing",
    "swung",
    "shuffle",
    "microtim",
    "behind the beat",
    "ahead of the beat",
    "laid-back",
    "laid back",
    "layback",
    "off-grid",
    "off grid",
    "mpc groove",
    "mpc energy",
    "lazy timing",
    // subdivision — no triplets/polyrhythm
    "triplet",
    "polyrhythm",
    "polymeter",
    // non-determinism — patterns are fixed
    "probab",
    "random",
    "generative",
    "unpredictab",
    "stochastic",
    // sub-step retrigger — no ratchet/stutter/flam
    "stutter",
    "ratchet",
    "machine-gun",
    "retrigger",
    "flam",
    // parameter automation — no CC/LFO
    "lfo",
    "filter sweep",
    "automat",
    "morph",
];

/// Chord- and duration-family terms are checked against the DATA (see below): a
/// description may use them when the pattern actually contains a chord step / a
/// sustained note. All other banned terms are never representable.
const CHORD_TERMS: &[&str] = &["chord", "triad", "voicing", "dyad"];
const DURATION_TERMS: &[&str] = &["sustain", "drone", "whole note", "pedal tone"];

/// (role, genre, name) whose description contains a chord/duration term but is
/// MONO/short in the data yet still musically accurate — arpeggiated/sequential
/// chord tones and single-note basslines that walk chord tones, plus one
/// description that explicitly *negates* swing. (Patterns upgraded to real chords
/// in Phase 2 are NOT listed here — they pass via the content-aware check.)
const ALLOW_CLAIM: &[(&str, &str, &str)] = &[
    ("synth", "acid-techno", "303 Minor"), // "minor triad arpeggio" (sequential)
    ("synth", "dub-techno", "Dub Minor"),  // chord-symbol progression, single notes
    ("synth", "edm", "Chord Pluck"),       // "arpeggiated chord tones"
    ("synth", "dubstep", "Broken Chords"), // "arpeggiated minor triad"
    ("synth", "synthwave", "Triad Arp"),   // arpeggiated triad
    ("synth", "psytrance", "Triad Arp"),   // arpeggiated triad
    ("drums", "electro", "Robot"),         // "zero swing, pure grid" (negation)
    ("bass", "dubstep", "Minor Movement"), // single-note bassline over chord tones
    ("bass", "lo-fi", "Root and Third"),   // chord-tone bass (sequential)
    ("bass", "synthwave", "Chord Tones"),  // chord-tone bass (sequential)
    ("bass", "deep-house", "Simple Move"), // chord movement, single notes
    ("bass", "psytrance", "Chord Tone Roll"), // rolling single-note chord tones
];

/// (role, genre, name) whose content is byte-identical to an earlier pattern in
/// the same role. These are intentional vendored aliases: identity is
/// role+genre+name, so removing them would break saved references. Kept as-is
/// (behaviour-preserving); documented here so NEW duplicates fail the lint.
const ALLOW_DUP: &[(&str, &str, &str)] = &[
    ("drums", "edm", "Festival"),
    ("drums", "house", "Classic"),
    ("drums", "breakbeat", "Half-Time"),
    ("bass", "trance", "Trance Quarter"),
    ("bass", "dub-techno", "Deep One Hit"),
    ("bass", "edm", "Sub Hit"),
    ("bass", "house", "Minimal Pulse"),
    ("bass", "house", "Dotted Root"),
    ("bass", "garage", "Grime Bass"),
    ("bass", "ambient", "Slow Breath"),
    ("bass", "ambient", "Ebb"),
    ("bass", "glitch", "Deep Stutter"),
    ("bass", "electro", "B-Boy Sub"),
    ("bass", "electro", "808 Rumble"),
    ("bass", "electro", "Sub Pulse"),
    ("bass", "downtempo", "Massive Sub"),
    ("bass", "downtempo", "Swing Sub"),
    ("bass", "downtempo", "Quarter Walk"),
    ("bass", "dubstep", "Sparse Hits"),
    ("bass", "synthwave", "Power Bass"),
    ("bass", "deep-house", "Walking Groove"),
    ("bass", "deep-house", "Minor Walk"),
    ("bass", "psytrance", "Block Roll"),
    ("synth", "ambient", "Slow Breath"),
    ("synth", "lo-fi", "Whisper"),
    ("synth", "synthwave", "Octave Pulse"),
    ("synth", "deep-house", "Warm Pad"),
    ("synth", "deep-house", "Rhodes"),
    ("synth", "deep-house", "Gentle Pad"),
    ("synth", "deep-house", "Whisper Pad"),
    ("synth", "psytrance", "Triad Arp"),
];

/// Descriptions shared verbatim by more than one pattern. Benign — they describe
/// genuinely similar patterns across related genres. Documented so NEW duplicate
/// descriptions fail the lint.
const ALLOW_DUP_DESC: &[&str] = &[
    // drums
    "ride cymbal instead of hats",
    "ride cymbal groove",
    "clap instead of snare",
    "open hat on offbeats",
    "ride cymbal variation",
    "constant 16th hats",
    "open hat at bar end",
    // bass
    "Root\u{2013}5th alternating \u{2014} wide harmonic motion",
    "Half-note root pulse \u{2014} deep and patient",
    "Quarter-note root \u{2014} simple and solid",
    "Minor scale walk \u{2014} melodic bass",
    "Hip-hop style bass \u{2014} head-nodding weight",
    "root to fifth movement",
    "out and back to root",
    "syncopated bass rhythm",
];

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/patterns")
}

/// Canonical content key from the musical SOURCE fields only (ignores gate/prob/
/// ratchet defaults the parser injects), so identical source content collides.
fn content_key(p: &Pattern) -> String {
    let mut s = String::new();
    match &p.data {
        PatternData::Drums(steps) => {
            s.push('D');
            for step in steps {
                s.push('|');
                let mut hits: Vec<(u8, u8)> = step.iter().map(|h| (h.note, h.vel)).collect();
                hits.sort_unstable();
                for (n, v) in hits {
                    s.push_str(&format!("{n}:{v},"));
                }
            }
        }
        PatternData::Melodic(steps) => {
            s.push('M');
            for step in steps {
                s.push('|');
                for n in step.iter() {
                    s.push_str(&format!("{}:{:.3}:{},", n.semi, n.vel, n.slide as u8));
                }
            }
        }
    }
    s
}

#[test]
fn vendored_catalog_lint() {
    let dir = assets_dir();
    let catalog_json = std::fs::read_to_string(dir.join("catalog.json")).expect("read catalog");
    let allow_claim: HashSet<(&str, &str, &str)> = ALLOW_CLAIM.iter().copied().collect();
    let allow_dup: HashSet<(&str, &str, &str)> = ALLOW_DUP.iter().copied().collect();
    let allow_dup_desc: HashSet<&str> = ALLOW_DUP_DESC.iter().copied().collect();

    let mut v: Vec<String> = Vec::new();

    for (role, librole) in ROLES {
        // --- load data (content) + catalog (names/descs) for this role ---
        let (file, gate) = match librole {
            LibRole::Drums => ("patterns-t8-drums.json", 0.0),
            LibRole::Bass => ("patterns-t8-bass.json", T8_BASS.gate_fraction),
            LibRole::Synth => ("patterns-s1.json", S1.gate_fraction),
            // chords is not in ROLES (v2-only, no legacy data file) — unreachable here.
            LibRole::Chords => unreachable!("chords has no legacy patterns-*.json file"),
        };
        let data_json = std::fs::read_to_string(dir.join(file)).expect("read data");
        let data = if librole == LibRole::Drums {
            parse_drum_file(&data_json).expect("parse drums")
        } else {
            parse_melodic_file(&data_json, gate).expect("parse melodic")
        };
        let catalog: HashMap<String, Vec<(String, String)>> =
            parse_catalog(&catalog_json, librole).expect("parse catalog");

        // (1) genre set alignment
        let data_genres: HashSet<&str> = data.keys().map(|s| s.as_str()).collect();
        let cat_genres: HashSet<&str> = catalog.keys().map(|s| s.as_str()).collect();
        for g in data_genres.difference(&cat_genres) {
            v.push(format!("[{role}] genre '{g}' in data has no catalog entry"));
        }
        for g in cat_genres.difference(&data_genres) {
            v.push(format!("[{role}] catalog genre '{g}' has no data"));
        }

        let mut seen_content: HashMap<String, (String, String)> = HashMap::new();

        for (genre, pats) in &data {
            let entries = catalog.get(genre);
            // (1) per-genre count alignment
            if let Some(entries) = entries {
                if entries.len() != pats.len() {
                    v.push(format!(
                        "[{role}] genre '{genre}': catalog {} vs data {} patterns",
                        entries.len(),
                        pats.len()
                    ));
                }
            }

            let mut names_seen: HashSet<&str> = HashSet::new();

            for (i, pat) in pats.iter().enumerate() {
                let (name, desc): (&str, &str) = match entries.and_then(|e| e.get(i)) {
                    Some((n, d)) => (n.as_str(), d.as_str()),
                    None => (pat.name.as_str(), ""), // fallback (no catalog entry)
                };

                // (2) unique names within role+genre
                if !names_seen.insert(name) {
                    v.push(format!("[{role}] genre '{genre}': duplicate name '{name}'"));
                }

                // (3) empty descriptions
                if desc.trim().is_empty() {
                    v.push(format!("[{role}] {genre}/{name}: empty description"));
                }

                // (4) feature claims — CONTENT-AWARE. Chord terms are allowed when
                // the pattern has a step with >=2 simultaneous notes; duration terms
                // when a note is longer than the gate (len >= 2 steps). Timing /
                // subdivision / non-determinism / automation terms are never
                // representable. `ALLOW_CLAIM` covers mono-but-accurate legacy uses.
                let low = desc.to_lowercase();
                if !allow_claim.contains(&(role, genre.as_str(), name)) {
                    let (has_chord, has_sustain) = match &pat.data {
                        PatternData::Melodic(steps) => (
                            steps.iter().any(|s| s.len() >= 2),
                            steps.iter().flat_map(|s| s.iter()).any(|n| n.len >= 2.0),
                        ),
                        PatternData::Drums(_) => (false, false),
                    };
                    for term in BANNED_CLAIMS {
                        if low.contains(term) {
                            let backed = (CHORD_TERMS.contains(term) && has_chord)
                                || (DURATION_TERMS.contains(term) && has_sustain);
                            if !backed {
                                v.push(format!(
                                    "[{role}] {genre}/{name}: desc claims '{term}' not backed by data: {desc:?}"
                                ));
                            }
                        }
                    }
                }

                // (5) exact-duplicate content
                let key = content_key(pat);
                if let Some((pg, pn)) = seen_content.get(&key) {
                    if !allow_dup.contains(&(role, genre.as_str(), name)) {
                        v.push(format!(
                            "[{role}] {genre}/{name}: exact-duplicate content of {pg}/{pn} (not allowlisted)"
                        ));
                    }
                } else {
                    seen_content.insert(key, (genre.clone(), name.to_string()));
                }

                // (7) value ranges + (8) length/data consistency
                if pat.length != 16 {
                    v.push(format!(
                        "[{role}] {genre}/{name}: length {} != 16",
                        pat.length
                    ));
                }
                match &pat.data {
                    PatternData::Drums(steps) => {
                        if steps.len() != pat.length {
                            v.push(format!(
                                "[{role}] {genre}/{name}: steps {} != length {}",
                                steps.len(),
                                pat.length
                            ));
                        }
                        for h in steps.iter().flatten() {
                            if h.note == 0 || h.vel == 0 {
                                v.push(format!("[{role}] {genre}/{name}: drum note/vel out of range note={} vel={}", h.note, h.vel));
                            }
                        }
                    }
                    PatternData::Melodic(steps) => {
                        if steps.len() != pat.length {
                            v.push(format!(
                                "[{role}] {genre}/{name}: steps {} != length {}",
                                steps.len(),
                                pat.length
                            ));
                        }
                        for n in steps.iter().flat_map(|s| s.iter()) {
                            if !(-24..=36).contains(&(n.semi as i32)) {
                                v.push(format!(
                                    "[{role}] {genre}/{name}: semi {} out of range",
                                    n.semi
                                ));
                            }
                            if !(0.5..=1.3).contains(&n.vel) {
                                v.push(format!(
                                    "[{role}] {genre}/{name}: vel {} out of [0.5,1.3]",
                                    n.vel
                                ));
                            }
                        }
                    }
                }
            }
        }

        // (6) duplicate descriptions within the role
        let mut desc_seen: HashMap<String, String> = HashMap::new();
        for (genre, entries) in &catalog {
            for (name, desc) in entries {
                if let Some(prev) = desc_seen.get(desc) {
                    if !allow_dup_desc.contains(desc.as_str()) {
                        v.push(format!(
                            "[{role}] {genre}/{name}: duplicate description of {prev} (not allowlisted): {desc:?}"
                        ));
                    }
                } else {
                    desc_seen.insert(desc.clone(), format!("{genre}/{name}"));
                }
            }
        }
    }

    assert!(
        v.is_empty(),
        "vendored catalog lint found {} issue(s):\n{}",
        v.len(),
        v.join("\n")
    );
}

// --- known-failure examples: the lint's building blocks reject bad inputs ---

#[test]
fn banned_claim_terms_detect_unrepresentable_features() {
    // A representative slice of the banned vocabulary must be recognised.
    for term in [
        "chord", "swing", "triplet", "stutter", "random", "lfo", "sustain",
    ] {
        assert!(
            BANNED_CLAIMS.contains(&term),
            "'{term}' should be a banned unrepresentable-feature term"
        );
    }
}

/// Phase 3: the performance-family registry must be internally consistent.
/// Hard failures (assert): every member resolves to a real pattern, family ids
/// are globally unique, each family declares exactly one pattern per function,
/// each family has a Core, and no pattern is enrolled in more than one family.
/// Soft (reported, not failed): families missing some functions — allowed, since
/// not every genre has a natural Fill/Breakdown/Peak.
#[test]
fn family_registry_lint() {
    use midip::pattern::library::{Library, PatternFunction};

    let lib = Library::load(&assets_dir()).expect("load library");
    let families = lib.families();
    assert!(!families.is_empty(), "catalog should ship families");

    let mut errors: Vec<String> = Vec::new();
    let mut ids: HashSet<&str> = HashSet::new();
    // (role, genre, name) -> family id, to detect a pattern in >1 family.
    let mut enrolled: HashMap<(LibRole, &str, &str), &str> = HashMap::new();

    const ALL_FUNCS: [PatternFunction; 6] = [
        PatternFunction::Core,
        PatternFunction::VariationA,
        PatternFunction::VariationB,
        PatternFunction::Fill,
        PatternFunction::Breakdown,
        PatternFunction::Peak,
    ];

    for fam in families {
        if !ids.insert(fam.id.as_str()) {
            errors.push(format!("duplicate family id: {}", fam.id));
        }
        let role = match fam.role {
            LibRole::Drums => "drums",
            LibRole::Bass => "bass",
            LibRole::Chords => "chords",
            LibRole::Synth => "synth",
        };

        // Function uniqueness within the family.
        let mut seen_funcs: HashSet<PatternFunction> = HashSet::new();
        for m in &fam.members {
            if !seen_funcs.insert(m.function) {
                errors.push(format!(
                    "family {} declares {:?} more than once",
                    fam.id, m.function
                ));
            }
            // Member resolves to a real pattern by role+genre+name.
            if lib.find(role, &fam.genre, &m.name).is_none() {
                errors.push(format!(
                    "family {} member {:?} does not resolve in {}/{}",
                    fam.id, m.name, role, fam.genre
                ));
            }
            // A pattern belongs to at most one family.
            if let Some(other) = enrolled.insert((fam.role, &fam.genre, &m.name), &fam.id) {
                if other != fam.id {
                    errors.push(format!(
                        "pattern {}/{}/{} is in two families: {} and {}",
                        role, fam.genre, m.name, other, fam.id
                    ));
                }
            }
        }

        // Every family must have a Core anchor.
        if !seen_funcs.contains(&PatternFunction::Core) {
            errors.push(format!("family {} has no Core member", fam.id));
        }
    }

    assert!(
        errors.is_empty(),
        "family registry errors:\n{}",
        errors.join("\n")
    );

    // Soft report: which functions each family is missing (informational).
    let mut incomplete = 0usize;
    for fam in families {
        let present: HashSet<PatternFunction> = fam.members.iter().map(|m| m.function).collect();
        let missing: Vec<&str> = ALL_FUNCS
            .iter()
            .filter(|f| !present.contains(f))
            .map(|f| f.label())
            .collect();
        if !missing.is_empty() {
            incomplete += 1;
            eprintln!(
                "family {} incomplete — missing: {}",
                fam.id,
                missing.join(", ")
            );
        }
    }
    eprintln!(
        "family registry: {} families, {} complete, {} incomplete",
        families.len(),
        families.len() - incomplete,
        incomplete
    );
}

#[test]
fn content_key_collides_only_on_identical_source() {
    use midip::pattern::model::{DrumHit, TrigCond};
    let mk = |vel: u8| Pattern {
        name: "x".into(),
        desc: String::new(),
        length: 1,
        data: PatternData::Drums(vec![vec![DrumHit {
            note: 36,
            vel,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }]]),
        id: midip::persist::Id::nil(),
        cc: vec![Vec::new()],
    };
    assert_eq!(content_key(&mk(100)), content_key(&mk(100)));
    assert_ne!(content_key(&mk(100)), content_key(&mk(101)));
}
