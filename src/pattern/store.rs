use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};

use crate::devices::profiles::profile_by_id;
#[cfg(test)]
use crate::pattern::model::TrigCond;
use crate::pattern::model::{Chain, Lane, LaneKind, LaneRoute, Pattern, PortRef, Scene, Set};
use crate::pattern::refs::PatternRef;
use crate::persist;

/// User preferences persisted across sessions (session pref, not part of the Set).
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct Prefs {
    #[serde(default)]
    pub mirror_on: bool,
}

/// Path of the prefs file under `dir`.
pub fn prefs_path(dir: &Path) -> PathBuf {
    dir.join("prefs.json")
}

/// Read + parse a persisted JSON sidecar file (prefs / favorites / crates).
///
/// - Missing/unreadable file (fresh install): `T::default()`, no side effects, no note.
/// - Parse error: the file is user data — quarantine it as `<path>.bak` so the next
///   atomic save cannot silently destroy it, fall back to `T::default()`, and return
///   a note for the caller to surface (M7; mirrors `load_set` rejecting bad files
///   instead of clobbering them).
fn load_sidecar<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
    what: &str,
) -> (T, Option<String>) {
    let Ok(json) = std::fs::read_to_string(path) else {
        return (T::default(), None);
    };
    match serde_json::from_str(&json) {
        Ok(v) => (v, None),
        Err(e) => {
            let bak = path.with_extension("json.bak");
            let note = match std::fs::rename(path, &bak) {
                Ok(()) => format!("{what} unreadable ({e}); kept as {}", bak.display()),
                Err(re) => format!("{what} unreadable ({e}); backup failed: {re}"),
            };
            (T::default(), Some(note))
        }
    }
}

/// Load prefs from `dir`. Missing file → default. A corrupt file is quarantined as
/// `prefs.json.bak` and reported in the returned note (see `load_sidecar`).
pub fn load_prefs(dir: &Path) -> (Prefs, Option<String>) {
    load_sidecar(&prefs_path(dir), "prefs")
}

/// Atomically write prefs to `dir/prefs.json`.
pub fn save_prefs(dir: &Path, prefs: &Prefs) -> anyhow::Result<()> {
    let path = prefs_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating prefs dir")?;
    }
    let json = serde_json::to_string_pretty(prefs).context("serializing prefs")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing prefs {}", path.display()))?;
    Ok(())
}

/// The current on-disk schema version. Increment when the format changes.
pub const CURRENT_SET_VERSION: u32 = 4;

/// On-disk lane: stores the profile *id* (not the static profile), rehydrated on load.
#[derive(Serialize, Deserialize)]
struct LaneDto {
    profile_id: String,
    pattern: Pattern,
    mute: bool,
    solo: bool,
    transpose: i8,
    octave: i8,
    /// Explicit routing override. Absent in old files → serde default `None`.
    #[serde(default)]
    route: Option<LaneRoute>,
    /// Per-voice mute set. Absent in old files → serde default empty vec.
    #[serde(default)]
    muted_voices: Vec<u8>,
    /// Scale for melodic lanes. Absent in old files → serde default `Scale::Chromatic`.
    #[serde(default)]
    scale: crate::music::scale::Scale,
    /// Per-lane root note override. Absent in old files → serde default `None`.
    #[serde(default)]
    root: Option<u8>,
    /// Per-lane swing override. Absent in old files → serde default `None`.
    #[serde(default)]
    swing: Option<f32>,
    /// Per-lane clock divisor override. Absent in old files → serde default `None`.
    #[serde(default)]
    clock_div: Option<u8>,
}

#[derive(Serialize, Deserialize)]
struct SetDto {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    id: persist::Id,
    name: String,
    bpm: f64,
    swing: f32,
    lanes: Vec<LaneDto>,
    /// Scenes stored in the set. Absent in old files → serde default `[]`.
    #[serde(default)]
    scenes: Vec<Scene>,
    /// Song-mode chains (M7). Absent in old files → serde default `[]`.
    #[serde(default)]
    chains: Vec<Chain>,
    /// MIDI clock-in port (M10). Absent in old files (pre-v4) → serde default `None`.
    #[serde(default)]
    clock_in_port: Option<PortRef>,
}

impl From<&Lane> for LaneDto {
    fn from(lane: &Lane) -> Self {
        LaneDto {
            profile_id: lane.profile.id.to_string(),
            pattern: lane.pattern.clone(),
            mute: lane.mute,
            solo: lane.solo,
            transpose: lane.transpose,
            octave: lane.octave,
            route: lane.route.clone(),
            muted_voices: lane.muted_voices.clone(),
            scale: lane.scale,
            root: lane.root,
            swing: lane.swing,
            clock_div: lane.clock_div,
        }
    }
}

impl From<&Set> for SetDto {
    fn from(set: &Set) -> Self {
        SetDto {
            version: CURRENT_SET_VERSION,
            id: set.id.clone(),
            name: set.name.clone(),
            bpm: set.bpm,
            swing: set.swing,
            lanes: set.lanes.iter().map(LaneDto::from).collect(),
            scenes: set.scenes.clone(),
            chains: set.chains.clone(),
            clock_in_port: set.clock_in_port.clone(),
        }
    }
}

/// Slugify a set name into a filesystem-safe stem: lowercase, runs of non-alphanumeric
/// collapsed to a single '-', leading/trailing '-' trimmed. Empty -> "set".
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
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "set".to_string()
    } else {
        trimmed
    }
}

/// Returns true if the value is missing, null, or an empty/all-zero string.
fn id_value_is_empty(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::String(s) => s.is_empty() || s == "0000000000000000",
        serde_json::Value::Null => true,
        _ => true,
    }
}

/// Migration: v0 (no version, no ids) → v1 (version=1, ids assigned to set + lane patterns).
fn migrate_v0_to_v1(v: &mut serde_json::Value) {
    v["version"] = serde_json::json!(1u32);
    if id_value_is_empty(&v["id"]) {
        v["id"] = serde_json::json!(persist::mint_id().as_str().to_string());
    }
    if let Some(lanes) = v["lanes"].as_array_mut() {
        for lane in lanes {
            let pat = &mut lane["pattern"];
            if id_value_is_empty(&pat["id"]) {
                pat["id"] = serde_json::json!(persist::mint_id().as_str().to_string());
            }
        }
    }
}

/// Migration: v1 → v2 (M7 adds `chains`; serde default supplies `[]`, no rewrite needed).
fn migrate_v1_to_v2(v: &mut serde_json::Value) {
    v["version"] = serde_json::json!(2u32);
}

/// Migration: v2 → v3 (M8 adds per-step `micro`/`cond`, per-pattern `cc`, per-lane
/// `swing`/`clock_div`; serde default supplies all new fields, no rewrite needed).
fn migrate_v2_to_v3(v: &mut serde_json::Value) {
    v["version"] = serde_json::json!(3u32);
}

/// Migration: v3 → v4 (M10 adds `clock_in_port`; serde default supplies `null`/`None`,
/// no rewrite needed).
fn migrate_v3_to_v4(v: &mut serde_json::Value) {
    v["version"] = serde_json::json!(4u32);
}

/// Run the migration ladder on a `serde_json::Value` before typed parse.
/// Rejects files saved by a newer midip; upgrades older files in-place.
pub fn migrate_set_value(v: &mut serde_json::Value) -> anyhow::Result<()> {
    // M9: a stray non-object .json in the sets dir (top-level array/number/string)
    // would panic in the v0 migration's `v["version"] = …` index-assign. Reject it.
    if !v.is_object() {
        return Err(anyhow!("set file root is not a JSON object"));
    }
    let version = v["version"].as_u64().unwrap_or(0) as u32;
    if version > CURRENT_SET_VERSION {
        return Err(anyhow!(
            "set was saved by a newer midip (v{}); not loading",
            version
        ));
    }
    let mut cur = version;
    while cur < CURRENT_SET_VERSION {
        match cur {
            0 => migrate_v0_to_v1(v),
            1 => migrate_v1_to_v2(v),
            2 => migrate_v2_to_v3(v),
            3 => migrate_v3_to_v4(v),
            _ => break,
        }
        cur += 1;
    }
    Ok(())
}

/// Serialize `set` to `<dir>/<slug>-<8hex>.json`. Returns the written path.
///
/// Mints a stable id for the set and each lane pattern if they are nil (so ids persist back
/// into the in-memory `Set` and remain stable across re-saves). The filename embeds the first
/// 8 hex digits of the set id so two sets with the same name never clobber each other, while
/// re-saving the same set (same id) always overwrites its own file atomically.
pub fn save_set(dir: &Path, set: &mut Set) -> anyhow::Result<PathBuf> {
    set.ensure_id();
    for lane in &mut set.lanes {
        lane.pattern.ensure_id();
    }
    std::fs::create_dir_all(dir).context("creating set store dir")?;
    let id_suffix = set.id.short();
    let path = dir.join(format!("{}-{}.json", slug(&set.name), id_suffix));
    let dto = SetDto::from(&*set);
    let json = serde_json::to_string_pretty(&dto).context("serializing set")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Load a set from a JSON file, running migration, typed parse, profile rehydration,
/// and `validate_and_repair`. Returns the set plus any repair notes.
pub fn load_set_with_report(path: &Path) -> anyhow::Result<(Set, Vec<String>)> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&json).context("parsing set JSON")?;
    migrate_set_value(&mut value).context("migrating set")?;
    let dto: SetDto = serde_json::from_value(value).context("deserializing set")?;
    let mut lanes = Vec::with_capacity(dto.lanes.len());
    let mut profile_notes: Vec<String> = Vec::new();
    for l in dto.lanes {
        let profile = match profile_by_id(&l.profile_id) {
            Some(p) => p,
            None => {
                // The set references a device no longer in the catalog (e.g. a
                // user-defined profile removed from devices.json, or a set shared
                // from another machine). Fall back to a same-kind generic so the
                // set still loads, and record the swap — never fail the whole load
                // over one missing device.
                let fallback_id = match l.pattern.kind() {
                    LaneKind::Drums => "generic-gm-drums",
                    LaneKind::Melodic => "generic-poly-synth",
                };
                let fallback = profile_by_id(fallback_id)
                    .ok_or_else(|| anyhow!("missing built-in fallback profile {fallback_id}"))?;
                profile_notes.push(format!(
                    "unknown device '{}' → {}",
                    l.profile_id, fallback.label
                ));
                fallback
            }
        };
        lanes.push(Lane {
            profile,
            pattern: l.pattern,
            mute: l.mute,
            solo: l.solo,
            transpose: l.transpose,
            octave: l.octave,
            route: l.route,
            muted_voices: l.muted_voices,
            scale: l.scale,
            root: l.root,
            swing: l.swing,
            clock_div: l.clock_div,
        });
    }
    let mut set = Set {
        name: dto.name,
        bpm: dto.bpm,
        swing: dto.swing,
        lanes,
        id: dto.id,
        scenes: dto.scenes,
        chains: dto.chains,
        clock_in_port: dto.clock_in_port,
    };
    let mut notes = profile_notes;
    notes.extend(validate_and_repair(&mut set));
    Ok((set, notes))
}

