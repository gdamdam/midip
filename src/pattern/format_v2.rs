//! Factory pattern format **v2** — a documented, versioned, self-describing JSON
//! representation for a single factory pattern.
//!
//! Motivation (architecture phase after Phases 1–3): the legacy "mpump" factory
//! files (`patterns-t8-*.json`, `patterns-s1.json`) encode only note/vel and a
//! null/object/array step shape; everything else in the runtime [`Pattern`] model
//! (probability, ratchets, microtiming, trig conditions, per-step CC locks, note
//! length, explicit role/kind, a stable name-independent identity, provenance) is
//! either impossible to express or only implied. v2 makes all of it explicit while
//! **deserializing into the existing `Pattern` model** — there is no parallel
//! runtime representation.
//!
//! Design invariants:
//!   * **Additive.** v2 files are loaded *in addition to* the legacy files and
//!     merged into the same [`GenreMap`](crate::pattern::library::GenreMap). Legacy
//!     loading is never changed, so all existing factory/user/set data keeps
//!     loading unchanged.
//!   * **Identity preserved.** A v2 pattern carries an authoritative `name`, so
//!     `PatternRef::Vendored{role,genre,name}` — persisted in sets, scenes, crates
//!     and favorites — keeps resolving. The stable `factory_id` is *additional*
//!     metadata held in the library index, never inside `PatternRef`.
//!   * **Migration path.** Optional `aliases` (prior display names) register a
//!     name→canonical mapping so a future rename does not break old saved refs;
//!     `resolve_pattern_ref` consults it on a miss.
//!   * **Reuse.** `steps` and `cc` deserialize straight into the model's own serde
//!     types, so the chord/rest step shapes and `TrigCond`/`CcLock` encodings are
//!     exactly those already used everywhere else.
//!
//! The on-disk schema is documented in `docs/pattern-format-v2.md`; this module is
//! the authoritative parser and validator.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::pattern::library::LibRole;
use crate::pattern::model::{CcLock, DrumStep, MelodicStep, Pattern, PatternData};

/// The fixed `schema` discriminator every v2 file must carry.
pub const V2_SCHEMA: &str = "midip.pattern";
/// The highest `version` this build understands. Files with a higher version are
/// rejected with an actionable error (never silently downgraded).
pub const V2_VERSION: u32 = 2;

/// Raw on-disk envelope. `deny_unknown_fields` turns typos / stray keys into hard
/// errors instead of silent data loss. `steps` is kept as a `Value` so the loader
/// can parse it into the drum- or melodic-shaped vec according to `kind` and emit a
/// clear error on a shape/kind mismatch.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawV2 {
    schema: String,
    version: u32,
    factory_id: String,
    role: String,
    kind: String,
    genre: String,
    name: String,
    #[serde(default)]
    desc: String,
    length: usize,
    /// Prior display names this pattern has had; registered as aliases so old
    /// `role+genre+name` references still resolve after a rename.
    #[serde(default)]
    aliases: Vec<String>,
    steps: Value,
    /// Per-step CC locks (same shape as `Pattern.cc`). Absent → none.
    #[serde(default)]
    cc: Vec<Vec<CcLock>>,
    /// Free-form extension point; carried on the library index, not the model.
    #[serde(default)]
    metadata: Map<String, Value>,
    /// Optional provenance (source/author/version/…). Free-form by design.
    #[serde(default)]
    provenance: Option<Map<String, Value>>,
}

/// A successfully parsed + validated v2 pattern: the runtime [`Pattern`] plus the
/// library-level metadata that has no home on the model itself.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedV2 {
    pub pattern: Pattern,
    pub role: LibRole,
    pub genre: String,
    pub factory_id: String,
    pub aliases: Vec<String>,
    pub metadata: Map<String, Value>,
    pub provenance: Option<Map<String, Value>>,
}

fn parse_role(s: &str) -> Option<LibRole> {
    LibRole::from_wire(s)
}

/// The `LibRole` → wire string used in `PatternRef`/`find` (drums/bass/chords/synth).
pub fn role_str(role: LibRole) -> &'static str {
    role.as_str()
}

