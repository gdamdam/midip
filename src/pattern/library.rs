use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use indexmap::IndexMap;
use serde::Deserialize;

use crate::devices::profiles::{S1, T8_BASS};
use crate::pattern::model::{DrumHit, MelodicNote, MelodicStep, Pattern, PatternData, TrigCond};

/// genre name -> patterns, preserving the file's genre order.
pub type GenreMap = IndexMap<String, Vec<Pattern>>;

/// The musical job a lane/pattern performs. This is the hardware-neutral role
/// taxonomy: it is independent of any device profile (a Chords lane may use a
/// J-6, Minilogue XD, MicroFreak, or a generic poly synth). Serialized as the
/// lowercase wire string ("drums"/"bass"/"chords"/"synth"); also persisted on a
/// lane (see `LaneDto.role`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibRole {
    Drums,
    Bass,
    Chords,
    Synth,
}

impl LibRole {
    /// Every role, in canonical lane order (DRUMS, BASS, CHORDS, SYNTH).
    pub const ALL: [LibRole; 4] = [
        LibRole::Drums,
        LibRole::Bass,
        LibRole::Chords,
        LibRole::Synth,
    ];

    /// The lowercase wire string used in `PatternRef`, factory ids, and the GUI DTO.
    pub fn as_str(&self) -> &'static str {
        match self {
            LibRole::Drums => "drums",
            LibRole::Bass => "bass",
            LibRole::Chords => "chords",
            LibRole::Synth => "synth",
        }
    }

    /// Parse a wire string back into a role; `None` for anything unrecognized.
    pub fn from_wire(s: &str) -> Option<LibRole> {
        match s {
            "drums" => Some(LibRole::Drums),
            "bass" => Some(LibRole::Bass),
            "chords" => Some(LibRole::Chords),
            "synth" => Some(LibRole::Synth),
            _ => None,
        }
    }

    /// Uppercase label for lane strips / editor headers (DRUMS, BASS, CHORDS, SYNTH).
    pub fn label(&self) -> &'static str {
        match self {
            LibRole::Drums => "DRUMS",
            LibRole::Bass => "BASS",
            LibRole::Chords => "CHORDS",
            LibRole::Synth => "SYNTH",
        }
    }

    /// The data shape patterns in this role use. Drums → drum data; every melodic
    /// role (bass/chords/synth) → melodic data.
    pub fn lane_kind(&self) -> crate::pattern::model::LaneKind {
        match self {
            LibRole::Drums => crate::pattern::model::LaneKind::Drums,
            _ => crate::pattern::model::LaneKind::Melodic,
        }
    }
}

/// The performance function a pattern serves within its family (Phase 3).
/// Metadata only — it never affects `PatternRef` resolution (role+genre+name).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternFunction {
    Core,
    VariationA,
    VariationB,
    Fill,
    Breakdown,
    Peak,
}

impl PatternFunction {
    pub fn label(&self) -> &'static str {
        match self {
            PatternFunction::Core => "Core",
            PatternFunction::VariationA => "Variation A",
            PatternFunction::VariationB => "Variation B",
            PatternFunction::Fill => "Fill",
            PatternFunction::Breakdown => "Breakdown",
            PatternFunction::Peak => "Peak",
        }
    }
}

/// One member of a performance family: an existing pattern (by name, within the
/// family's role+genre) tagged with the function it serves.
#[derive(Clone, Debug, PartialEq)]
pub struct FamilyMember {
    pub function: PatternFunction,
    pub name: String,
}

/// A performance family: a coherent set of related patterns in one role+genre,
/// grouped by function (Core / Variation A/B / Fill / Breakdown / Peak). Family
/// identity is the stable `id`, never the display names.
#[derive(Clone, Debug, PartialEq)]
pub struct Family {
    pub id: String,
    pub label: String,
    pub role: LibRole,
    pub genre: String,
    pub members: Vec<FamilyMember>,
}

pub struct Library {
    pub drums: GenreMap,
    pub bass: GenreMap,
    /// Polyphonic chord patterns (role: chords). Populated from v2 `chords-*.json`
    /// files; there is no legacy device-shaped file for this role.
    pub chords: GenreMap,
    pub synth: GenreMap,
    /// Performance families (Phase 3). Additive metadata over the patterns above;
    /// does not affect loading or `PatternRef` resolution.
    pub families: Vec<Family>,
    /// Stable factory-id and alias registry from v2 patterns (architecture phase).
    /// Additive: `PatternRef` still resolves by role+genre+name; this index only
    /// adds id lookup and an alias fallback for renamed patterns.
    pub v2_index: V2Index,
    /// Flat search/filter index over every pattern (legacy + v2), built once at
    /// load (Phase 8). Additive — existing accessors are untouched.
    pub records: Vec<crate::pattern::index::Record>,
}

