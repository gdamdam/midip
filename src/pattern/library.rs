use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use indexmap::IndexMap;
use serde::Deserialize;

use crate::devices::profiles::{S1, T8_BASS};
use crate::pattern::model::{DrumHit, MelodicNote, Pattern, PatternData};

/// genre name -> patterns, preserving the file's genre order.
pub type GenreMap = IndexMap<String, Vec<Pattern>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibRole {
    Drums,
    Bass,
    Synth,
}

pub struct Library {
    pub drums: GenreMap,
    pub bass: GenreMap,
    pub synth: GenreMap,
}

// --- raw JSON shapes (mpump format) ---------------------------------------

#[derive(Deserialize)]
struct RawDrumHit {
    note: u8,
    vel: u8,
}

#[derive(Deserialize)]
struct RawMelodicNote {
    semi: i8,
    vel: f32,
    slide: bool,
}

// Drums file: { genre: [ pattern[ step[ {note,vel} ] ] ] }
type RawDrumFile = IndexMap<String, Vec<Vec<Vec<RawDrumHit>>>>;
// Melodic file: { genre: [ pattern[ step: null | {semi,vel,slide} ] ] }
type RawMelodicFile = IndexMap<String, Vec<Vec<Option<RawMelodicNote>>>>;

/// Parse a drums library JSON into a GenreMap. Steps are lists of `{note,vel}` hits.
pub fn parse_drum_file(json: &str) -> anyhow::Result<GenreMap> {
    let raw: RawDrumFile = serde_json::from_str(json).context("parsing drum pattern file")?;
    let mut out: GenreMap = IndexMap::new();
    for (genre, patterns) in raw {
        let mut parsed = Vec::with_capacity(patterns.len());
        for (idx, steps) in patterns.into_iter().enumerate() {
            let length = steps.len();
            let data = steps
                .into_iter()
                .map(|hits| {
                    hits.into_iter()
                        .map(|h| DrumHit {
                            note: h.note,
                            vel: h.vel,
                            prob: 1.0,
                            ratchet: 1,
                        })
                        .collect::<Vec<DrumHit>>()
                })
                .collect();
            parsed.push(Pattern {
                name: format!("{} #{:02}", genre, idx + 1),
                desc: String::new(),
                length,
                data: PatternData::Drums(data),
            });
        }
        out.insert(genre, parsed);
    }
    Ok(out)
}

/// Parse a melodic (bass/synth) library JSON into a GenreMap. Steps are `null` (rest) or
/// `{semi,vel,slide}`. Each note's `len` is initialized to `gate_fraction` (authoring default).
pub fn parse_melodic_file(json: &str, gate_fraction: f32) -> anyhow::Result<GenreMap> {
    let raw: RawMelodicFile = serde_json::from_str(json).context("parsing melodic pattern file")?;
    let mut out: GenreMap = IndexMap::new();
    for (genre, patterns) in raw {
        let mut parsed = Vec::with_capacity(patterns.len());
        for (idx, steps) in patterns.into_iter().enumerate() {
            let length = steps.len();
            let data = steps
                .into_iter()
                .map(|step| {
                    step.map(|n| MelodicNote {
                        semi: n.semi,
                        vel: n.vel,
                        slide: n.slide,
                        len: gate_fraction,
                        prob: 1.0,
                        ratchet: 1,
                    })
                })
                .collect();
            parsed.push(Pattern {
                name: format!("{} #{:02}", genre, idx + 1),
                desc: String::new(),
                length,
                data: PatternData::Melodic(data),
            });
        }
        out.insert(genre, parsed);
    }
    Ok(out)
}

// --- catalog.json shapes --------------------------------------------------

#[derive(Deserialize)]
struct CatalogEntry {
    name: String,
    desc: String,
}

#[derive(Deserialize)]
struct CatalogGenre {
    name: String,
    patterns: Vec<CatalogEntry>,
}

#[derive(Deserialize)]
struct CatalogS1 {
    genres: Vec<CatalogGenre>,
}

#[derive(Deserialize)]
struct CatalogT8 {
    drum_genres: Vec<CatalogGenre>,
    bass_genres: Vec<CatalogGenre>,
}

#[derive(Deserialize)]
struct CatalogRoot {
    s1: CatalogS1,
    t8: CatalogT8,
}

/// Parse catalog.json and return genre → [(name, desc)] for the given role.
/// Returns an empty map on any parse error (non-fatal).
pub fn parse_catalog(
    json: &str,
    role: LibRole,
) -> anyhow::Result<HashMap<String, Vec<(String, String)>>> {
    let root: CatalogRoot = serde_json::from_str(json).context("parsing catalog.json")?;
    let genres: Vec<CatalogGenre> = match role {
        LibRole::Drums => root.t8.drum_genres,
        LibRole::Bass => root.t8.bass_genres,
        LibRole::Synth => root.s1.genres,
    };
    let mut map = HashMap::new();
    for g in genres {
        let entries = g.patterns.into_iter().map(|e| (e.name, e.desc)).collect();
        map.insert(g.name, entries);
    }
    Ok(map)
}