/// Maximum simultaneous notes a factory melodic step may hold. This is the
/// baseline four-voice polyphony of the default CHORDS device (Roland J-6); mono
/// roles (bass/synth) never approach it.
pub const MAX_CHORD_VOICES: usize = 4;

/// A factory_id must be a non-empty run of `[a-z0-9._-]` — stable, lowercase, and
/// filesystem/URL friendly, independent of the display name.
fn factory_id_valid(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// Range/consistency checks over an already-built `Pattern`. Returns a list of
/// human-readable problems (empty = valid). Kept separate from parsing so one call
/// reports *all* issues, mirroring the `library_lint` accumulate-and-report style.
fn validate(pattern: &Pattern, role: LibRole) -> Vec<String> {
    let mut errs = Vec::new();

    if pattern.length == 0 || pattern.length > 64 {
        errs.push(format!("length {} out of range 1..=64", pattern.length));
    }

    // role ↔ data-kind consistency. Every melodic role (bass/chords/synth) holds
    // melodic data; only drums holds drum data.
    match (&pattern.data, role) {
        (PatternData::Drums(_), LibRole::Drums) => {}
        (PatternData::Melodic(_), LibRole::Bass | LibRole::Chords | LibRole::Synth) => {}
        (PatternData::Drums(_), r) => {
            errs.push(format!("role '{}' cannot hold drum data", role_str(r)))
        }
        (PatternData::Melodic(_), LibRole::Drums) => {
            errs.push("role 'drums' cannot hold melodic data".to_string())
        }
    }

    let cond_ok = |errs: &mut Vec<String>, where_: &str, c: &crate::pattern::model::TrigCond| {
        if let crate::pattern::model::TrigCond::Ratio { x, y } = c {
            if *x < 1 || *y < 1 || x > y {
                errs.push(format!(
                    "{where_}: trig Ratio x={x} y={y} must satisfy 1<=x<=y"
                ));
            }
        }
    };

    match &pattern.data {
        PatternData::Drums(steps) => {
            for (i, step) in steps.iter().enumerate() {
                for hit in step {
                    let w = format!("step {i} note {}", hit.note);
                    // note is u8 (0..=127 by type); vel must be an audible 1..=127.
                    if hit.vel < 1 {
                        errs.push(format!("{w}: vel {} must be 1..=127", hit.vel));
                    }
                    if !(0.0..=1.0).contains(&hit.prob) {
                        errs.push(format!("{w}: prob {} must be 0.0..=1.0", hit.prob));
                    }
                    if !(1..=8).contains(&hit.ratchet) {
                        errs.push(format!("{w}: ratchet {} must be 1..=8", hit.ratchet));
                    }
                    if !(-500..=500).contains(&hit.micro) {
                        errs.push(format!(
                            "{w}: micro {} must be -500..=500 permille of a step",
                            hit.micro
                        ));
                    }
                    cond_ok(&mut errs, &w, &hit.cond);
                }
            }
        }
        PatternData::Melodic(steps) => {
            for (i, step) in steps.iter().enumerate() {
                // No factory step may exceed the four-voice baseline (J-6). This
                // keeps chord voicings playable on the default CHORDS device and
                // never truncates silently at runtime.
                if step.len() > MAX_CHORD_VOICES {
                    errs.push(format!(
                        "step {i}: {} simultaneous notes exceeds the {MAX_CHORD_VOICES}-voice maximum",
                        step.len()
                    ));
                }
                for note in step.iter() {
                    let w = format!("step {i} semi {}", note.semi);
                    if !(0.5..=1.3).contains(&note.vel) {
                        errs.push(format!("{w}: vel {} must be 0.5..=1.3", note.vel));
                    }
                    if !(note.len > 0.0 && note.len <= 64.0) {
                        errs.push(format!("{w}: len {} must be 0<len<=64", note.len));
                    }
                    if !(0.0..=1.0).contains(&note.prob) {
                        errs.push(format!("{w}: prob {} must be 0.0..=1.0", note.prob));
                    }
                    if !(1..=8).contains(&note.ratchet) {
                        errs.push(format!("{w}: ratchet {} must be 1..=8", note.ratchet));
                    }
                    if !(-500..=500).contains(&note.micro) {
                        errs.push(format!(
                            "{w}: micro {} must be -500..=500 permille of a step",
                            note.micro
                        ));
                    }
                    cond_ok(&mut errs, &w, &note.cond);
                }
            }
        }
    }

    for (i, slot) in pattern.cc.iter().enumerate() {
        for lock in slot {
            if lock.cc > 127 || lock.val > 127 {
                errs.push(format!(
                    "step {i}: cc lock cc={} val={} each must be 0..=127",
                    lock.cc, lock.val
                ));
            }
        }
    }

    errs
}

/// Parse and fully validate one v2 pattern JSON document. `source` labels the
/// origin (a filename or `"<memory>"`) and is woven into every error so failures
/// are actionable.
///
/// Errors (all `anyhow`, all prefixed with `source`):
///   * malformed JSON / unknown field / missing required key,
///   * wrong `schema` string,
///   * unsupported `version` (in particular a *newer* version is rejected, not
///     silently accepted),
///   * unknown `role`/`kind` or role↔kind mismatch,
///   * `steps` shape not matching `kind`,
///   * `length` ≠ step count or out of 1..=64,
///   * any range/consistency violation from [`validate`],
///   * malformed `factory_id`.
pub fn parse_pattern_v2(json: &str, source: &str) -> anyhow::Result<LoadedV2> {
    let raw: RawV2 = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("{source}: not a valid v2 pattern document: {e}"))?;

    if raw.schema != V2_SCHEMA {
        anyhow::bail!(
            "{source}: unknown schema {:?} (expected {:?})",
            raw.schema,
            V2_SCHEMA
        );
    }
    if raw.version > V2_VERSION {
        anyhow::bail!(
            "{source}: pattern schema version {} was written by a newer midip; this build supports up to version {}",
            raw.version,
            V2_VERSION
        );
    }
    if raw.version != V2_VERSION {
        anyhow::bail!(
            "{source}: unsupported schema version {} (this build supports version {})",
            raw.version,
            V2_VERSION
        );
    }

    let role = parse_role(&raw.role)
        .ok_or_else(|| anyhow::anyhow!("{source}: unknown role {:?}", raw.role))?;

    // Build PatternData by parsing `steps` into the model's own step types. Kind is
    // validated against role here; the shape is validated by serde.
    let data = match raw.kind.as_str() {
        "drums" => {
            if role != LibRole::Drums {
                anyhow::bail!(
                    "{source}: kind 'drums' is only valid for role 'drums' (got {:?})",
                    raw.role
                );
            }
            let steps: Vec<DrumStep> = serde_json::from_value(raw.steps).map_err(|e| {
                anyhow::anyhow!("{source}: 'steps' is not valid drum-step data: {e}")
            })?;
            PatternData::Drums(steps)
        }
        "melodic" => {
            if role == LibRole::Drums {
                anyhow::bail!("{source}: kind 'melodic' is not valid for role 'drums'");
            }
            let steps: Vec<MelodicStep> = serde_json::from_value(raw.steps).map_err(|e| {
                anyhow::anyhow!("{source}: 'steps' is not valid melodic-step data: {e}")
            })?;
            PatternData::Melodic(steps)
        }
        other => anyhow::bail!("{source}: unknown kind {other:?} (expected 'drums' or 'melodic')"),
    };

    let step_count = match &data {
        PatternData::Drums(s) => s.len(),
        PatternData::Melodic(s) => s.len(),
    };
    if raw.length != step_count {
        anyhow::bail!(
            "{source}: declared length {} != step count {}",
            raw.length,
            step_count
        );
    }

    if raw.cc.len() > raw.length {
        anyhow::bail!(
            "{source}: cc has {} slots but pattern length is {}",
            raw.cc.len(),
            raw.length
        );
    }
    if !factory_id_valid(&raw.factory_id) {
        anyhow::bail!(
            "{source}: factory_id {:?} must be a non-empty run of [a-z0-9._-]",
            raw.factory_id
        );
    }

    // `cc` is kept length-synced with the pattern, matching every other code path.
    let mut cc = raw.cc;
    cc.resize(raw.length, Vec::new());

    let pattern = Pattern {
        name: raw.name,
        desc: raw.desc,
        length: raw.length,
        data,
        id: crate::persist::Id::nil(),
        cc,
    };

    let errs = validate(&pattern, role);
    if !errs.is_empty() {
        anyhow::bail!(
            "{source}: invalid v2 pattern '{}' ({}):\n  - {}",
            pattern.name,
            raw.factory_id,
            errs.join("\n  - ")
        );
    }

    Ok(LoadedV2 {
        pattern,
        role,
        genre: raw.genre,
        factory_id: raw.factory_id,
        aliases: raw.aliases,
        metadata: raw.metadata,
        provenance: raw.provenance,
    })
}

