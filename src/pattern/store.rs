use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};

use crate::devices::profiles::profile_by_id;
use crate::pattern::model::{Lane, Pattern, Set};
use crate::persist;

/// The current on-disk schema version. Increment when the format changes.
pub const CURRENT_SET_VERSION: u32 = 1;

/// On-disk lane: stores the profile *id* (not the static profile), rehydrated on load.
#[derive(Serialize, Deserialize)]
struct LaneDto {
    profile_id: String,
    pattern: Pattern,
    mute: bool,
    solo: bool,
    transpose: i8,
    octave: i8,
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

/// Run the migration ladder on a `serde_json::Value` before typed parse.
/// Rejects files saved by a newer midip; upgrades older files in-place.
pub fn migrate_set_value(v: &mut serde_json::Value) -> anyhow::Result<()> {
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
            _ => break,
        }
        cur += 1;
    }
    Ok(())
}

/// Serialize `set` to `<dir>/<slug>.json`. Returns the written path.
pub fn save_set(dir: &Path, set: &Set) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).context("creating set store dir")?;
    let path = dir.join(format!("{}.json", slug(&set.name)));
    let dto = SetDto::from(set);
    let json = serde_json::to_string_pretty(&dto).context("serializing set")?;
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Load a set from a JSON file, rehydrating each lane's static profile via its id.
/// Runs the migration ladder before typed parse so old files (no version, no id) load correctly.
pub fn load_set(path: &Path) -> anyhow::Result<Set> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&json).context("parsing set JSON")?;
    migrate_set_value(&mut value).context("migrating set")?;
    let dto: SetDto = serde_json::from_value(value).context("deserializing set")?;
    let mut lanes = Vec::with_capacity(dto.lanes.len());
    for l in dto.lanes {
        let profile = profile_by_id(&l.profile_id)
            .ok_or_else(|| anyhow!("unknown profile id: {}", l.profile_id))?;
        lanes.push(Lane {
            profile,
            pattern: l.pattern,
            mute: l.mute,
            solo: l.solo,
            transpose: l.transpose,
            octave: l.octave,
        });
    }
    Ok(Set {
        name: dto.name,
        bpm: dto.bpm,
        swing: dto.swing,
        lanes,
        id: dto.id,
    })
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

    // ── per-lane ──────────────────────────────────────────────────────────────
    for (lane_idx, lane) in set.lanes.iter_mut().enumerate() {
        let pat = &mut lane.pattern;
        let lane_num = lane_idx + 1;

        // length
        let orig_len = pat.length;
        pat.length = pat.length.clamp(1, 64);
        if pat.length != orig_len {
            notes.push(format!(
                "lane {} length {}→{}",
                lane_num, orig_len, pat.length
            ));
        }
        let target = pat.length;

        // data resize + field clamping
        match &mut pat.data {
            crate::pattern::model::PatternData::Drums(steps) => {
                if steps.len() != target {
                    steps.resize_with(target, Vec::new);
                    notes.push(format!("lane {} data resized to {}", lane_num, target));
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
                    notes.push(format!("lane {} drum hit fields clamped", lane_num));
                }
            }
            crate::pattern::model::PatternData::Melodic(steps) => {
                if steps.len() != target {
                    steps.resize_with(target, || None);
                    notes.push(format!("lane {} data resized to {}", lane_num, target));
                }
                let mut note_repaired = false;
                for step in steps.iter_mut().flatten() {
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
                    notes.push(format!("lane {} melodic note fields clamped", lane_num));
                }
            }
        }
    }

    notes
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
    fn validate_clean_set_is_unchanged_and_returns_no_notes() {
        let mut set = Set::default_set(default_profiles());
        let original = set.clone();
        let notes = validate_and_repair(&mut set);
        assert!(notes.is_empty(), "clean set must return no repair notes");
        assert_eq!(set, original, "clean set must be unchanged");
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
        assert_eq!(v["version"], 1);
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
    fn already_v1_file_passes_through_unchanged() {
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
        assert_eq!(v["version"], 1);
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
            steps[0] = Some(MelodicNote {
                semi: 7,
                vel: 1.3,
                slide: true,
                len: 0.5,
                prob: 1.0,
                ratchet: 1,
            });
        }
        set.lanes[0].mute = true;
        set.lanes[2].transpose = 3;

        let path = save_set(&dir, &set).unwrap();
        assert!(path.exists());

        let loaded = load_set(&path).unwrap();
        // Ids are nil (not yet minted by ensure_id), so they survive the round-trip as nil.
        assert_eq!(loaded.id, set.id);
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
        let path = save_set(&dir, &set).unwrap();

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
        let path = save_set(&dir, &set).unwrap();
        let fname = path.file_name().unwrap().to_str().unwrap();
        // lowercased, non-alphanumeric collapsed to '-'
        assert_eq!(fname, "acid-jam-3.json");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_set_errors_on_unknown_profile_id() {
        let dir = unique_dir("unknown-profile");
        let set = Set::default_set(default_profiles());
        let path = save_set(&dir, &set).unwrap();

        // Mutate the saved JSON to introduce a bogus profile_id
        let json_str = std::fs::read_to_string(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        json["lanes"][0]["profile_id"] = serde_json::json!("nonexistent-id");
        std::fs::write(&path, json.to_string()).unwrap();

        // Loading should fail with an unknown profile id error
        assert!(load_set(&path).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