/// Load a set from a JSON file, rehydrating each lane's static profile via its id.
/// Runs the migration ladder before typed parse so old files (no version, no id) load correctly.
/// Applies `validate_and_repair` silently; use `load_set_with_report` to surface repair notes.
pub fn load_set(path: &Path) -> anyhow::Result<Set> {
    let (set, _notes) = load_set_with_report(path)?;
    Ok(set)
}

/// Clamp and repair all fields in a single `Pattern` to safe ranges.
///
/// Returns a list of human-readable notes describing what was changed.
/// Returns an empty `Vec` (and leaves `p` unchanged) when everything is already in range.
/// Never panics.
pub fn validate_and_repair_pattern(p: &mut Pattern) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();

    // length
    let orig_len = p.length;
    p.length = p.length.clamp(1, 64);
    if p.length != orig_len {
        notes.push(format!("length {}→{}", orig_len, p.length));
    }
    let target = p.length;

    // data resize + field clamping
    match &mut p.data {
        crate::pattern::model::PatternData::Drums(steps) => {
            if steps.len() != target {
                steps.resize_with(target, Vec::new);
                notes.push(format!("data resized to {}", target));
            }
            let mut hit_repaired = false;
            for step in steps.iter_mut() {
                for hit in step.iter_mut() {
                    let orig_note = hit.note;
                    hit.note = hit.note.clamp(0, 127);
                    let orig_vel = hit.vel;
                    hit.vel = hit.vel.clamp(1, 127);
                    let orig_prob = hit.prob;
                    hit.prob = hit.prob.clamp(0.0, 1.0);
                    let orig_ratchet = hit.ratchet;
                    hit.ratchet = hit.ratchet.clamp(1, 8);
                    if hit.note != orig_note
                        || hit.vel != orig_vel
                        || hit.prob != orig_prob
                        || hit.ratchet != orig_ratchet
                    {
                        hit_repaired = true;
                    }
                }
            }
            if hit_repaired {
                notes.push("drum hit fields clamped".to_string());
            }
        }
        crate::pattern::model::PatternData::Melodic(steps) => {
            if steps.len() != target {
                steps.resize_with(target, crate::pattern::model::MelodicStep::default);
                notes.push(format!("data resized to {}", target));
            }
            let mut note_repaired = false;
            for step in steps.iter_mut().flat_map(|s| s.iter_mut()) {
                let orig_vel = step.vel;
                step.vel = step.vel.clamp(0.5, 1.3);
                let orig_len = step.len;
                step.len = step.len.clamp(0.0, 64.0);
                let orig_prob = step.prob;
                step.prob = step.prob.clamp(0.0, 1.0);
                let orig_ratchet = step.ratchet;
                step.ratchet = step.ratchet.clamp(1, 8);
                if step.vel != orig_vel
                    || step.len != orig_len
                    || step.prob != orig_prob
                    || step.ratchet != orig_ratchet
                {
                    note_repaired = true;
                }
            }
            if note_repaired {
                notes.push("melodic note fields clamped".to_string());
            }
        }
    }

    // cc step locks (M8): cc number and value go straight to the wire — clamp to 0..=127.
    let mut cc_repaired = false;
    for lock in p.cc.iter_mut().flatten() {
        let orig_cc = lock.cc;
        lock.cc = lock.cc.min(127);
        let orig_val = lock.val;
        lock.val = lock.val.min(127);
        if lock.cc != orig_cc || lock.val != orig_val {
            cc_repaired = true;
        }
    }
    if cc_repaired {
        notes.push("cc lock fields clamped".to_string());
    }

    notes
}

/// Clamp and repair all fields in `set` to safe ranges.
///
/// Returns a list of human-readable notes describing what was changed.
/// Returns an empty `Vec` (and leaves `set` unchanged) when everything is already in range.
/// Never panics.
pub fn validate_and_repair(set: &mut Set) -> Vec<String> {
    let mut notes: Vec<String> = Vec::new();

    // ── top-level fields ──────────────────────────────────────────────────────
    let orig_bpm = set.bpm;
    set.bpm = set.bpm.clamp(20.0, 300.0);
    if set.bpm != orig_bpm {
        notes.push(format!("bpm clamped {:.4}→{:.4}", orig_bpm, set.bpm));
    }

    let orig_swing = set.swing;
    set.swing = set.swing.clamp(0.5, 0.66);
    if set.swing != orig_swing {
        notes.push(format!("swing clamped {:.4}→{:.4}", orig_swing, set.swing));
    }

    // A corrupt/foreign set file can carry an id that isn't well-formed (too short,
    // non-hex, or multibyte). Regenerate it here so downstream code that slices the
    // id (e.g. the save filename) always sees a valid 16-hex-char id. `is_valid()`
    // accepts the nil id, so a nil id is left for `ensure_id()` to mint on save.
    if !set.id.is_valid() {
        notes.push(format!(
            "set id '{}' malformed, regenerated",
            set.id.as_str()
        ));
        set.id = persist::mint_id();
    }

    // ── per-lane ──────────────────────────────────────────────────────────────
    for (lane_idx, lane) in set.lanes.iter_mut().enumerate() {
        let lane_num = lane_idx + 1;
        let pat_notes = validate_and_repair_pattern(&mut lane.pattern);
        for note in pat_notes {
            notes.push(format!("lane {} {}", lane_num, note));
        }
    }

    // ── scenes ────────────────────────────────────────────────────────────────
    let lane_count = set.lanes.len();
    for (scene_idx, scene) in set.scenes.iter_mut().enumerate() {
        // Clamp per-assignment fields first (only over existing assignments).
        for (assign_idx, a) in scene.assignments.iter_mut().enumerate() {
            let orig_transpose = a.transpose;
            a.transpose = a.transpose.clamp(-24, 24);
            if a.transpose != orig_transpose {
                notes.push(format!(
                    "scene {} assignment {} transpose clamped {}→{}",
                    scene_idx, assign_idx, orig_transpose, a.transpose
                ));
            }
            let orig_octave = a.octave;
            a.octave = a.octave.clamp(-4, 4);
            if a.octave != orig_octave {
                notes.push(format!(
                    "scene {} assignment {} octave clamped {}→{}",
                    scene_idx, assign_idx, orig_octave, a.octave
                ));
            }
        }

        // Reconcile assignment count to match lane count so T2 can safely index
        // assignments by lane index without bounds checks.
        let got = scene.assignments.len();
        if got > lane_count {
            // Too many: truncate excess assignments (spurious lanes no longer present).
            scene.assignments.truncate(lane_count);
            notes.push(format!(
                "scene {} truncated assignments {}→{}",
                scene_idx, got, lane_count
            ));
        } else if got < lane_count {
            // Too few: pad with a neutral assignment referencing the lane's current pattern.
            // Using the lane's current PatternRef::User id is the safest default —
            // it resolves immediately from the set's inline patterns and leaves performance
            // state (mute/solo/transpose/octave) at neutral values.
            for lane_idx in got..lane_count {
                scene
                    .assignments
                    .push(crate::pattern::model::LaneAssignment {
                        pattern: crate::pattern::refs::PatternRef::User(
                            set.lanes[lane_idx].pattern.id.clone(),
                        ),
                        mute: false,
                        solo: false,
                        transpose: 0,
                        octave: 0,
                    });
            }
            notes.push(format!(
                "scene {} padded assignments {}→{}",
                scene_idx, got, lane_count
            ));
        }
    }

    notes
}

/// Serialize `p` to `<dir>/<slug>-<8hex>.json` atomically. Returns the written path.
///
/// Mints a stable id for the pattern if it is nil, so it persists back into the
/// in-memory `Pattern` and remains stable across re-saves.
pub fn save_user_pattern(dir: &Path, p: &mut Pattern) -> anyhow::Result<PathBuf> {
    p.ensure_id();
    std::fs::create_dir_all(dir).context("creating user-pattern store dir")?;
    let id_suffix = p.id.short();
    let path = dir.join(format!("{}-{}.json", slug(&p.name), id_suffix));
    let json = serde_json::to_string_pretty(p).context("serializing pattern")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// All `*.json` pattern files in `dir` (sorted; empty if dir absent).
pub fn list_user_patterns(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Read + parse a `Pattern` from `path`, apply `validate_and_repair_pattern`, return the pattern.
///
/// The id is whatever was saved — `ensure_id` is NOT called on load. Missing `id` fields
/// deserialize to `Id::nil()` via the model's `#[serde(default)]`.
pub fn load_user_pattern(path: &Path) -> anyhow::Result<Pattern> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut p: Pattern =
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))?;
    validate_and_repair_pattern(&mut p);
    Ok(p)
}

/// Returns the path of the autosave recovery file under `dir` (never inside the sets dir).
pub fn recovery_path(dir: &Path) -> PathBuf {
    dir.join("recovery").join("autosave.json")
}

/// Write a snapshot of `set` to the recovery file under `dir` atomically.
///
/// Takes `&Set` (not `&mut Set`) so it never mints ids or mutates the live document.
/// The recovery file is a transient crash-recovery snapshot, not a deliberate save.
pub fn save_recovery(dir: &Path, set: &Set) -> anyhow::Result<()> {
    let path = recovery_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating recovery dir")?;
    }
    let dto = SetDto::from(set);
    let json = serde_json::to_string_pretty(&dto).context("serializing recovery set")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing recovery file {}", path.display()))?;
    Ok(())
}

/// Remove the recovery file under `dir`. Best-effort: "not found" errors are silently ignored.
pub fn clear_recovery(dir: &Path) {
    let path = recovery_path(dir);
    if let Err(e) = std::fs::remove_file(&path) {
        // Only ignore "not found"; surface other errors as a debug note (no panic).
        if e.kind() != std::io::ErrorKind::NotFound {
            // Best-effort: log nothing, just swallow. The caller never depends on this.
        }
    }
}

/// Returns true when the recovery file under `dir` exists on disk.
pub fn recovery_exists(dir: &Path) -> bool {
    recovery_path(dir).exists()
}

/// Path of the clean-shutdown marker file under `dir`.
pub fn clean_marker_path(dir: &Path) -> PathBuf {
    dir.join("recovery").join("clean")
}