/// One stable-id record: the pattern's `factory_id` mapped to its identity triple.
#[derive(Clone, Debug, PartialEq)]
pub struct FactoryIdEntry {
    pub factory_id: String,
    pub role: LibRole,
    pub genre: String,
    pub name: String,
}

/// One alias record: a prior display `alias` (within role+genre) that should
/// resolve to the current `canonical` name.
#[derive(Clone, Debug, PartialEq)]
pub struct AliasEntry {
    pub role: LibRole,
    pub genre: String,
    pub alias: String,
    pub canonical: String,
}

/// Retained v2 envelope metadata for one pattern, keyed by identity. Populated at
/// merge time so the search index (Phase 8) can filter on it; empty for legacy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct V2Meta {
    pub subgenre: Option<String>,
    pub bpm: Option<(u16, u16)>,
    /// The timing-template name (Phase 7) or a free `feel` string.
    pub feel: Option<String>,
    pub energy: Option<String>,
    pub density: Option<String>,
    pub harmonic: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub source: Option<String>,
}

/// Registry populated from v2 factory patterns. Empty when no v2 files are present,
/// so it is inert for a purely-legacy library.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct V2Index {
    pub ids: Vec<FactoryIdEntry>,
    pub aliases: Vec<AliasEntry>,
    /// Envelope metadata keyed by (role, genre, name).
    pub meta: std::collections::HashMap<(LibRole, String, String), V2Meta>,
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
    // Legacy files always carry `slide`; default keeps newer hand-authored notes terse.
    #[serde(default)]
    slide: bool,
    // Phase 2: optional explicit note length in steps. Absent → the profile gate
    // (preserving exact legacy behaviour for all old factory files).
    #[serde(default)]
    len: Option<f32>,
}

/// A melodic step in a factory file. Backward-compatible superset (Phase 2):
///   - `null`                    → rest (handled by the surrounding `Option`)
///   - `{semi,vel,slide?,len?}`  → one note (legacy mono shape)
///   - `[ {..}, {..}, ... ]`     → simultaneous notes (a chord)
/// Untagged: `Many` (array) is listed before `One` so `[..]` is never misread as
/// a single struct note.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawMelodicStep {
    Many(Vec<RawMelodicNote>),
    One(RawMelodicNote),
}

// Drums file: { genre: [ pattern[ step[ {note,vel} ] ] ] }
type RawDrumFile = IndexMap<String, Vec<Vec<Vec<RawDrumHit>>>>;
// Melodic file: { genre: [ pattern[ step: null | {..} | [ {..}, .. ] ] ] }
type RawMelodicFile = IndexMap<String, Vec<Vec<Option<RawMelodicStep>>>>;

