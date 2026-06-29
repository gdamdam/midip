use std::path::Path;

use crate::pattern::library::Library;
use crate::pattern::model::{Pattern, Scene};
use crate::pattern::store::{list_user_patterns, load_user_pattern};

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PatternRef {
    User(crate::persist::Id),
    Vendored {
        role: String,
        genre: String,
        name: String,
    },
}

impl PatternRef {
    /// For Vendored: returns name. For User: returns first 8 chars of id hex.
    pub fn display_name(&self) -> String {
        match self {
            PatternRef::Vendored { name, .. } => name.clone(),
            PatternRef::User(id) => id.as_str()[..8.min(id.as_str().len())].to_string(),
        }
    }

    /// For Vendored: drums→0, bass→1, synth→2. For User: None.
    pub fn role_lane_hint(&self) -> Option<usize> {
        match self {
            PatternRef::Vendored { role, .. } => match role.as_str() {
                "drums" => Some(0),
                "bass" => Some(1),
                "synth" => Some(2),
                _ => None,
            },
            PatternRef::User(_) => None,
        }
    }
}

/// Resolve a PatternRef to a Pattern.
/// - Vendored: looks up in lib.find(role, genre, name).cloned()
/// - User: scans list_user_patterns(user_dir), loads each, returns the one whose id matches
///
/// Returns None if unresolvable.
pub fn resolve_pattern_ref(r: &PatternRef, lib: &Library, user_dir: &Path) -> Option<Pattern> {
    match r {
        PatternRef::Vendored { role, genre, name } => lib.find(role, genre, name).cloned(),
        PatternRef::User(id) => {
            for path in list_user_patterns(user_dir) {
                if let Ok(pat) = load_user_pattern(&path) {
                    if &pat.id == id {
                        return Some(pat);
                    }
                }
            }
            None
        }
    }
}

/// Resolve each assignment in `scene` to a `Pattern`.
///
/// For each assignment, the resolution order is:
/// 1. For `PatternRef::User`: search `inline_patterns` by id (patterns already held in memory,
///    e.g. from `Set::lanes`). `Err(())` if not found.
/// 2. For `PatternRef::Vendored`: resolve via the library using all three fields
///    (role + genre + name), which is the canonical match used by `resolve_pattern_ref`.
///    Inline patterns are NOT searched for vendored refs — a name-only match would
///    silently resolve to the wrong pattern when two vendored entries share a name but
///    differ in role or genre.
///
/// Returns one `Result<Pattern, ()>` per assignment in order. `Err(())` means the
/// pattern could not be resolved; the caller (Task 2/3) should warn and skip that lane.
/// This function is pure — it never mutates the set or launches anything.
pub fn resolve_scene(
    scene: &Scene,
    lib: &Library,
    inline_patterns: &[Pattern],
) -> Vec<Result<Pattern, ()>> {
    scene
        .assignments
        .iter()
        .map(|a| match &a.pattern {
            // Vendored refs are resolved exclusively by role+genre+name via the library,
            // matching the canonical logic in `resolve_pattern_ref`. A name-only inline
            // search is omitted: two vendored entries can share a name while differing in
            // role or genre, which would cause silent mis-resolution.
            PatternRef::Vendored { role, genre, name } => {
                lib.find(role, genre, name).cloned().ok_or(())
            }
            PatternRef::User(id) => inline_patterns
                .iter()
                .find(|p| &p.id == id)
                .cloned()
                .ok_or(()),
        })
        .collect()
}