/// Write an empty clean-shutdown marker atomically. Called on graceful exit.
pub fn mark_clean_shutdown(dir: &Path) -> anyhow::Result<()> {
    let path = clean_marker_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating recovery dir for clean marker")?;
    }
    persist::write_atomic(&path, b"1")
        .with_context(|| format!("writing clean marker {}", path.display()))?;
    Ok(())
}

/// Remove the clean-shutdown marker. Best-effort: not-found errors are silently ignored.
pub fn clear_clean_marker(dir: &Path) {
    let path = clean_marker_path(dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            // Best-effort: swallow silently.
        }
    }
}

/// Returns true when the clean-shutdown marker exists under `dir`.
pub fn clean_marker_exists(dir: &Path) -> bool {
    clean_marker_path(dir).exists()
}

/// Path of the first-run marker file under `dir`. Presence means onboarding
/// was completed (or dismissed) at least once; absence means first run.
pub fn first_run_marker_path(dir: &Path) -> PathBuf {
    dir.join("onboarded")
}

/// Returns true when the first-run marker is absent — i.e. the onboarding
/// walkthrough has never been completed or dismissed. Pure existence check:
/// never errors, never creates anything, never blocks startup.
pub fn is_first_run(dir: &Path) -> bool {
    !first_run_marker_path(dir).exists()
}

/// Write the first-run marker atomically. Mirrors `mark_clean_shutdown`.
/// Called when the onboarding walkthrough is dismissed or completed.
pub fn mark_onboarded(dir: &Path) -> anyhow::Result<()> {
    let path = first_run_marker_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating data dir for first-run marker")?;
    }
    persist::write_atomic(&path, b"1")
        .with_context(|| format!("writing first-run marker {}", path.display()))?;
    Ok(())
}

/// Returns true when an unclean shutdown is detected: a recovery file exists
/// but no clean-shutdown marker was written (i.e. the previous run crashed or was killed).
pub fn unclean_shutdown_detected(dir: &Path) -> bool {
    recovery_exists(dir) && !clean_marker_exists(dir)
}

/// All `*.json` set files in `dir` (non-recursive). Empty list if the dir is absent.
pub fn list_sets(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).context("listing set store dir")? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// The current on-disk schema version for favorites. Increment when format changes.
pub const CURRENT_FAVORITES_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Favorites {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub refs: Vec<PatternRef>,
}

impl Favorites {
    /// Toggle: if ref present, remove and return false; else push and return true.
    pub fn toggle(&mut self, r: PatternRef) -> bool {
        if let Some(pos) = self.refs.iter().position(|x| x == &r) {
            self.refs.remove(pos);
            false
        } else {
            self.refs.push(r);
            true
        }
    }

    pub fn contains(&self, r: &PatternRef) -> bool {
        self.refs.contains(r)
    }
}

pub fn favorites_path(dir: &Path) -> PathBuf {
    dir.join("favorites.json")
}

/// Load favorites from `dir`. Missing file → default. A corrupt file is quarantined
/// as `favorites.json.bak` and reported in the returned note (see `load_sidecar`).
pub fn load_favorites(dir: &Path) -> (Favorites, Option<String>) {
    load_sidecar(&favorites_path(dir), "favorites")
}

pub fn save_favorites(dir: &Path, favs: &Favorites) -> anyhow::Result<()> {
    let path = favorites_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating favorites dir")?;
    }
    let mut stamped = favs.clone();
    stamped.version = CURRENT_FAVORITES_VERSION;
    let json = serde_json::to_string_pretty(&stamped).context("serializing favorites")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing favorites {}", path.display()))?;
    Ok(())
}

// ── Crate model + store ───────────────────────────────────────────────────────

/// The current on-disk schema version for crates. Increment when the format changes.
pub const CURRENT_CRATES_VERSION: u32 = 1;

/// One slot in a crate: a pattern reference plus an optional display label override.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrateEntry {
    pub pattern: PatternRef,
    #[serde(default)]
    pub label: Option<String>,
}

/// A named, ordered collection of pattern references.
/// A pattern may appear in multiple crates; entries within one crate may also repeat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crate {
    #[serde(default)]
    pub id: persist::Id,
    pub name: String,
    #[serde(default)]
    pub entries: Vec<CrateEntry>,
}

/// Top-level index of all crates, persisted as a single `crates.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CrateIndex {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub crates: Vec<Crate>,
}

impl CrateIndex {
    /// Push a new crate with a fresh id; returns the index of the new crate.
    pub fn add_crate(&mut self, name: String) -> usize {
        self.crates.push(Crate {
            id: persist::mint_id(),
            name,
            entries: Vec::new(),
        });
        self.crates.len() - 1
    }

    /// Remove the crate at `idx`. No-op if out of range.
    pub fn remove_crate(&mut self, idx: usize) {
        if idx < self.crates.len() {
            self.crates.remove(idx);
        }
    }

    /// Rename the crate at `idx`, keeping its stable id. No-op if out of range.
    pub fn rename_crate(&mut self, idx: usize, name: String) {
        if let Some(c) = self.crates.get_mut(idx) {
            c.name = name;
        }
    }

    /// Clone the crate at `idx` with a fresh id and name `"<name> copy"`.
    /// Returns the index of the new crate, or `None` if `idx` is out of range.
    pub fn duplicate_crate(&mut self, idx: usize) -> Option<usize> {
        let src = self.crates.get(idx)?.clone();
        let new_crate = Crate {
            id: persist::mint_id(),
            name: format!("{} copy", src.name),
            entries: src.entries,
        };
        self.crates.push(new_crate);
        Some(self.crates.len() - 1)
    }

    /// Append `entry` to the crate at `crate_idx`. No-op if out of range.
    pub fn add_entry(&mut self, crate_idx: usize, entry: CrateEntry) {
        if let Some(c) = self.crates.get_mut(crate_idx) {
            c.entries.push(entry);
        }
    }

    /// Remove the entry at `entry_idx` from the crate at `crate_idx`.
    /// No-op if either index is out of range.
    pub fn remove_entry(&mut self, crate_idx: usize, entry_idx: usize) {
        if let Some(c) = self.crates.get_mut(crate_idx) {
            if entry_idx < c.entries.len() {
                c.entries.remove(entry_idx);
            }
        }
    }

    /// Move the entry at `from` to position `to` within the crate at `crate_idx`.
    /// No-op if `crate_idx`, `from`, or `to` are out of range.
    pub fn reorder_entry(&mut self, crate_idx: usize, from: usize, to: usize) {
        let Some(c) = self.crates.get_mut(crate_idx) else {
            return;
        };
        let len = c.entries.len();
        if from >= len || to >= len {
            return;
        }
        let entry = c.entries.remove(from);
        c.entries.insert(to, entry);
    }
}

/// Path of the crates index file under `dir`.
pub fn crates_path(dir: &Path) -> PathBuf {
    dir.join("crates.json")
}

/// Load the crate index from `dir/crates.json`. Missing file → default. A corrupt
/// file is quarantined as `crates.json.bak` and reported in the returned note
/// (see `load_sidecar`).
pub fn load_crates(dir: &Path) -> (CrateIndex, Option<String>) {
    load_sidecar(&crates_path(dir), "crates")
}