/// Sort a GenreMap alphabetically by genre name.
fn sort_genres(map: GenreMap) -> GenreMap {
    let mut entries: Vec<(String, Vec<Pattern>)> = map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.into_iter().collect()
}

/// Overlay catalog names+descs onto a GenreMap. Patterns not found in catalog keep their
/// fallback name and an empty desc.
fn overlay_catalog(
    mut map: GenreMap,
    catalog: &HashMap<String, Vec<(String, String)>>,
) -> GenreMap {
    for (genre, patterns) in map.iter_mut() {
        if let Some(entries) = catalog.get(genre) {
            for (i, pat) in patterns.iter_mut().enumerate() {
                if let Some((name, desc)) = entries.get(i) {
                    pat.name = name.clone();
                    pat.desc = desc.clone();
                }
            }
        }
    }
    map
}

impl Library {
    /// Load the three vendored files from `dir` (e.g. `assets/patterns/`).
    pub fn load(dir: &Path) -> anyhow::Result<Library> {
        let drums_json = std::fs::read_to_string(dir.join("patterns-t8-drums.json"))
            .context("reading patterns-t8-drums.json")?;
        let bass_json = std::fs::read_to_string(dir.join("patterns-t8-bass.json"))
            .context("reading patterns-t8-bass.json")?;
        let synth_json = std::fs::read_to_string(dir.join("patterns-s1.json"))
            .context("reading patterns-s1.json")?;

        // Catalog is optional — failure is non-fatal; patterns still load with fallback names.
        let catalog_json = std::fs::read_to_string(dir.join("catalog.json")).ok();
        let drums_cat = catalog_json
            .as_deref()
            .and_then(|j| parse_catalog(j, LibRole::Drums).ok())
            .unwrap_or_default();
        let bass_cat = catalog_json
            .as_deref()
            .and_then(|j| parse_catalog(j, LibRole::Bass).ok())
            .unwrap_or_default();
        let synth_cat = catalog_json
            .as_deref()
            .and_then(|j| parse_catalog(j, LibRole::Synth).ok())
            .unwrap_or_default();

        let drums = sort_genres(overlay_catalog(parse_drum_file(&drums_json)?, &drums_cat));
        let bass = sort_genres(overlay_catalog(
            parse_melodic_file(&bass_json, T8_BASS.gate_fraction)?,
            &bass_cat,
        ));
        let synth = sort_genres(overlay_catalog(
            parse_melodic_file(&synth_json, S1.gate_fraction)?,
            &synth_cat,
        ));

        Ok(Library { drums, bass, synth })
    }

