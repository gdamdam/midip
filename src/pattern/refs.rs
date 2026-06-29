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
/// 1. Search `inline_patterns` by id (patterns already held in memory, e.g. from `Set::lanes`).
/// 2. For `PatternRef::Vendored`: look up in `lib`.
/// 3. For `PatternRef::User`: the assignment is missing.
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
            PatternRef::Vendored { role, genre, name } => {
                // Check inline first (covers user-overridden vendored patterns in the set),
                // then fall back to the vendored library.
                inline_patterns
                    .iter()
                    .find(|p| &p.name == name)
                    .cloned()
                    .or_else(|| lib.find(role, genre, name).cloned())
                    .ok_or(())
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