/// Build a runtime `MelodicNote` from a factory note, applying the profile gate
/// when no explicit length was authored.
fn mk_melodic_note(n: RawMelodicNote, gate_fraction: f32) -> MelodicNote {
    MelodicNote {
        semi: n.semi,
        vel: n.vel,
        slide: n.slide,
        len: n.len.unwrap_or(gate_fraction),
        prob: 1.0,
        ratchet: 1,
        micro: 0,
        cond: TrigCond::Always,
    }
}

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
                            micro: 0,
                            cond: TrigCond::Always,
                        })
                        .collect::<Vec<DrumHit>>()
                })
                .collect();
            parsed.push(Pattern {
                name: format!("{} #{:02}", genre, idx + 1),
                desc: String::new(),
                length,
                data: PatternData::Drums(data),
                id: crate::persist::Id::nil(),
                cc: vec![Vec::new(); length],
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
            // Per-step shape: `null` -> rest; a single object -> mono; an array ->
            // a chord (Phase 2). Each note's `len` is its explicit value or the gate.
            let data = steps
                .into_iter()
                .map(|step| {
                    let notes: Vec<MelodicNote> = match step {
                        None => Vec::new(),
                        Some(RawMelodicStep::One(n)) => vec![mk_melodic_note(n, gate_fraction)],
                        Some(RawMelodicStep::Many(v)) => v
                            .into_iter()
                            .map(|n| mk_melodic_note(n, gate_fraction))
                            .collect(),
                    };
                    MelodicStep::from(notes)
                })
                .collect();
            parsed.push(Pattern {
                name: format!("{} #{:02}", genre, idx + 1),
                desc: String::new(),
                length,
                data: PatternData::Melodic(data),
                id: crate::persist::Id::nil(),
                cc: vec![Vec::new(); length],
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
        // Chords is a v2-only role — patterns carry their own names, so there is
        // no legacy catalog section to overlay.
        LibRole::Chords => Vec::new(),
    };
    let mut map = HashMap::new();
    for g in genres {
        let entries = g.patterns.into_iter().map(|e| (e.name, e.desc)).collect();
        map.insert(g.name, entries);
    }
    Ok(map)
}

// --- families (Phase 3) ---------------------------------------------------

#[derive(Deserialize)]
struct RawFamilyMember {
    function: PatternFunction,
    name: String,
}

#[derive(Deserialize)]
struct RawFamily {
    id: String,
    label: String,
    role: String,
    genre: String,
    members: Vec<RawFamilyMember>,
}

#[derive(Deserialize)]
struct FamiliesRoot {
    #[serde(default)]
    families: Vec<RawFamily>,
}

/// Parse the optional top-level `families` array from catalog.json. Non-fatal:
/// returns empty on any parse error or when the key is absent (families whose
/// `role` is unknown are skipped).
/// Parse the retained v2 envelope metadata (free-form JSON maps) into typed
/// `V2Meta`. Missing/garbage keys degrade to None — never fails.
fn v2meta_from(
    md: &serde_json::Map<String, serde_json::Value>,
    prov: &Option<serde_json::Map<String, serde_json::Value>>,
) -> V2Meta {
    let s = |k: &str| md.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let u16v = |k: &str| md.get(k).and_then(|v| v.as_u64()).map(|n| n as u16);
    let bpm = match (u16v("bpm_min"), u16v("bpm_max")) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    };
    let tags = md
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let pstr = |k: &str| {
        prov.as_ref()
            .and_then(|p| p.get(k))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    V2Meta {
        subgenre: s("subgenre"),
        bpm,
        // Prefer the timing-template name (Phase 7); fall back to a free feel string.
        feel: s("timing").or_else(|| s("feel")),
        energy: s("energy"),
        density: s("density"),
        harmonic: s("harmonic"),
        tags,
        author: pstr("author"),
        source: pstr("source"),
    }
}

pub fn parse_families(json: &str) -> Vec<Family> {
    let root: FamiliesRoot = match serde_json::from_str(json) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    root.families
        .into_iter()
        .filter_map(|rf| {
            let role = match LibRole::from_wire(&rf.role) {
                Some(r) => r,
                None => return None,
            };
            Some(Family {
                id: rf.id,
                label: rf.label,
                role,
                genre: rf.genre,
                members: rf
                    .members
                    .into_iter()
                    .map(|m| FamilyMember {
                        function: m.function,
                        name: m.name,
                    })
                    .collect(),
            })
        })
        .collect()
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

        // Performance families (Phase 3) — optional; empty if the catalog is
        // absent or has no `families` array. Additive metadata only.
        let families = catalog_json
            .as_deref()
            .map(parse_families)
            .unwrap_or_default();

        let mut lib = Library {
            drums,
            bass,
            // Chords has no legacy file; it is filled entirely from v2 below.
            chords: GenreMap::new(),
            synth,
            families,
            v2_index: V2Index::default(),
            records: Vec::new(),
        };

        // v2 factory patterns (architecture phase) — optional, additive, non-fatal.
        // Any v2 file under `<dir>/v2/` is merged into the genre maps and its stable
        // id/aliases recorded. A missing dir yields nothing; a malformed or too-new
        // file is skipped with a warning rather than failing the whole load.
        let (loaded, mut warnings) = crate::pattern::format_v2::load_v2_dir(&dir.join("v2"));
        warnings.extend(lib.merge_v2(loaded));
        for w in &warnings {
            eprintln!("midip: v2 pattern load: {w}");
        }

        // Build the flat search/filter index (Phase 8) once, after all patterns are
        // present. Purely additive — existing accessors are unaffected.
        lib.records = lib.build_records();

        Ok(lib)
    }

    /// Construct an empty library (no patterns in any role).
    pub fn empty() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            chords: GenreMap::new(),
            synth: GenreMap::new(),
            families: Vec::new(),
            v2_index: V2Index::default(),
            records: Vec::new(),
        }
    }

    /// Merge parsed v2 patterns into the library. Each pattern is appended to its
    /// `role`/`genre` list (creating the genre if new) and its stable id + aliases
    /// are recorded. A v2 pattern whose `name` collides with an existing pattern in
    /// the same role+genre is **skipped** (returned as a warning) so it can never
    /// shadow or duplicate a legacy pattern that saved refs point at.
    ///
    /// Returns a list of human-readable warnings for skipped patterns.
    pub fn merge_v2(&mut self, loaded: Vec<crate::pattern::format_v2::LoadedV2>) -> Vec<String> {
        let mut warnings = Vec::new();
        for item in loaded {
            let map = match item.role {
                LibRole::Drums => &mut self.drums,
                LibRole::Bass => &mut self.bass,
                LibRole::Chords => &mut self.chords,
                LibRole::Synth => &mut self.synth,
            };
            let bucket = map.entry(item.genre.clone()).or_default();
            if bucket.iter().any(|p| p.name == item.pattern.name) {
                warnings.push(format!(
                    "v2 {}: name '{}' already exists in {}/{} — skipped",
                    item.factory_id,
                    item.pattern.name,
                    crate::pattern::format_v2::role_str(item.role),
                    item.genre
                ));
                continue;
            }
            bucket.push(item.pattern.clone());
            self.v2_index.meta.insert(
                (item.role, item.genre.clone(), item.pattern.name.clone()),
                v2meta_from(&item.metadata, &item.provenance),
            );
            self.v2_index.ids.push(FactoryIdEntry {
                factory_id: item.factory_id,
                role: item.role,
                genre: item.genre.clone(),
                name: item.pattern.name.clone(),
            });
            for alias in item.aliases {
                self.v2_index.aliases.push(AliasEntry {
                    role: item.role,
                    genre: item.genre.clone(),
                    alias,
                    canonical: item.pattern.name.clone(),
                });
            }
        }
        warnings
    }

    /// The stable factory id for a pattern identity, if one is registered.
    pub fn factory_id_of(&self, role: LibRole, genre: &str, name: &str) -> Option<&str> {
        self.v2_index
            .ids
            .iter()
            .find(|e| e.role == role && e.genre == genre && e.name == name)
            .map(|e| e.factory_id.as_str())
    }

    /// Resolve a stable factory id back to its `(role, genre, name)` identity.
    pub fn resolve_factory_id(&self, factory_id: &str) -> Option<(LibRole, &str, &str)> {
        self.v2_index
            .ids
            .iter()
            .find(|e| e.factory_id == factory_id)
            .map(|e| (e.role, e.genre.as_str(), e.name.as_str()))
    }

    /// If `name` is a registered alias within `role`+`genre`, return the current
    /// canonical name it maps to. Used as a resolution fallback for renamed
    /// patterns so old saved `PatternRef`s keep working. `role` is the wire string
    /// ("drums"/"bass"/"synth").
    pub fn resolve_alias_name(&self, role: &str, genre: &str, name: &str) -> Option<&str> {
        let role = LibRole::from_wire(role)?;
        self.v2_index
            .aliases
            .iter()
            .find(|a| a.role == role && a.genre == genre && a.alias == name)
            .map(|a| a.canonical.as_str())
    }

    /// Look up a pattern by role+genre+name, falling back to the v2 alias registry
    /// when the direct name misses (so a renamed pattern still resolves by its old
    /// name). This is the single consult point for alias-aware resolution.
    pub fn find_aliased(&self, role: &str, genre: &str, name: &str) -> Option<&Pattern> {
        self.find(role, genre, name)
            .or_else(|| {
                self.resolve_alias_name(role, genre, name)
                    .and_then(|canon| self.find(role, genre, canon))
            })
            // Cross-role migration bridge: chord patterns that used to ship under
            // the `synth` role were reclassified to `chords` (same genre+name).
            // An old saved `PatternRef` with role "synth" must still resolve.
            .or_else(|| {
                if role == "synth" {
                    self.find("chords", genre, name)
                } else {
                    None
                }
            })
    }

    /// All performance families.
    pub fn families(&self) -> &[Family] {
        &self.families
    }

    /// The flat search/filter index over every pattern (Phase 8).
    pub fn records(&self) -> &[crate::pattern::index::Record] {
        &self.records
    }

    /// Run a query against the index, returning the matching records (sorted).
    pub fn query<'a>(
        &'a self,
        q: &crate::pattern::index::Query,
        favs: &crate::pattern::store::Favorites,
    ) -> Vec<&'a crate::pattern::index::Record> {
        crate::pattern::index::filter(&self.records, q, favs)
            .into_iter()
            .map(|i| &self.records[i])
            .collect()
    }

    /// Build the flat record index over all patterns. v2 patterns take their rich
    /// metadata from the retained envelope; legacy patterns derive what they can and
    /// leave the rest Unknown/None.
    fn build_records(&self) -> Vec<crate::pattern::index::Record> {
        use crate::pattern::index::{feel_from_data, make_record, poly_of, Density, Energy, Feel};
        let mut out = Vec::new();
        for (role, map) in [
            (LibRole::Drums, &self.drums),
            (LibRole::Bass, &self.bass),
            (LibRole::Chords, &self.chords),
            (LibRole::Synth, &self.synth),
        ] {
            for (genre, pats) in map {
                for p in pats {
                    let fam = self.family_of(role, genre, &p.name);
                    let m = self
                        .v2_index
                        .meta
                        .get(&(role, genre.clone(), p.name.clone()));
                    let fid = self
                        .factory_id_of(role, genre, &p.name)
                        .map(|s| s.to_string());
                    let (feel, energy, density, bpm, harmonic, subgenre, tags, author, source) =
                        match m {
                            Some(m) => (
                                m.feel.as_deref().map(Feel::parse).unwrap_or(Feel::Straight),
                                m.energy
                                    .as_deref()
                                    .map(Energy::parse)
                                    .unwrap_or(Energy::Unknown),
                                m.density
                                    .as_deref()
                                    .map(Density::parse)
                                    .unwrap_or(Density::Unknown),
                                m.bpm,
                                m.harmonic.clone(),
                                m.subgenre.clone(),
                                m.tags.clone(),
                                m.author.clone(),
                                m.source.clone(),
                            ),
                            None => (
                                feel_from_data(&p.data),
                                Energy::Unknown,
                                Density::Unknown,
                                None,
                                None,
                                None,
                                Vec::new(),
                                None,
                                None,
                            ),
                        };
                    out.push(make_record(
                        role,
                        genre.clone(),
                        p.name.clone(),
                        fid,
                        p.kind(),
                        p.length,
                        fam.map(|(f, _)| f.label.clone()),
                        fam.map(|(_, func)| func),
                        feel,
                        poly_of(&p.data),
                        subgenre,
                        bpm,
                        energy,
                        density,
                        harmonic,
                        tags,
                        author,
                        source,
                        p.desc.clone(),
                    ));
                }
            }
        }
        out
    }

    /// The family and function a given pattern belongs to, if any. Matches by
    /// role + genre + name (the same identity as `PatternRef`).
    pub fn family_of(
        &self,
        role: LibRole,
        genre: &str,
        name: &str,
    ) -> Option<(&Family, PatternFunction)> {
        self.families.iter().find_map(|f| {
            if f.role == role && f.genre == genre {
                f.members
                    .iter()
                    .find(|m| m.name == name)
                    .map(|m| (f, m.function))
            } else {
                None
            }
        })
    }

    /// Genre names in file order for a given role.
    pub fn genres(&self, role: LibRole) -> Vec<&str> {
        let map = match role {
            LibRole::Drums => &self.drums,
            LibRole::Bass => &self.bass,
            LibRole::Chords => &self.chords,
            LibRole::Synth => &self.synth,
        };
        map.keys().map(|s| s.as_str()).collect()
    }

    /// Look up a pattern by (role, genre, name). role must be "drums", "bass",
    /// "chords", or "synth".
    pub fn find(&self, role: &str, genre: &str, name: &str) -> Option<&Pattern> {
        let map = match role {
            "drums" => &self.drums,
            "bass" => &self.bass,
            "chords" => &self.chords,
            "synth" => &self.synth,
            _ => return None,
        };
        map.get(genre)?.iter().find(|p| p.name == name)
    }

    /// All (role, genre, name) triples for building PatternRef::Vendored.
    pub fn entries(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (role, map) in [
            ("drums", &self.drums),
            ("bass", &self.bass),
            ("chords", &self.chords),
            ("synth", &self.synth),
        ] {
            for (genre, patterns) in map {
                for pat in patterns {
                    out.push((role.to_string(), genre.clone(), pat.name.clone()));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles::{S1, T8_BASS};
    use crate::pattern::model::{LaneKind, PatternData, TrigCond};

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
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
                    }
                );
                assert_eq!(
                    steps[0][1],
                    DrumHit {
                        note: 42,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
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
                let n0 = steps[0].first().unwrap();
                assert_eq!(n0.semi, 0);
                assert_eq!(n0.vel, 1.0);
                assert!(!n0.slide);
                assert_eq!(n0.len, 0.5); // T8_BASS.gate_fraction
                assert!(steps[1].is_empty());
                let n2 = steps[2].first().unwrap();
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
            assert_eq!(steps[0].first().unwrap().len, 0.9);
        } else {
            panic!("expected melodic");
        }
    }

    #[test]
    fn parse_melodic_file_accepts_chords_and_explicit_len() {
        // Step 0: rest. Step 1: legacy single object (no len). Step 2: a 3-note
        // chord array. Step 3: a single note with an explicit len.
        let json = r#"{
            "house": [
                [ null,
                  {"semi": 0, "vel": 1.0, "slide": false},
                  [ {"semi": 0, "vel": 1.0, "slide": false},
                    {"semi": 3, "vel": 1.0, "slide": false},
                    {"semi": 7, "vel": 1.0, "slide": false, "len": 2.0} ],
                  {"semi": -5, "vel": 0.9, "slide": false, "len": 8.0} ]
            ]
        }"#;
        let map = parse_melodic_file(json, S1.gate_fraction).unwrap();
        let p = &map["house"][0];
        assert_eq!(p.length, 4);
        match &p.data {
            PatternData::Melodic(steps) => {
                assert!(steps[0].is_empty(), "rest");
                // legacy single object -> one note, len == gate
                assert_eq!(steps[1].len(), 1);
                assert_eq!(steps[1][0].len, S1.gate_fraction);
                // chord array -> 3 simultaneous notes
                assert_eq!(steps[2].len(), 3, "chord step has 3 notes");
                assert_eq!(steps[2][0].semi, 0);
                assert_eq!(steps[2][1].semi, 3);
                assert_eq!(steps[2][2].semi, 7);
                // explicit len survives (both in the chord and the mono note)
                assert_eq!(steps[2][2].len, 2.0);
                assert_eq!(steps[3].len(), 1);
                assert_eq!(steps[3][0].len, 8.0);
            }
            _ => panic!("expected melodic"),
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

        // Each role has the 20 original legacy genres, plus any additive v2 genre
        // packs (Phase 6). The legacy files themselves still contribute exactly 20.
        assert!(lib.genres(LibRole::Drums).len() >= 20);
        assert!(lib.genres(LibRole::Bass).len() >= 20);
        assert!(lib.genres(LibRole::Synth).len() >= 20);

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

    #[test]
    fn parse_families_reads_schema_and_skips_unknown_roles() {
        let json = r#"{"families":[
          {"id":"a-drums","label":"A","role":"drums","genre":"techno",
           "members":[{"function":"core","name":"Four on Floor"},
                      {"function":"variation_a","name":"Kick Drive"}]},
          {"id":"bad","label":"B","role":"percussion","genre":"techno",
           "members":[{"function":"core","name":"X"}]}
        ]}"#;
        let fams = parse_families(json);
        // Unknown role ("percussion") is skipped.
        assert_eq!(fams.len(), 1);
        let f = &fams[0];
        assert_eq!(f.id, "a-drums");
        assert_eq!(f.role, LibRole::Drums);
        assert_eq!(f.members[0].function, PatternFunction::Core);
        assert_eq!(f.members[1].function, PatternFunction::VariationA);
        // Absent/garbage → empty, never a panic.
        assert!(parse_families("{}").is_empty());
        assert!(parse_families("not json").is_empty());
    }

    #[test]
    fn load_reads_families_and_family_of_matches_by_identity() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&dir).unwrap();

        // Catalog ships the 20 original Phase-3 families plus the Phase-6 genre-pack
        // families; each has a Core member and every member resolves in its role+genre.
        assert!(
            lib.families().len() >= 20,
            "expected at least the 20 original families, got {}",
            lib.families().len()
        );
        for fam in lib.families() {
            let role = fam.role.as_str();
            assert!(
                fam.members
                    .iter()
                    .any(|m| m.function == PatternFunction::Core),
                "family {} lacks a Core member",
                fam.id
            );
            for m in &fam.members {
                assert!(
                    lib.find(role, &fam.genre, &m.name).is_some(),
                    "family {} member {:?} does not resolve",
                    fam.id,
                    m.name
                );
            }
        }

        // family_of matches by role+genre+name (same identity as PatternRef).
        let (fam, func) = lib
            .family_of(LibRole::Drums, "techno", "Four on Floor")
            .expect("Four on Floor is in the techno drum family");
        assert_eq!(fam.id, "techno-drive-drums");
        assert_eq!(func, PatternFunction::Core);
        // A pattern not enrolled in any family → None.
        assert!(lib
            .family_of(LibRole::Drums, "techno", "Iron Grid")
            .is_none());
    }

    #[test]
    fn factory_s1_upgrades_have_real_chords_and_lengths() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&dir).unwrap();

        // A step somewhere in `p` is exactly the (sorted) voicing `want`.
        let has_chord = |p: &Pattern, want: &[i8]| -> bool {
            match &p.data {
                PatternData::Melodic(steps) => steps.iter().any(|s| {
                    if s.len() < 2 {
                        return false;
                    }
                    let mut v: Vec<i8> = s.iter().map(|n| n.semi).collect();
                    v.sort_unstable();
                    v == want
                }),
                _ => false,
            }
        };
        let has_len = |p: &Pattern, min: f32| -> bool {
            match &p.data {
                PatternData::Melodic(steps) => {
                    steps.iter().flat_map(|s| s.iter()).any(|n| n.len >= min)
                }
                _ => false,
            }
        };

        // deep-house "Rhodes": held maj7 voicing.
        let rhodes = lib.find("synth", "deep-house", "Rhodes").unwrap();
        assert!(has_chord(rhodes, &[0, 4, 7, 11]), "Rhodes maj7 chord");
        assert!(has_len(rhodes, 4.0), "Rhodes is held");
        // lo-fi "Jazz Chord": maj7.
        assert!(has_chord(
            lib.find("synth", "lo-fi", "Jazz Chord").unwrap(),
            &[0, 4, 7, 11]
        ));
        // garage "R&B Chord": m7 stab.
        assert!(has_chord(
            lib.find("synth", "garage", "R&B Chord").unwrap(),
            &[0, 3, 7, 10]
        ));
        // ambient "Long Tone": open-fifth drone sustained a whole bar.
        let lt = lib.find("synth", "ambient", "Long Tone").unwrap();
        assert!(has_chord(lt, &[0, 7]), "open fifth");
        assert!(has_len(lt, 16.0), "whole-bar drone");

        // Legacy mono pattern is untouched: monophonic and at the profile gate length.
        let iron = lib.find("synth", "techno", "Iron Grid").unwrap();
        match &iron.data {
            PatternData::Melodic(steps) => {
                assert!(steps.iter().all(|s| s.len() <= 1), "legacy stays mono");
                assert!(
                    steps
                        .iter()
                        .flat_map(|s| s.iter())
                        .all(|n| (n.len - S1.gate_fraction).abs() < 1e-6),
                    "legacy note length == profile gate"
                );
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn library_find_returns_known_pattern() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&dir).unwrap();
        let pat = lib.find("drums", "techno", "Four on Floor");
        assert!(pat.is_some());
        assert_eq!(pat.unwrap().name, "Four on Floor");
        // None for unknown
        assert!(lib.find("drums", "techno", "nonexistent").is_none());
        assert!(lib
            .find("unknown_role", "techno", "Four on Floor")
            .is_none());
    }

    #[test]
    fn library_entries_nonempty() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&dir).unwrap();
        let entries = lib.entries();
        assert!(!entries.is_empty());
        // All three roles present
        assert!(entries.iter().any(|(r, _, _)| r == "drums"));
        assert!(entries.iter().any(|(r, _, _)| r == "bass"));
        assert!(entries.iter().any(|(r, _, _)| r == "synth"));
    }
}