/// Resolve each assignment in `scene` using a file-backed user-pattern directory.
///
/// Same semantics as `resolve_scene` but searches `user_dir` on disk instead of
/// an in-memory slice. Suitable for contexts where the set is not fully loaded.
pub fn resolve_scene_from_dir(
    scene: &Scene,
    lib: &Library,
    user_dir: &Path,
) -> Vec<Result<Pattern, ()>> {
    scene
        .assignments
        .iter()
        .map(|a| resolve_pattern_ref(&a.pattern, lib, user_dir).ok_or(()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist;

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("midip-refs-{}-{}", tag, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pattern_ref_serde_roundtrip() {
        let user_ref = PatternRef::User(persist::mint_id());
        let vendored_ref = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "Four on Floor".to_string(),
        };

        let user_json = serde_json::to_string(&user_ref).unwrap();
        let vendored_json = serde_json::to_string(&vendored_ref).unwrap();

        let user_back: PatternRef = serde_json::from_str(&user_json).unwrap();
        let vendored_back: PatternRef = serde_json::from_str(&vendored_json).unwrap();

        assert_eq!(user_ref, user_back);
        assert_eq!(vendored_ref, vendored_back);
    }

    #[test]
    fn pattern_ref_role_lane_hint() {
        let drums = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "x".to_string(),
        };
        let bass = PatternRef::Vendored {
            role: "bass".to_string(),
            genre: "techno".to_string(),
            name: "x".to_string(),
        };
        let synth = PatternRef::Vendored {
            role: "synth".to_string(),
            genre: "techno".to_string(),
            name: "x".to_string(),
        };
        let user = PatternRef::User(persist::mint_id());

        assert_eq!(drums.role_lane_hint(), Some(0));
        assert_eq!(bass.role_lane_hint(), Some(1));
        assert_eq!(synth.role_lane_hint(), Some(2));
        assert_eq!(user.role_lane_hint(), None);
    }

    /// Two vendored patterns share the same name but have different roles.
    /// `resolve_scene` must return the one whose role+genre+name all match,
    /// not the first one found by name alone.
    #[test]
    fn resolve_scene_vendored_disambiguates_by_role_and_genre() {
        use crate::pattern::library::Library;
        use crate::pattern::model::{LaneAssignment, Pattern, Scene};

        // Build a minimal library with two vendored patterns sharing the name "Shared".
        // We use the real library path so Library::load gives us a baseline, then
        // we directly test the logic via a scene with real lib entries.
        // Since we can't easily inject a custom Library, we test the *observable behavior*:
        // resolve_scene for a Vendored ref with role="drums" / genre="techno" must NOT
        // return a pattern that has role="bass" when both names happen to be identical.
        //
        // Strategy: create a scene with a Vendored ref for a role that exists in the
        // real library, and a different Vendored ref with the same name but wrong role.
        // The wrong-role ref must resolve to Err (not found), proving name alone is not used.

        let lib_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = Library::load(&lib_path).expect("library loads");

        // Find any pattern in the bass library to get a real name.
        let bass_patterns = lib.bass.values().next().expect("bass genre present");
        let real_bass_pat = bass_patterns.first().expect("bass pattern present");
        let real_name = real_bass_pat.name.clone();
        let real_genre = lib.bass.keys().next().unwrap().clone();

        // A Vendored ref matching the real bass pattern (role=bass).
        let correct_ref = PatternRef::Vendored {
            role: "bass".to_string(),
            genre: real_genre.clone(),
            name: real_name.clone(),
        };
        // A Vendored ref using the SAME name but wrong role — must NOT resolve.
        let wrong_role_ref = PatternRef::Vendored {
            role: "drums".to_string(), // wrong role
            genre: real_genre.clone(),
            name: real_name.clone(),
        };

        let scene_correct = Scene {
            id: crate::persist::mint_id(),
            name: "correct".to_string(),
            assignments: vec![LaneAssignment {
                pattern: correct_ref,
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
            }],
        };
        let scene_wrong = Scene {
            id: crate::persist::mint_id(),
            name: "wrong".to_string(),
            assignments: vec![LaneAssignment {
                pattern: wrong_role_ref,
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
            }],
        };

        // Neither scene has inline patterns — pure library resolution.
        let inline: Vec<Pattern> = vec![];

        let correct_results = resolve_scene(&scene_correct, &lib, &inline);
        assert_eq!(correct_results.len(), 1);
        assert!(
            correct_results[0].is_ok(),
            "correct role+genre+name must resolve to Ok"
        );
        assert_eq!(
            correct_results[0].as_ref().unwrap().name,
            real_name,
            "resolved pattern name must match"
        );

        let wrong_results = resolve_scene(&scene_wrong, &lib, &inline);
        assert_eq!(wrong_results.len(), 1);
        assert!(
            wrong_results[0].is_err(),
            "wrong role with same name must resolve to Err (not mis-resolved by name alone)"
        );
    }

    #[test]
    fn resolve_user_ref_roundtrip() {
        let dir = unique_dir("resolve-user");
        let mut pat = crate::pattern::model::Pattern::empty_drums(16);
        pat.name = "test-resolve".to_string();
        crate::pattern::store::save_user_pattern(&dir, &mut pat).unwrap();

        let r = PatternRef::User(pat.id.clone());
        let lib = Library::empty();
        let resolved = resolve_pattern_ref(&r, &lib, &dir);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().name, "test-resolve");
    }
}