    /// Construct an empty library (no patterns in any role).
    pub fn empty() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    /// Genre names in file order for a given role.
    pub fn genres(&self, role: LibRole) -> Vec<&str> {
        let map = match role {
            LibRole::Drums => &self.drums,
            LibRole::Bass => &self.bass,
            LibRole::Synth => &self.synth,
        };
        map.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles::{S1, T8_BASS};
    use crate::pattern::model::{LaneKind, PatternData};

    #[test]
    fn parse_drum_file_reads_genres_patterns_and_hits() {
        // Two genres; first has one pattern of two steps; second has one empty-step pattern.
        let json = r#"{
            "techno": [
                [ [ {"note": 36, "vel": 120}, {"note": 42, "vel": 100} ], [] ]
            ],
            "acid": [
                [ [] ]
            ]
        }"#;
        let map = parse_drum_file(json).unwrap();
        // genre order preserved
        let genres: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
        assert_eq!(genres, vec!["techno", "acid"]);

        let techno = &map["techno"];
        assert_eq!(techno.len(), 1);
        let p = &techno[0];
        assert_eq!(p.length, 2);
        assert_eq!(p.kind(), LaneKind::Drums);
        match &p.data {
            PatternData::Drums(steps) => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].len(), 2);
                assert_eq!(
                    steps[0][0],
                    DrumHit {
                        note: 36,
                        vel: 120,
                        prob: 1.0,
                        ratchet: 1
                    }
                );
                assert_eq!(
                    steps[0][1],
                    DrumHit {
                        note: 42,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1
                    }
                );
                assert!(steps[1].is_empty());
            }
            _ => panic!("expected drums"),
        }
    }

    #[test]
    fn parse_melodic_file_reads_notes_rests_and_sets_len_from_gate() {
        let json = r#"{
            "techno": [
                [ {"semi": 0, "vel": 1.0, "slide": false}, null,
                  {"semi": 7, "vel": 1.3, "slide": true} ]
            ]
        }"#;
        let map = parse_melodic_file(json, T8_BASS.gate_fraction).unwrap();
        let p = &map["techno"][0];
        assert_eq!(p.length, 3);
        assert_eq!(p.kind(), LaneKind::Melodic);
        match &p.data {
            PatternData::Melodic(steps) => {
                assert_eq!(steps.len(), 3);
                let n0 = steps[0].as_ref().unwrap();
                assert_eq!(n0.semi, 0);
                assert_eq!(n0.vel, 1.0);
                assert!(!n0.slide);
                assert_eq!(n0.len, 0.5); // T8_BASS.gate_fraction
                assert!(steps[1].is_none());
                let n2 = steps[2].as_ref().unwrap();
                assert_eq!(n2.semi, 7);
                assert_eq!(n2.vel, 1.3);
                assert!(n2.slide);
                assert_eq!(n2.len, 0.5);
            }
            _ => panic!("expected melodic"),
        }

        // synth gate yields a different default len
        let map_s = parse_melodic_file(json, S1.gate_fraction).unwrap();
        let n = map_s["techno"][0].data.clone();
        if let PatternData::Melodic(steps) = n {
            assert_eq!(steps[0].as_ref().unwrap().len, 0.9);
        } else {
            panic!("expected melodic");
        }
    }

    #[test]
    fn parse_catalog_extracts_entries_by_role() {
        let json = r#"{
            "s1": { "genres": [
                { "name": "techno", "patterns": [
                    { "name": "Iron Grid", "desc": "relentless" },
                    { "name": "Acid Drive", "desc": "squelchy" }
                ]}
            ]},
            "t8": {
                "drum_genres": [
                    { "name": "techno", "patterns": [
                        { "name": "Four on Floor", "desc": "4/4 kick" }
                    ]}
                ],
                "bass_genres": [
                    { "name": "techno", "patterns": [
                        { "name": "Bass Walk", "desc": "walking" }
                    ]}
                ]
            },
            "keys": [],
            "octave_min": -2,
            "octave_max": 2
        }"#;
        let drums = parse_catalog(json, LibRole::Drums).unwrap();
        assert_eq!(
            drums["techno"][0],
            ("Four on Floor".to_string(), "4/4 kick".to_string())
        );

        let bass = parse_catalog(json, LibRole::Bass).unwrap();
        assert_eq!(
            bass["techno"][0],
            ("Bass Walk".to_string(), "walking".to_string())
        );

        let synth = parse_catalog(json, LibRole::Synth).unwrap();
        assert_eq!(
            synth["techno"][0],
            ("Iron Grid".to_string(), "relentless".to_string())
        );
        assert_eq!(
            synth["techno"][1],
            ("Acid Drive".to_string(), "squelchy".to_string())
        );
    }

    #[test]
    fn load_reads_real_vendored_files() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&dir).unwrap();

        // Each role: 20 genres.
        assert_eq!(lib.genres(LibRole::Drums).len(), 20);
        assert_eq!(lib.genres(LibRole::Bass).len(), 20);
        assert_eq!(lib.genres(LibRole::Synth).len(), 20);

        // Genres are alphabetical — "acid-techno" sorts before everything else.
        assert_eq!(lib.genres(LibRole::Drums)[0], "acid-techno");
        assert_eq!(lib.genres(LibRole::Bass)[0], "acid-techno");
        assert_eq!(lib.genres(LibRole::Synth)[0], "acid-techno");

        // Catalog names applied: drums techno[0] == "Four on Floor".
        let drums_techno = &lib.drums["techno"];
        assert_eq!(drums_techno[0].name, "Four on Floor");
        assert!(
            !drums_techno[0].desc.is_empty(),
            "desc should be non-empty from catalog"
        );

        // Synth techno[0] == "Iron Grid".
        let synth_techno = &lib.synth["techno"];
        assert_eq!(synth_techno[0].name, "Iron Grid");

        // First drum genre (alphabetically) has 20 patterns, each 16 steps.
        let first_drum_genre = lib.genres(LibRole::Drums)[0];
        let pats = &lib.drums[first_drum_genre];
        assert_eq!(pats.len(), 20);
        assert_eq!(pats[0].length, 16);
        assert_eq!(pats[0].kind(), LaneKind::Drums);

        // First bass genre: 20 patterns, 16 steps, melodic kind.
        let first_bass_genre = lib.genres(LibRole::Bass)[0];
        let bpats = &lib.bass[first_bass_genre];
        assert_eq!(bpats.len(), 20);
        assert_eq!(bpats[0].length, 16);
        assert_eq!(bpats[0].kind(), LaneKind::Melodic);
    }
}
