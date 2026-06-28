use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};

use crate::devices::profiles::profile_by_id;
use crate::pattern::model::{Lane, Pattern, Set};

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
pub fn load_set(path: &Path) -> anyhow::Result<Set> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let dto: SetDto = serde_json::from_str(&json).context("deserializing set")?;
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
    })
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
    use crate::pattern::model::{MelodicNote, PatternData, Set};

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

    #[test]
    fn save_then_load_round_trips_a_set() {
        let dir = unique_dir("roundtrip");
        let mut set = Set::default_set(default_profiles());
        set.name = "My Jam".to_string();
        set.bpm = 124.0;
        set.swing = 0.56;
        // Make lane 1 (melodic) non-trivial so we exercise note serialization.
        if let PatternData::Melodic(steps) = &mut set.lanes[1].pattern.data {
            steps[0] = Some(MelodicNote { semi: 7, vel: 1.3, slide: true, len: 0.5, prob: 1.0, ratchet: 1 });
        }
        set.lanes[0].mute = true;
        set.lanes[2].transpose = 3;

        let path = save_set(&dir, &set).unwrap();
        assert!(path.exists());

        let loaded = load_set(&path).unwrap();
        assert_eq!(loaded, set);

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
        assert!(listed.iter().all(|p| p.extension().and_then(|e| e.to_str()) == Some("json")));

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