/// Load every `*.json` in `dir` as a v2 pattern. Returns the successfully parsed
/// patterns and a list of per-file warnings for the ones that failed. Non-fatal by
/// design: a bad or too-new v2 file is skipped with a clear warning, never crashing
/// the app (the whole factory/catalog load path is already best-effort).
///
/// A missing directory is not an error — it simply yields no patterns.
pub fn load_v2_dir(dir: &std::path::Path) -> (Vec<LoadedV2>, Vec<String>) {
    let mut loaded = Vec::new();
    let mut warnings = Vec::new();
    if !dir.exists() {
        return (loaded, warnings);
    }
    let mut paths: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect(),
        Err(e) => {
            warnings.push(format!("reading v2 dir {}: {e}", dir.display()));
            return (loaded, warnings);
        }
    };
    paths.sort();
    for path in paths {
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<v2>")
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(json) => match parse_pattern_v2(&json, &label) {
                Ok(p) => loaded.push(p),
                Err(e) => warnings.push(format!("{e}")),
            },
            Err(e) => warnings.push(format!("{label}: read error: {e}")),
        }
    }
    (loaded, warnings)
}

// --- deterministic legacy → v2 conversion (the conversion tool's engine) --------

/// Lowercase-slug a display name into the stable-id tail: alphanumerics kept,
/// everything else collapsed to single `-`. Pure and deterministic.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Deterministically convert an in-memory factory [`Pattern`] (already resolved
/// from legacy data) into a v2 envelope JSON string. Same inputs always yield
/// byte-identical output — no timestamps, no randomness — so it is safe to use as
/// a batch conversion tool and to assert on in tests.
///
/// The generated `factory_id` is `"{role}.{genre}.{slug(name)}"`. `steps`/`cc` are
/// serialized through the model's own serde, so the output round-trips back through
/// [`parse_pattern_v2`] into an equal `Pattern`.
pub fn to_v2_json(role: LibRole, genre: &str, pat: &Pattern) -> anyhow::Result<String> {
    let kind = match pat.data {
        PatternData::Drums(_) => "drums",
        PatternData::Melodic(_) => "melodic",
    };
    let steps = match &pat.data {
        PatternData::Drums(s) => serde_json::to_value(s)?,
        PatternData::Melodic(s) => serde_json::to_value(s)?,
    };
    // Preserve the pattern's cc, trimmed of trailing empties for compactness.
    let mut cc = pat.cc.clone();
    while cc.last().is_some_and(|s| s.is_empty()) {
        cc.pop();
    }
    let factory_id = format!("{}.{}.{}", role_str(role), genre, slug(&pat.name));

    let mut obj = Map::new();
    obj.insert("schema".into(), Value::from(V2_SCHEMA));
    obj.insert("version".into(), Value::from(V2_VERSION));
    obj.insert("factory_id".into(), Value::from(factory_id));
    obj.insert("role".into(), Value::from(role_str(role)));
    obj.insert("kind".into(), Value::from(kind));
    obj.insert("genre".into(), Value::from(genre));
    obj.insert("name".into(), Value::from(pat.name.clone()));
    if !pat.desc.is_empty() {
        obj.insert("desc".into(), Value::from(pat.desc.clone()));
    }
    obj.insert("length".into(), Value::from(pat.length));
    obj.insert("steps".into(), steps);
    if !cc.is_empty() {
        obj.insert("cc".into(), serde_json::to_value(&cc)?);
    }
    let mut prov = Map::new();
    prov.insert("source".into(), Value::from("factory"));
    prov.insert("converted_from".into(), Value::from("mpump-legacy"));
    obj.insert("provenance".into(), Value::Object(prov));

    Ok(serde_json::to_string_pretty(&Value::Object(obj))?)
}