/// Atomically write the crate index to `dir/crates.json`, stamping the current version.
pub fn save_crates(dir: &Path, index: &CrateIndex) -> anyhow::Result<()> {
    let path = crates_path(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating crates dir")?;
    }
    let mut stamped = index.clone();
    stamped.version = CURRENT_CRATES_VERSION;
    let json = serde_json::to_string_pretty(&stamped).context("serializing crates")?;
    persist::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing crates {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::model::{DrumHit, MelodicNote, PatternData, Set};

    // ── Task 3: validate_and_repair ───────────────────────────────────────────

    #[test]
    fn validate_clamps_bpm_swing() {
        let mut set = Set::default_set(default_profiles());
        set.bpm = 0.0;
        set.swing = 2.0;
        let notes = validate_and_repair(&mut set);
        assert_eq!(set.bpm, 20.0, "bpm must be clamped to 20");
        assert_eq!(set.swing, 0.66, "swing must be clamped to 0.66");
        assert!(!notes.is_empty(), "repair notes must be non-empty");
    }

    #[test]
    fn validate_resizes_data_to_length() {
        let mut set = Set::default_set(default_profiles());
        // Lane 0 is drums. Give it length=16 but only 4 data steps.
        set.lanes[0].pattern.length = 16;
        if let PatternData::Drums(ref mut v) = set.lanes[0].pattern.data {
            v.truncate(4);
        }
        let notes = validate_and_repair(&mut set);
        if let PatternData::Drums(ref v) = set.lanes[0].pattern.data {
            assert_eq!(v.len(), 16, "data must be resized to match length=16");
        }
        assert!(!notes.is_empty());

        // Also test length=0 is clamped to 1 and data resized.
        let mut set2 = Set::default_set(default_profiles());
        set2.lanes[0].pattern.length = 0;
        if let PatternData::Drums(ref mut v) = set2.lanes[0].pattern.data {
            v.clear();
        }
        let notes2 = validate_and_repair(&mut set2);
        assert_eq!(
            set2.lanes[0].pattern.length, 1,
            "length 0 must be clamped to 1"
        );
        if let PatternData::Drums(ref v) = set2.lanes[0].pattern.data {
            assert_eq!(v.len(), 1, "data must be resized to 1");
        }
        assert!(!notes2.is_empty());
    }

    #[test]
    fn validate_clamps_drum_vel_prob_ratchet() {
        let mut set = Set::default_set(default_profiles());
        // Lane 0 is drums. Place a bad hit on step 0.
        if let PatternData::Drums(ref mut v) = set.lanes[0].pattern.data {
            v[0] = vec![DrumHit {
                note: 200,
                vel: 200,
                prob: 5.0,
                ratchet: 99,
                micro: 0,
                cond: TrigCond::Always,
            }];
        }
        let notes = validate_and_repair(&mut set);
        if let PatternData::Drums(ref v) = set.lanes[0].pattern.data {
            let hit = &v[0][0];
            assert_eq!(hit.note, 127, "note must be clamped to 127");
            assert_eq!(hit.vel, 127, "vel must be clamped to 127");
            assert_eq!(hit.prob, 1.0, "prob must be clamped to 1.0");
            assert_eq!(hit.ratchet, 8, "ratchet must be clamped to 8");
        }
        assert!(!notes.is_empty());
    }

    #[test]
    fn validate_clamps_cc_locks_and_roundtrips_to_valid_wire_bytes() {
        use crate::pattern::model::CcLock;
        // M8: cc/val feed ControlChange data bytes directly — out-of-range values
        // (e.g. a hand-edited file) must be clamped to the 7-bit wire range.
        let mut set = Set::default_set(default_profiles());
        if let PatternData::Drums(ref mut v) = set.lanes[0].pattern.data {
            v[0] = vec![DrumHit {
                note: 36,
                vel: 200,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }];
        }
        set.lanes[0]
            .pattern
            .set_step_cc(0, vec![CcLock { cc: 200, val: 200 }]);
        let notes = validate_and_repair(&mut set);
        assert!(
            notes.iter().any(|n| n.contains("cc lock")),
            "cc repair must be reported; got {notes:?}"
        );
        let lock = &set.lanes[0].pattern.step_cc(0)[0];
        assert_eq!(lock.cc, 127, "cc number clamped to 127");
        assert_eq!(lock.val, 127, "cc value clamped to 127");
        // Round-trip the repaired values onto the wire: all data bytes valid.
        let msgs = [
            crate::midi::message::MidiMessage::NoteOn {
                channel: 0,
                note: 36,
                vel: match &set.lanes[0].pattern.data {
                    PatternData::Drums(v) => v[0][0].vel,
                    _ => unreachable!(),
                },
            },
            crate::midi::message::MidiMessage::ControlChange {
                channel: 0,
                controller: lock.cc,
                value: lock.val,
            },
        ];
        for m in msgs {
            let bytes = m.to_bytes();
            assert!(bytes[0] >= 0x80, "status byte");
            for b in &bytes[1..] {
                assert!(*b <= 0x7F, "data byte {b:#04x} must be 7-bit");
            }
        }
    }

    #[test]
    fn validate_clean_set_is_unchanged_and_returns_no_notes() {
        let mut set = Set::default_set(default_profiles());
        let original = set.clone();
        let notes = validate_and_repair(&mut set);
        assert!(notes.is_empty(), "clean set must return no repair notes");
        assert_eq!(set, original, "clean set must be unchanged");
    }

    /// H6 regression: a malformed set id (too short for the old `[..8]` byte slice)
    /// must be repaired to a well-formed 16-hex id by `validate_and_repair`.
    #[test]
    fn validate_repairs_malformed_short_set_id() {
        let mut set = Set::default_set(default_profiles());
        set.id = serde_json::from_str(r#""abc""#).unwrap();
        let notes = validate_and_repair(&mut set);
        assert_eq!(
            set.id.as_str().len(),
            16,
            "id must be repaired to canonical length"
        );
        assert!(set.id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(
            notes.iter().any(|n| n.contains("id") && n.contains("abc")),
            "expected a repair note about the malformed id; got: {notes:?}"
        );
    }

    /// H6 regression: a multibyte set id must also be repaired (not merely tolerated),
    /// since it can never be a well-formed 16-hex id.
    #[test]
    fn validate_repairs_malformed_multibyte_set_id() {
        let mut set = Set::default_set(default_profiles());
        set.id = serde_json::from_str(r#""日本語abc😀""#).unwrap();
        let notes = validate_and_repair(&mut set);
        assert_eq!(
            set.id.as_str().len(),
            16,
            "id must be repaired to canonical length"
        );
        assert!(set.id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(
            !notes.is_empty(),
            "expected a repair note about the malformed id"
        );
    }

    /// H6 regression: loading a set file with a corrupt/foreign id (`"id":"abc"`, fewer
    /// than 8 bytes) must not panic, must repair the id in-memory, and a subsequent
    /// `save_set` (which slices the id for the filename) must also not panic.
    #[test]
    fn load_and_resave_set_with_short_id_does_not_panic() {
        let dir = unique_dir("h6-short-id");
        let profile_id = default_profiles()[0].id;
        let json = format!(
            r#"{{
                "version": {ver},
                "id": "abc",
                "name": "corrupt",
                "bpm": 120.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{profile_id}",
                    "pattern": {{"name": "p", "desc": "", "length": 1, "data": {{"Drums": [[]]}}}},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#,
            ver = CURRENT_SET_VERSION
        );
        let path = dir.join("corrupt.json");
        std::fs::write(&path, &json).unwrap();

        // Load must not panic (old code: `&set.id.as_str()[..8]` would panic at save,
        // but a naive fix could still panic on load if the id were sliced there too).
        let mut loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.id.as_str().len(),
            16,
            "malformed id must be repaired on load"
        );

        // Save must not panic (this is where the original bug crashed the app).
        let saved_path = save_set(&dir, &mut loaded).unwrap();
        assert!(saved_path.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// H6 regression: same as above but with a multibyte id, which used to be able to
    /// panic on a UTF-8 char boundary rather than a plain out-of-range index.
    #[test]
    fn load_and_resave_set_with_multibyte_id_does_not_panic() {
        let dir = unique_dir("h6-multibyte-id");
        let profile_id = default_profiles()[0].id;
        let json = format!(
            r#"{{
                "version": {ver},
                "id": "日本語abc😀",
                "name": "corrupt-mb",
                "bpm": 120.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{profile_id}",
                    "pattern": {{"name": "p", "desc": "", "length": 1, "data": {{"Drums": [[]]}}}},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#,
            ver = CURRENT_SET_VERSION
        );
        let path = dir.join("corrupt-mb.json");
        std::fs::write(&path, &json).unwrap();

        let mut loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.id.as_str().len(),
            16,
            "multibyte id must be repaired on load"
        );

        let saved_path = save_set(&dir, &mut loaded).unwrap();
        assert!(saved_path.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A unique temp subdir per test run, so parallel tests don't collide.
    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("midip-store-{}-{}", tag, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A minimal v0 JSON: no version, no ids. Simulates a file saved before T2.
    const OLD_SET_JSON_NO_VERSION: &str = r#"{
        "name": "old jam",
        "bpm": 120.0,
        "swing": 0.5,
        "lanes": [
            {
                "profile_id": "drums-sp404",
                "pattern": {
                    "name": "beat",
                    "desc": "",
                    "length": 1,
                    "data": {"Drums": [[]]}
                },
                "mute": false,
                "solo": false,
                "transpose": 0,
                "octave": 0
            }
        ]
    }"#;

    #[test]
    fn migrate_v0_assigns_version_and_ids() {
        let mut v: serde_json::Value = serde_json::from_str(OLD_SET_JSON_NO_VERSION).unwrap();
        migrate_set_value(&mut v).unwrap();
        assert_eq!(v["version"], CURRENT_SET_VERSION);
        assert!(
            v["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "set id must be non-empty after migration"
        );
        // Each lane's pattern should also have a non-empty id
        let lane_pat_id = &v["lanes"][0]["pattern"]["id"];
        assert!(
            lane_pat_id.as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "lane pattern id must be non-empty after migration"
        );
    }

    #[test]
    fn newer_version_is_rejected_not_misparsed() {
        let mut v = serde_json::json!({
            "version": 9999u32,
            "name": "x",
            "bpm": 120.0,
            "swing": 0.5,
            "lanes": []
        });
        assert!(
            migrate_set_value(&mut v).is_err(),
            "a future-version file must be rejected"
        );
    }

    #[test]
    fn non_object_root_is_rejected_not_panicking() {
        // M9: a stray non-object .json (top-level array/number/string) previously
        // panicked in the v0 migration's `v["version"] = …` index-assign.
        for mut v in [
            serde_json::json!([1, 2, 3]),
            serde_json::json!(42),
            serde_json::json!("hello"),
        ] {
            assert!(
                migrate_set_value(&mut v).is_err(),
                "non-object root {v} must be rejected"
            );
        }
    }

    #[test]
    fn load_set_non_object_root_errors_instead_of_panicking() {
        // M9 end-to-end: a stray `[1,2,3]` file in the sets dir loads as Err, no panic.
        let dir = unique_dir("non-object-root");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stray.json");
        std::fs::write(&path, b"[1,2,3]").unwrap();
        assert!(load_set(&path).is_err(), "non-object root must be an error");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn already_v1_file_migrates_to_current_version() {
        let id = persist::Id::generate(0xABCD, 1);
        let mut v = serde_json::json!({
            "version": 1u32,
            "id": id.as_str(),
            "name": "current",
            "bpm": 120.0,
            "swing": 0.5,
            "lanes": []
        });
        migrate_set_value(&mut v).unwrap();
        assert_eq!(v["version"], CURRENT_SET_VERSION);
        // id must be preserved through migration
        assert_eq!(v["id"].as_str().unwrap(), id.as_str());
    }

    #[test]
    fn save_then_load_round_trips_a_set() {
        let dir = unique_dir("roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "My Jam".to_string();
        set.bpm = 124.0;
        set.swing = 0.56;
        // Make lane 1 (melodic) non-trivial so we exercise note serialization.
        if let PatternData::Melodic(steps) = &mut set.lanes[1].pattern.data {
            steps[0] = crate::pattern::model::MelodicStep::from(vec![MelodicNote {
                semi: 7,
                vel: 1.3,
                slide: true,
                len: 0.5,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }]);
        }
        set.lanes[0].mute = true;
        set.lanes[2].transpose = 3;

        // save_set takes &mut Set and mints ids on first save.
        let path = save_set(&dir, &mut set).unwrap();
        assert!(path.exists());

        let loaded = load_set(&path).unwrap();
        // save_set calls ensure_id, so both set and loaded now have the same non-nil id.
        assert_eq!(loaded.id, set.id);
        assert!(!loaded.id.is_nil(), "id must be non-nil after save");
        assert_eq!(loaded.name, set.name);
        assert_eq!(loaded.bpm, set.bpm);
        assert_eq!(loaded.swing, set.swing);
        assert_eq!(loaded.lanes.len(), set.lanes.len());
        for (a, b) in loaded.lanes.iter().zip(set.lanes.iter()) {
            assert_eq!(a.pattern, b.pattern);
            assert_eq!(a.mute, b.mute);
            assert_eq!(a.transpose, b.transpose);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_old_file_without_version_succeeds_and_assigns_ids() {
        // Write a raw v0 file (no version, no id) and verify load_set migrates it.
        let dir = unique_dir("old-file");
        // We need a valid profile_id; use one from default_profiles.
        let profile_id = default_profiles()[0].id;
        let old_json = format!(
            r#"{{
                "name": "legacy",
                "bpm": 100.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{}",
                    "pattern": {{
                        "name": "old",
                        "desc": "",
                        "length": 1,
                        "data": {{"Drums": [[]]}}
                    }},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#,
            profile_id
        );
        let path = dir.join("legacy.json");
        std::fs::write(&path, &old_json).unwrap();

        let loaded = load_set(&path).unwrap();
        assert_eq!(loaded.name, "legacy");
        // Migration assigns a non-nil id to the set
        assert!(!loaded.id.is_nil(), "migrated set must have a non-nil id");
        // And to the lane pattern
        assert!(
            !loaded.lanes[0].pattern.id.is_nil(),
            "migrated lane pattern must have a non-nil id"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_sets_finds_saved_files() {
        let dir = unique_dir("list");
        let mut set = Set::default_set(default_profiles());
        set.name = "Listed Set".to_string();
        let path = save_set(&dir, &mut set).unwrap();

        let listed = list_sets(&dir).unwrap();
        assert!(listed.iter().any(|p| p == &path));
        assert!(listed
            .iter()
            .all(|p| p.extension().and_then(|e| e.to_str()) == Some("json")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn slug_from_name_is_used_for_filename() {
        let dir = unique_dir("slug");
        let mut set = Set::default_set(default_profiles());
        set.name = "Acid Jam #3!".to_string();
        let path = save_set(&dir, &mut set).unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        // lowercased, non-alphanumeric collapsed to '-', followed by '-<8hex>.json'
        assert!(
            fname.starts_with("acid-jam-3-"),
            "filename must start with slug 'acid-jam-3-' but was: {fname}"
        );
        assert!(
            fname.ends_with(".json"),
            "filename must end with .json but was: {fname}"
        );
        // slug prefix + '-' + 8 hex chars + '.json'
        let hex_part = fname
            .strip_prefix("acid-jam-3-")
            .unwrap()
            .strip_suffix(".json")
            .unwrap();
        assert_eq!(
            hex_part.len(),
            8,
            "id suffix must be 8 hex chars but was: {hex_part}"
        );
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "id suffix must be hex but was: {hex_part}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 4: atomic + versioned + id-named save; load migrates + validates ────

    #[test]
    fn save_load_roundtrip_preserves_ids_and_version() {
        let dir = unique_dir("t4-roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "Preserve IDs".to_string();

        let path = save_set(&dir, &mut set).unwrap();
        assert!(!set.id.is_nil(), "ensure_id must mint a non-nil id");
        for lane in &set.lanes {
            assert!(!lane.pattern.id.is_nil(), "lane pattern id must be minted");
        }

        let loaded = load_set(&path).unwrap();
        assert_eq!(loaded.id, set.id, "set id must survive round-trip");
        assert_eq!(loaded.lanes.len(), set.lanes.len());
        for (a, b) in loaded.lanes.iter().zip(set.lanes.iter()) {
            assert_eq!(
                a.pattern.id, b.pattern.id,
                "lane pattern id must survive round-trip"
            );
        }

        // Verify version is stamped in the file
        let json = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["version"].as_u64().unwrap(), CURRENT_SET_VERSION as u64);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_filename_includes_id_suffix() {
        let dir = unique_dir("t4-suffix");
        let mut set = Set::default_set(default_profiles());
        set.name = "My Beat".to_string();

        let path = save_set(&dir, &mut set).unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        let id_hex = &set.id.as_str()[..8];

        assert!(
            fname.starts_with("my-beat-"),
            "filename must start with slug: {fname}"
        );
        assert!(
            fname.contains(id_hex),
            "filename must contain first 8 hex of id ({id_hex}): {fname}"
        );
        assert!(
            fname.ends_with(".json"),
            "filename must end with .json: {fname}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_sets_same_name_dont_collide() {
        let dir = unique_dir("t4-no-collide");
        let mut set_a = Set::default_set(default_profiles());
        set_a.name = "jam".to_string();
        let mut set_b = Set::default_set(default_profiles());
        set_b.name = "jam".to_string();

        let path_a = save_set(&dir, &mut set_a).unwrap();
        let path_b = save_set(&dir, &mut set_b).unwrap();

        assert_ne!(
            path_a, path_b,
            "two sets with same name must get different files"
        );
        assert!(path_a.exists(), "file A must exist");
        assert!(path_b.exists(), "file B must exist");

        // Both should show up in list_sets
        let listed = list_sets(&dir).unwrap();
        assert_eq!(listed.len(), 2, "must have exactly two distinct files");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resave_same_set_overwrites_one_file() {
        let dir = unique_dir("t4-resave");
        let mut set = Set::default_set(default_profiles());
        set.name = "single".to_string();

        let path1 = save_set(&dir, &mut set).unwrap();
        set.bpm = 140.0; // mutate content
        let path2 = save_set(&dir, &mut set).unwrap();

        assert_eq!(path1, path2, "re-saving same set must produce same path");

        let listed = list_sets(&dir).unwrap();
        assert_eq!(listed.len(), 1, "must be exactly one file after two saves");

        // Updated content is present
        let loaded = load_set(&path2).unwrap();
        assert_eq!(loaded.bpm, 140.0, "re-save must update content");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_applies_repairs_via_report() {
        let dir = unique_dir("t4-repair");
        let mut set = Set::default_set(default_profiles());
        let path = save_set(&dir, &mut set).unwrap();

        // Tamper: set bpm to 0 (out of range)
        let json = std::fs::read_to_string(&path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["bpm"] = serde_json::json!(0.0);
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let (repaired, notes) = load_set_with_report(&path).unwrap();
        assert_eq!(repaired.bpm, 20.0, "bpm must be clamped to 20 after load");
        assert!(
            !notes.is_empty(),
            "repair notes must be non-empty for out-of-range bpm"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 5: LaneRoute serde + old-file backward compat ───────────────────

    #[test]
    fn lane_route_serde_roundtrips() {
        use crate::pattern::model::{LaneRoute, PortRef};
        let dir = unique_dir("t5-route-roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "route test".to_string();
        // Assign an explicit route to lane 0
        set.lanes[0].route = Some(LaneRoute {
            port: PortRef {
                stable_key: "MY-PORT".to_string(),
                name: "My Port".to_string(),
            },
            channel: 3,
            clock_out: false,
        });

        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.lanes[0].route, set.lanes[0].route,
            "explicit route must survive save/load round-trip"
        );
        // Lanes without explicit route stay None
        assert!(
            loaded.lanes[1].route.is_none(),
            "lane with no explicit route must load as None"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_set_json_without_route_loads_as_none() {
        use crate::pattern::model::LaneKind;
        let dir = unique_dir("t5-old-no-route");
        let profile_id = default_profiles()[0].id;
        // Construct a JSON that has NO "route" key in the lane (old format)
        let old_json = format!(
            r#"{{
                "name": "legacy no-route",
                "bpm": 120.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{profile_id}",
                    "pattern": {{
                        "name": "beat",
                        "desc": "",
                        "length": 1,
                        "data": {{"Drums": [[]]}}
                    }},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#
        );
        let path = dir.join("legacy.json");
        std::fs::write(&path, &old_json).unwrap();

        let loaded = load_set(&path).unwrap();
        assert!(
            loaded.lanes[0].route.is_none(),
            "old JSON without route key must load with route=None"
        );
        // effective_route must still derive from profile
        let r = loaded.lanes[0].effective_route();
        assert_eq!(r.channel, default_profiles()[0].channel);
        assert_eq!(r.port.stable_key, default_profiles()[0].port_match);
        assert!(r.clock_out);
        assert_eq!(loaded.lanes[0].pattern.kind(), LaneKind::Drums);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_set_falls_back_on_unknown_profile_id() {
        let dir = unique_dir("unknown-profile");
        let mut set = Set::default_set(default_profiles());
        let path = save_set(&dir, &mut set).unwrap();

        // Mutate the saved JSON to introduce a bogus profile_id on lane 0 (drums).
        let json_str = std::fs::read_to_string(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        json["lanes"][0]["profile_id"] = serde_json::json!("nonexistent-id");
        std::fs::write(&path, json.to_string()).unwrap();

        // Loading must SUCCEED, swapping the missing device for a same-kind generic
        // and reporting a repair note — instead of failing the whole set.
        let (loaded, notes) = load_set_with_report(&path).unwrap();
        assert_eq!(loaded.lanes[0].profile.id, "generic-gm-drums");
        assert_eq!(loaded.lanes[0].pattern.kind(), LaneKind::Drums);
        assert!(
            notes.iter().any(|n| n.contains("nonexistent-id")),
            "expected a repair note mentioning the unknown id; got: {notes:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// §2.6: muted_voices persists through save+load; old files (no field) load as empty.
    #[test]
    fn lane_muted_voices_serde_roundtrips() {
        let dir = unique_dir("muted-voices-roundtrip");
        let mut set = Set::default_set(default_profiles());
        // Set muted_voices on lane 0 (drums).
        set.lanes[0].muted_voices = vec![36, 42];

        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();

        assert_eq!(
            loaded.lanes[0].muted_voices,
            vec![36, 42],
            "muted_voices must survive save+load"
        );
        // Lanes without muted_voices set must come back as empty.
        assert!(
            loaded.lanes[1].muted_voices.is_empty(),
            "lane without muted_voices must load as empty"
        );

        // Verify old JSON (no muted_voices field) loads with empty vec.
        let json_str = std::fs::read_to_string(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        json["lanes"][0]
            .as_object_mut()
            .unwrap()
            .remove("muted_voices");
        let tmp = dir.join("old-format.json");
        std::fs::write(&tmp, json.to_string()).unwrap();
        let old_loaded = load_set(&tmp).unwrap();
        assert!(
            old_loaded.lanes[0].muted_voices.is_empty(),
            "old JSON without muted_voices must load as empty vec"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 9: recovery file helpers ────────────────────────────────────────

    #[test]
    fn save_recovery_writes_to_recovery_path_not_set_dir() {
        // Unique temp dir so this test never shares a path with another.
        let dir = unique_dir("recovery-path");

        // Verify recovery_path(dir) ends in recovery/autosave.json.
        let rpath = recovery_path(&dir);
        assert!(
            rpath.ends_with("recovery/autosave.json"),
            "recovery_path must end with recovery/autosave.json but was: {}",
            rpath.display()
        );

        // save_recovery then recovery_exists → true.
        let set = Set::default_set(default_profiles());
        save_recovery(&dir, &set).unwrap();
        assert!(
            recovery_exists(&dir),
            "recovery_exists must be true after save_recovery"
        );

        // The written file must NOT be inside the sets dir.
        let sets_dir = dir.join("sets");
        assert!(
            !rpath.starts_with(&sets_dir),
            "recovery file must not be in the sets dir"
        );

        // Clean up.
        clear_recovery(&dir);
        assert!(
            !recovery_exists(&dir),
            "recovery_exists must be false after clear_recovery"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_recovery_removes_file() {
        // Unique temp dir so this test never shares a path with another.
        let dir = unique_dir("recovery-clear");
        let set = Set::default_set(default_profiles());
        save_recovery(&dir, &set).unwrap();
        assert!(recovery_exists(&dir));
        clear_recovery(&dir);
        assert!(
            !recovery_exists(&dir),
            "recovery file must be gone after clear_recovery"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 10: clean-shutdown marker + unclean detection ───────────────────

    #[test]
    fn unclean_detected_when_recovery_present_and_no_marker() {
        let dir = unique_dir("t10-unclean");
        let set = Set::default_set(default_profiles());
        // Write recovery but NO clean marker → unclean shutdown detected.
        save_recovery(&dir, &set).unwrap();
        assert!(
            unclean_shutdown_detected(&dir),
            "must detect unclean shutdown when recovery exists and no clean marker"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clean_marker_suppresses_recovery_prompt() {
        let dir = unique_dir("t10-clean");
        let set = Set::default_set(default_profiles());
        // Both recovery AND clean marker present → clean exit, no prompt.
        save_recovery(&dir, &set).unwrap();
        mark_clean_shutdown(&dir).unwrap();
        assert!(
            !unclean_shutdown_detected(&dir),
            "clean marker must suppress unclean detection even when recovery exists"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_clean_marker_removes_it() {
        let dir = unique_dir("t10-clear-marker");
        mark_clean_shutdown(&dir).unwrap();
        assert!(clean_marker_exists(&dir), "marker must exist after writing");
        clear_clean_marker(&dir);
        assert!(
            !clean_marker_exists(&dir),
            "marker must be gone after clear_clean_marker"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 9 (Phase 2): first-run marker ───────────────────────────────────

    #[test]
    fn first_run_marker_flips_after_onboarding() {
        let dir = unique_dir("t9-first-run");
        assert!(is_first_run(&dir), "absent marker must read as first run");
        mark_onboarded(&dir).unwrap();
        assert!(
            !is_first_run(&dir),
            "first run must be over once the marker is written"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_first_run_true_for_missing_dir() {
        // A data dir that does not exist at all is still a first run — the
        // check must not error or create anything.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let absent = std::env::temp_dir().join(format!("midip-t9-absent-{}", nanos));
        assert!(is_first_run(&absent));
        assert!(!absent.exists(), "is_first_run must not create the dir");
    }

    // ── Mirror prefs (M2.5-T2) ───────────────────────────────────────────────

    #[test]
    fn prefs_roundtrip() {
        let dir = unique_dir("prefs-roundtrip");
        let prefs = Prefs { mirror_on: true };
        save_prefs(&dir, &prefs).unwrap();
        let (loaded, note) = load_prefs(&dir);
        assert_eq!(loaded, prefs, "mirror_on must survive save/load round-trip");
        assert!(note.is_none(), "clean round-trip emits no note");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_prefs_missing_returns_default() {
        let dir = unique_dir("prefs-missing");
        // Do NOT create prefs.json — load must return default (mirror_on=false).
        let (loaded, note) = load_prefs(&dir);
        assert_eq!(loaded, Prefs::default());
        assert!(
            !loaded.mirror_on,
            "missing prefs must default to mirror_on=false"
        );
        assert!(note.is_none(), "missing file is a fresh install, no note");
    }

    #[test]
    fn load_prefs_corrupt_is_quarantined_not_destroyed() {
        // M7: a parse error must NOT silently reset to default and let the next
        // save clobber the user's file — the corrupt bytes are kept as .bak.
        let dir = unique_dir("prefs-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = prefs_path(&dir);
        std::fs::write(&path, b"{not json").unwrap();
        let (loaded, note) = load_prefs(&dir);
        assert_eq!(
            loaded,
            Prefs::default(),
            "corrupt prefs fall back to default"
        );
        assert!(note.is_some(), "corruption is surfaced to the caller");
        assert!(!path.exists(), "corrupt file moved out of the save path");
        let bak = path.with_extension("json.bak");
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            b"{not json",
            "original bytes preserved in {}",
            bak.display()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 4: user-pattern store ───────────────────────────────────────────

    fn make_drum_pattern(name: &str) -> Pattern {
        Pattern {
            name: name.to_string(),
            desc: "test desc".to_string(),
            length: 4,
            data: PatternData::Drums(vec![
                vec![DrumHit {
                    note: 36,
                    vel: 100,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }],
                vec![],
                vec![DrumHit {
                    note: 42,
                    vel: 80,
                    prob: 0.75,
                    ratchet: 2,
                    micro: 0,
                    cond: TrigCond::Always,
                }],
                vec![],
            ]),
            id: persist::Id::nil(),
            cc: Default::default(),
        }
    }

    #[test]
    fn save_load_user_pattern_roundtrip_with_stable_id() {
        let dir = unique_dir("t4-pat-roundtrip");
        let mut p = make_drum_pattern("My Beat");

        let path = save_user_pattern(&dir, &mut p).unwrap();
        assert!(path.exists(), "saved file must exist");
        assert!(!p.id.is_nil(), "ensure_id must mint a non-nil id");

        let loaded = load_user_pattern(&path).unwrap();
        assert_eq!(loaded.name, p.name, "name must survive round-trip");
        assert_eq!(loaded.id, p.id, "id must survive round-trip");
        assert!(!loaded.id.is_nil(), "loaded id must be non-nil");
        assert_eq!(loaded.data, p.data, "data must survive round-trip");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn user_pattern_filename_has_id_suffix() {
        let dir = unique_dir("t4-pat-filename");
        let mut p = make_drum_pattern("Acid Loop #7!");

        let path = save_user_pattern(&dir, &mut p).unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        let id_hex = &p.id.as_str()[..8];

        // slug of "Acid Loop #7!" → "acid-loop-7"
        assert!(
            fname.starts_with("acid-loop-7-"),
            "filename must start with slug 'acid-loop-7-' but was: {fname}"
        );
        assert!(
            fname.contains(id_hex),
            "filename must contain first 8 hex of id ({id_hex}): {fname}"
        );
        assert!(
            fname.ends_with(".json"),
            "filename must end with .json: {fname}"
        );

        // slug prefix + '-' + 8 hex chars + '.json'
        let hex_part = fname
            .strip_prefix("acid-loop-7-")
            .unwrap()
            .strip_suffix(".json")
            .unwrap();
        assert_eq!(
            hex_part.len(),
            8,
            "id suffix must be 8 hex chars but was: {hex_part}"
        );
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "id suffix must be hex but was: {hex_part}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_user_patterns_finds_saved() {
        let dir = unique_dir("t4-pat-list");
        let mut p1 = make_drum_pattern("Alpha");
        let mut p2 = make_drum_pattern("Beta");

        let path1 = save_user_pattern(&dir, &mut p1).unwrap();
        let path2 = save_user_pattern(&dir, &mut p2).unwrap();

        // Write a non-JSON file — must NOT appear in the listing.
        std::fs::write(dir.join("notes.txt"), b"ignore me").unwrap();

        let listed = list_user_patterns(&dir);
        assert_eq!(listed.len(), 2, "must list exactly 2 .json files");
        assert!(listed.iter().any(|p| p == &path1), "path1 must be listed");
        assert!(listed.iter().any(|p| p == &path2), "path2 must be listed");
        assert!(
            listed
                .iter()
                .all(|p| p.extension().and_then(|e| e.to_str()) == Some("json")),
            "list must only contain .json files"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_user_patterns_empty_when_dir_absent() {
        // Use a path that definitely doesn't exist.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let absent = std::env::temp_dir().join(format!("midip-absent-{}", nanos));
        let listed = list_user_patterns(&absent);
        assert!(listed.is_empty(), "absent dir must return empty list");
    }

    #[test]
    fn load_user_pattern_repairs_bad_fields() {
        let dir = unique_dir("t4-pat-repair");

        // Craft a JSON with out-of-range fields:
        // length=2 but data has 4 steps; drum vel=200, prob=5.0, ratchet=99.
        let bad_json = r#"{
            "name": "bad pattern",
            "desc": "",
            "length": 2,
            "data": {"Drums": [
                [{"note": 200, "vel": 200, "prob": 5.0, "ratchet": 99}],
                [],
                [],
                []
            ]},
            "id": "0000000000000000"
        }"#;
        let path = dir.join("bad-pattern.json");
        std::fs::write(&path, bad_json).unwrap();

        let loaded = load_user_pattern(&path).unwrap();

        // length stays at 2 (already in range 1..=64)
        assert_eq!(loaded.length, 2);
        // data must be resized from 4 steps to 2
        if let PatternData::Drums(ref steps) = loaded.data {
            assert_eq!(steps.len(), 2, "data must be resized to match length=2");
            let hit = &steps[0][0];
            assert_eq!(hit.note, 127, "note clamped to 127");
            assert_eq!(hit.vel, 127, "vel clamped to 127");
            assert_eq!(hit.prob, 1.0, "prob clamped to 1.0");
            assert_eq!(hit.ratchet, 8, "ratchet clamped to 8");
        } else {
            panic!("expected Drums data");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_and_repair_pattern_clean_is_noop() {
        let mut p = make_drum_pattern("Clean");
        let original = p.clone();
        let notes = validate_and_repair_pattern(&mut p);
        assert!(
            notes.is_empty(),
            "clean pattern must return no repair notes"
        );
        assert_eq!(p, original, "clean pattern must be unchanged");
    }

    fn vendored_ref(role: &str) -> PatternRef {
        PatternRef::Vendored {
            role: role.to_string(),
            genre: "techno".to_string(),
            name: "Four on Floor".to_string(),
        }
    }

    #[test]
    fn favorites_toggle_add_remove() {
        let mut favs = Favorites::default();
        let r = vendored_ref("drums");
        let added = favs.toggle(r.clone());
        assert!(added, "first toggle should add");
        assert!(favs.contains(&r));
        let removed = favs.toggle(r.clone());
        assert!(!removed, "second toggle should remove");
        assert!(!favs.contains(&r));
    }

    #[test]
    fn favorites_dedup() {
        let mut favs = Favorites::default();
        let r = vendored_ref("drums");
        favs.toggle(r.clone());
        favs.toggle(r.clone());
        assert!(favs.refs.is_empty(), "toggling same ref twice nets empty");
        assert_eq!(favs.refs.iter().filter(|x| *x == &r).count(), 0);
    }

    #[test]
    fn favorites_store_roundtrip() {
        let dir = unique_dir("favorites-roundtrip");
        let mut favs = Favorites::default();
        favs.toggle(vendored_ref("drums"));
        favs.toggle(vendored_ref("bass"));
        save_favorites(&dir, &favs).unwrap();
        let (loaded, _) = load_favorites(&dir);
        assert_eq!(loaded.version, CURRENT_FAVORITES_VERSION);
        assert_eq!(loaded.refs.len(), 2);
        assert!(loaded.contains(&vendored_ref("drums")));
        assert!(loaded.contains(&vendored_ref("bass")));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Task 2: Crate model + store ──────────────────────────────────────────

    fn drum_entry() -> CrateEntry {
        CrateEntry {
            pattern: vendored_ref("drums"),
            label: None,
        }
    }

    fn bass_entry() -> CrateEntry {
        CrateEntry {
            pattern: vendored_ref("bass"),
            label: Some("my bass".to_string()),
        }
    }

    #[test]
    fn crate_index_roundtrip() {
        let dir = unique_dir("crate-roundtrip");
        let mut idx = CrateIndex::default();
        let ci = idx.add_crate("Set A".to_string());
        idx.add_entry(ci, drum_entry());
        idx.add_entry(ci, bass_entry());

        let id_before = idx.crates[ci].id.clone();

        save_crates(&dir, &idx).unwrap();
        let (loaded, _) = load_crates(&dir);

        assert_eq!(loaded.version, CURRENT_CRATES_VERSION);
        assert_eq!(loaded.crates.len(), 1);
        assert_eq!(loaded.crates[0].name, "Set A");
        assert_eq!(loaded.crates[0].id, id_before);
        assert_eq!(loaded.crates[0].entries.len(), 2);
        assert_eq!(loaded.crates[0].entries[0], drum_entry());
        assert_eq!(loaded.crates[0].entries[1], bass_entry());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_remove_rename_crate() {
        let mut idx = CrateIndex::default();
        let ci = idx.add_crate("Alpha".to_string());
        let id_before = idx.crates[ci].id.clone();

        // rename keeps id
        idx.rename_crate(ci, "Beta".to_string());
        assert_eq!(idx.crates[ci].name, "Beta");
        assert_eq!(idx.crates[ci].id, id_before, "rename must keep id stable");

        // add a second crate then remove first
        idx.add_crate("Gamma".to_string());
        assert_eq!(idx.crates.len(), 2);
        idx.remove_crate(0);
        assert_eq!(idx.crates.len(), 1);
        assert_eq!(idx.crates[0].name, "Gamma");

        // out-of-range remove is a no-op (must not panic)
        idx.remove_crate(99);
        assert_eq!(idx.crates.len(), 1);
    }

    #[test]
    fn duplicate_crate_fresh_id() {
        let mut idx = CrateIndex::default();
        let ci = idx.add_crate("Original".to_string());
        idx.add_entry(ci, drum_entry());
        let orig_id = idx.crates[ci].id.clone();

        let dup_ci = idx.duplicate_crate(ci).expect("duplicate must succeed");
        let dup = &idx.crates[dup_ci];

        assert_ne!(dup.id, orig_id, "duplicate must have a fresh id");
        assert_eq!(dup.name, "Original copy");
        assert_eq!(dup.entries.len(), 1, "entries must be copied");
        assert_eq!(dup.entries[0], drum_entry());

        // out-of-range duplicate returns None (must not panic)
        assert!(idx.duplicate_crate(99).is_none());
    }

    #[test]
    fn add_remove_reorder_entry() {
        let mut idx = CrateIndex::default();
        let ci = idx.add_crate("Crate".to_string());
        idx.add_entry(ci, drum_entry());
        idx.add_entry(ci, bass_entry());

        let synth_entry = CrateEntry {
            pattern: vendored_ref("synth"),
            label: None,
        };
        idx.add_entry(ci, synth_entry.clone());
        // entries: [drums, bass, synth]
        assert_eq!(idx.crates[ci].entries.len(), 3);

        // reorder: move index 2 (synth) to index 0
        idx.reorder_entry(ci, 2, 0);
        // entries: [synth, drums, bass]
        assert_eq!(idx.crates[ci].entries[0], synth_entry);
        assert_eq!(idx.crates[ci].entries[1], drum_entry());
        assert_eq!(idx.crates[ci].entries[2], bass_entry());

        // out-of-range reorder is a no-op (must not panic)
        idx.reorder_entry(ci, 99, 0);
        idx.reorder_entry(ci, 0, 99);
        idx.reorder_entry(99, 0, 1);
        assert_eq!(
            idx.crates[ci].entries.len(),
            3,
            "entries unchanged after bad reorder"
        );

        // remove entry at index 1 (drums)
        idx.remove_entry(ci, 1);
        assert_eq!(idx.crates[ci].entries.len(), 2);
        assert_eq!(idx.crates[ci].entries[0], synth_entry);
        assert_eq!(idx.crates[ci].entries[1], bass_entry());

        // out-of-range remove_entry is a no-op (must not panic)
        idx.remove_entry(ci, 99);
        idx.remove_entry(99, 0);
        assert_eq!(idx.crates[ci].entries.len(), 2);
    }

    #[test]
    fn same_pattern_in_two_crates() {
        let mut idx = CrateIndex::default();
        let ca = idx.add_crate("Crate A".to_string());
        let cb = idx.add_crate("Crate B".to_string());

        // The same PatternRef added to both crates — must be allowed
        idx.add_entry(ca, drum_entry());
        idx.add_entry(cb, drum_entry());

        assert_eq!(idx.crates[ca].entries[0].pattern, vendored_ref("drums"));
        assert_eq!(idx.crates[cb].entries[0].pattern, vendored_ref("drums"));
    }

    #[test]
    fn load_crates_missing_returns_default() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let absent = std::env::temp_dir().join(format!("midip-crate-absent-{}", nanos));
        // Directory does not exist — must return empty default
        let (loaded, note) = load_crates(&absent);
        assert_eq!(loaded.version, 0);
        assert!(loaded.crates.is_empty());
        assert!(note.is_none(), "missing file is a fresh install, no note");
    }

    #[test]
    fn load_favorites_corrupt_is_quarantined_not_destroyed() {
        // M7: same quarantine contract as prefs — user data survives as .bak.
        let dir = unique_dir("favorites-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = favorites_path(&dir);
        std::fs::write(&path, b"[[[").unwrap();
        let (loaded, note) = load_favorites(&dir);
        assert!(
            loaded.refs.is_empty(),
            "corrupt favorites fall back to default"
        );
        assert!(note.is_some());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(path.with_extension("json.bak")).unwrap(),
            b"[[["
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── M5a Task 2: per-lane scale + root persistence ────────────────────────

    #[test]
    fn lane_scale_root_serde_roundtrips() {
        use crate::music::scale::Scale;
        let dir = unique_dir("m5a-scale-roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "scale test".to_string();
        // Set a non-chromatic scale and explicit root on lane 1 (melodic).
        set.lanes[1].scale = Scale::Major;
        set.lanes[1].root = Some(50);

        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();

        assert_eq!(
            loaded.lanes[1].scale,
            Scale::Major,
            "scale must survive save/load"
        );
        assert_eq!(
            loaded.lanes[1].root,
            Some(50),
            "root override must survive save/load"
        );
        // Lanes without overrides stay at defaults.
        assert_eq!(loaded.lanes[0].scale, Scale::Chromatic);
        assert_eq!(loaded.lanes[0].root, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── M6 Task 1: Scene persistence ────────────────────────────────────────

    #[test]
    fn set_scenes_default_empty_on_old_json() {
        // A set JSON without a `scenes` field must deserialize with scenes == [].
        let profile_id = default_profiles()[0].id;
        let old_json = format!(
            r#"{{
                "version": 1,
                "id": "abcdef1234567890",
                "name": "old-no-scenes",
                "bpm": 120.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{profile_id}",
                    "pattern": {{
                        "name": "beat",
                        "desc": "",
                        "length": 1,
                        "data": {{"Drums": [[]]}}
                    }},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#
        );
        let dir = unique_dir("m6-old-no-scenes");
        let path = dir.join("old.json");
        std::fs::write(&path, &old_json).unwrap();
        let loaded = load_set(&path).unwrap();
        assert!(
            loaded.scenes.is_empty(),
            "old set JSON without scenes field must load with scenes == []"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scene_roundtrips_through_store() {
        use crate::pattern::model::{LaneAssignment, Scene};
        use crate::pattern::refs::PatternRef;
        let dir = unique_dir("m6-scene-roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "Scenes Test".to_string();
        // Ensure lane patterns have non-nil ids.
        for lane in &mut set.lanes {
            lane.pattern.ensure_id();
        }
        // Add a scene.
        let scene_id = persist::mint_id();
        set.scenes = vec![Scene {
            id: scene_id.clone(),
            name: "Scene 1".to_string(),
            assignments: vec![
                LaneAssignment {
                    pattern: PatternRef::User(set.lanes[0].pattern.id.clone()),
                    mute: true,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                },
                LaneAssignment {
                    pattern: PatternRef::User(set.lanes[1].pattern.id.clone()),
                    mute: false,
                    solo: false,
                    transpose: 3,
                    octave: 1,
                },
                LaneAssignment {
                    pattern: PatternRef::User(set.lanes[2].pattern.id.clone()),
                    mute: false,
                    solo: true,
                    transpose: 0,
                    octave: 0,
                },
            ],
        }];

        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();

        assert_eq!(loaded.scenes.len(), 1, "scene must survive save/load");
        let s = &loaded.scenes[0];
        assert_eq!(s.id, scene_id, "scene id must be stable");
        assert_eq!(s.name, "Scene 1");
        assert_eq!(s.assignments.len(), 3);
        assert!(s.assignments[0].mute);
        assert_eq!(s.assignments[1].transpose, 3);
        assert_eq!(s.assignments[1].octave, 1);
        assert!(s.assignments[2].solo);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_repairs_malformed_scene() {
        use crate::pattern::model::{LaneAssignment, Scene};
        use crate::pattern::refs::PatternRef;
        let mut set = Set::default_set(default_profiles());
        for lane in &mut set.lanes {
            lane.pattern.ensure_id();
        }
        // A scene with out-of-range transpose and octave.
        set.scenes = vec![Scene {
            id: persist::mint_id(),
            name: "Bad Scene".to_string(),
            assignments: vec![LaneAssignment {
                pattern: PatternRef::User(set.lanes[0].pattern.id.clone()),
                mute: false,
                solo: false,
                transpose: 127, // out of range
                octave: -99,    // out of range
            }],
        }];
        let notes = validate_and_repair(&mut set);
        let a = &set.scenes[0].assignments[0];
        assert!(
            a.transpose >= -24 && a.transpose <= 24,
            "transpose must be clamped, got {}",
            a.transpose
        );
        assert!(
            a.octave >= -4 && a.octave <= 4,
            "octave must be clamped, got {}",
            a.octave
        );
        assert!(!notes.is_empty(), "repair must note that scene was fixed");
    }

    /// A scene with too many assignments is truncated to `lanes.len()`;
    /// a scene with too few is padded to `lanes.len()`.
    /// Neither must panic, and the resulting count must equal lane count.
    #[test]
    fn validate_reconciles_scene_assignment_count_to_lane_count() {
        use crate::pattern::model::{LaneAssignment, Scene};
        use crate::pattern::refs::PatternRef;

        let mut set = Set::default_set(default_profiles());
        // Give all lane patterns non-nil ids so padding can reference them.
        for lane in &mut set.lanes {
            lane.pattern.ensure_id();
        }
        let lane_count = set.lanes.len(); // 3

        // Scene A: too many assignments (5 > 3).
        let too_many = Scene {
            id: persist::mint_id(),
            name: "Too Many".to_string(),
            assignments: (0..5)
                .map(|_| LaneAssignment {
                    pattern: PatternRef::User(persist::mint_id()),
                    mute: false,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                })
                .collect(),
        };

        // Scene B: too few assignments (1 < 3).
        let too_few = Scene {
            id: persist::mint_id(),
            name: "Too Few".to_string(),
            assignments: vec![LaneAssignment {
                pattern: PatternRef::User(set.lanes[0].pattern.id.clone()),
                mute: true,
                solo: false,
                transpose: 2,
                octave: 0,
            }],
        };

        set.scenes = vec![too_many, too_few];
        let notes = validate_and_repair(&mut set);

        // Both scenes must now have exactly lane_count assignments — no panic.
        assert_eq!(
            set.scenes[0].assignments.len(),
            lane_count,
            "too-many scene must be truncated to lane count"
        );
        assert_eq!(
            set.scenes[1].assignments.len(),
            lane_count,
            "too-few scene must be padded to lane count"
        );

        // The padded assignments must have neutral values.
        let padded = &set.scenes[1].assignments;
        // First assignment was already there — preserved.
        assert!(padded[0].mute, "original assignment[0] must be preserved");
        assert_eq!(padded[0].transpose, 2);
        // Padded slots must be neutral.
        for a in &padded[1..] {
            assert!(!a.mute, "padded assignment must have mute=false");
            assert!(!a.solo, "padded assignment must have solo=false");
            assert_eq!(a.transpose, 0, "padded assignment must have transpose=0");
            assert_eq!(a.octave, 0, "padded assignment must have octave=0");
        }

        // Repair notes must mention both scenes.
        assert!(
            notes.iter().any(|n| n.contains("truncated")),
            "repair notes must mention truncation"
        );
        assert!(
            notes.iter().any(|n| n.contains("padded")),
            "repair notes must mention padding"
        );
    }

    #[test]
    fn old_lane_json_without_scale_loads_chromatic() {
        use crate::music::scale::Scale;
        let dir = unique_dir("m5a-old-no-scale");
        let profile_id = default_profiles()[1].id; // melodic lane profile
                                                   // Old-format JSON: no `scale` or `root` keys in the lane object.
        let old_json = format!(
            r#"{{
                "name": "legacy scale test",
                "bpm": 120.0,
                "swing": 0.5,
                "lanes": [{{
                    "profile_id": "{profile_id}",
                    "pattern": {{
                        "name": "mel",
                        "desc": "",
                        "length": 1,
                        "data": {{"Melodic": [null]}}
                    }},
                    "mute": false,
                    "solo": false,
                    "transpose": 0,
                    "octave": 0
                }}]
            }}"#
        );
        let path = dir.join("legacy-scale.json");
        std::fs::write(&path, &old_json).unwrap();

        let loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.lanes[0].scale,
            Scale::Chromatic,
            "old JSON without scale must load as Chromatic"
        );
        assert_eq!(
            loaded.lanes[0].root, None,
            "old JSON without root must load as None"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── M7 Task 2: chain persistence ─────────────────────────────────────────

    /// Minimal v1 Set JSON (no `chains` key) — simulates a file saved before M7.
    const MINIMAL_V1_SET_JSON: &str = r#"{
        "version": 1,
        "id": "abcdef0123456789",
        "name": "pre-m7",
        "bpm": 120.0,
        "swing": 0.5,
        "lanes": []
    }"#;

    #[test]
    fn v1_set_without_chains_loads_with_empty_chains() {
        let mut v: serde_json::Value = serde_json::from_str(MINIMAL_V1_SET_JSON).unwrap();
        migrate_set_value(&mut v).unwrap();
        assert_eq!(v["version"], 4, "v1 must migrate to version 4");
        // Deserialize into SetDto — missing `chains` key must default to [].
        let dto: SetDto = serde_json::from_value(v).unwrap();
        assert!(
            dto.chains.is_empty(),
            "missing chains field must default to empty"
        );
    }

    #[test]
    fn set_with_chains_roundtrips() {
        use crate::pattern::model::{Chain, ChainEntry};
        let dir = unique_dir("chain-roundtrip");
        let mut set = Set::default_set(default_profiles());
        let mut c = Chain::new("A->B");
        c.entries.push(ChainEntry {
            scene_id: persist::mint_id(),
            repeats: 2,
            bars: 4,
        });
        set.chains.push(c);
        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();
        assert_eq!(loaded.chains.len(), 1, "chain must survive save/load");
        assert_eq!(loaded.chains[0].entries[0].bars, 4);
        assert_eq!(loaded.chains[0].entries[0].repeats, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── M8 Task 2: v2→v3 migration (per-step/per-lane fields) ────────────────

    /// Minimal v2 Set JSON (no M8 fields) — simulates a file saved before M8.
    const MINIMAL_V2_SET_JSON: &str = r#"{
        "version": 2,
        "id": "abcdef0123456789",
        "name": "pre-m8",
        "bpm": 120.0,
        "swing": 0.5,
        "lanes": []
    }"#;

    #[test]
    fn v2_set_without_m8_fields_loads_with_defaults() {
        let mut v: serde_json::Value = serde_json::from_str(MINIMAL_V2_SET_JSON).unwrap();
        migrate_set_value(&mut v).unwrap();
        assert_eq!(v["version"], 4, "v2 must migrate to version 4");
        // Deserialize into SetDto — missing M8 keys must default cleanly.
        let dto: SetDto = serde_json::from_value(v).unwrap();
        assert!(
            dto.chains.is_empty(),
            "missing chains field must default to empty"
        );
        assert!(
            dto.lanes.is_empty(),
            "empty lanes must stay empty after migration"
        );
    }

    #[test]
    fn set_with_m8_fields_roundtrips() {
        use crate::pattern::model::{CcLock, DrumHit, MelodicNote, PatternData, TrigCond};
        let dir = unique_dir("m8-roundtrip");
        let mut set = Set::default_set(default_profiles());
        // Set per-lane M8 fields on lane 0.
        set.lanes[0].swing = Some(0.55);
        set.lanes[0].clock_div = Some(2);
        // Set per-step M8 fields: a drum hit with micro+cond.
        if let PatternData::Drums(ref mut steps) = set.lanes[0].pattern.data {
            steps[0] = vec![DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: -50,
                cond: TrigCond::Fill,
            }];
        }
        // Set per-step M8 fields: a melodic note with micro+cond.
        if let PatternData::Melodic(ref mut steps) = set.lanes[1].pattern.data {
            steps[0] = crate::pattern::model::MelodicStep::from(vec![MelodicNote {
                semi: 4,
                vel: 1.0,
                slide: false,
                len: 0.5,
                prob: 1.0,
                ratchet: 1,
                micro: 30,
                cond: TrigCond::Always,
            }]);
        }
        // Set per-pattern M8 cc field.
        set.lanes[0].pattern.cc = vec![vec![CcLock { cc: 74, val: 64 }]];

        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();

        assert_eq!(
            loaded.lanes[0].swing,
            Some(0.55),
            "lane swing must survive save/load"
        );
        assert_eq!(
            loaded.lanes[0].clock_div,
            Some(2),
            "lane clock_div must survive save/load"
        );
        if let PatternData::Drums(ref steps) = loaded.lanes[0].pattern.data {
            assert_eq!(steps[0][0].micro, -50, "drum micro must survive save/load");
            assert_eq!(
                steps[0][0].cond,
                TrigCond::Fill,
                "drum cond must survive save/load"
            );
        } else {
            panic!("expected Drums on lane 0");
        }
        if let PatternData::Melodic(ref steps) = loaded.lanes[1].pattern.data {
            assert_eq!(
                steps[0][0].micro, 30,
                "melodic micro must survive save/load"
            );
        } else {
            panic!("expected Melodic on lane 1");
        }
        assert_eq!(
            loaded.lanes[0].pattern.cc,
            vec![vec![CcLock { cc: 74, val: 64 }]],
            "cc must survive save/load"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_version_is_3() {
        // Version was 3; kept for history. The live assertion is set_version_is_4.
        // This test is intentionally removed — see set_version_is_4.
    }

    // ── M10 Task 1: v3→v4 migration + clock_in_port persistence ─────────────

    /// Minimal v3 Set JSON (no `clock_in_port`) — simulates a file saved before M10.
    const MINIMAL_V3_SET_JSON: &str = r#"{
        "version": 3,
        "id": "abcdef0123456789",
        "name": "pre-m10",
        "bpm": 120.0,
        "swing": 0.5,
        "lanes": []
    }"#;

    #[test]
    fn set_version_is_4() {
        assert_eq!(CURRENT_SET_VERSION, 4);
    }

    #[test]
    fn v3_set_without_clock_in_port_loads_with_none() {
        let mut v: serde_json::Value = serde_json::from_str(MINIMAL_V3_SET_JSON).unwrap();
        migrate_set_value(&mut v).unwrap();
        assert_eq!(v["version"], 4, "v3 must migrate to version 4");
        let dto: SetDto = serde_json::from_value(v).unwrap();
        assert!(
            dto.clock_in_port.is_none(),
            "missing clock_in_port must default to None"
        );
    }

    #[test]
    fn set_with_clock_in_port_roundtrips() {
        use crate::pattern::model::PortRef;
        let dir = unique_dir("clock-in-roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.clock_in_port = Some(PortRef {
            stable_key: "hw:1,0".to_string(),
            name: "Keystep".to_string(),
        });
        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.clock_in_port,
            Some(PortRef {
                stable_key: "hw:1,0".to_string(),
                name: "Keystep".to_string(),
            }),
            "clock_in_port must survive save/load"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_without_clock_in_port_roundtrips_as_none() {
        let dir = unique_dir("clock-in-none-roundtrip");
        let mut set = Set::default_set(default_profiles());
        assert!(set.clock_in_port.is_none());
        let path = save_set(&dir, &mut set).unwrap();
        let loaded = load_set(&path).unwrap();
        assert!(
            loaded.clock_in_port.is_none(),
            "absent clock_in_port must load as None"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
