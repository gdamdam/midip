//! Paths and defaults.

use std::path::PathBuf;

/// Default tempo for a fresh set.
pub const DEFAULT_BPM: f64 = 120.0;

/// Resolve the directory containing the running executable.
/// Returns `None` if `current_exe()` fails or the path has no parent.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// Dev fallback: project root baked in at compile time.
fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Pure helper: resolve a data directory from explicit inputs.
/// Preferred over the env-reading `data_dir()` in tests.
///
/// Resolution order: env_val → exe_relative → dev_fallback.
pub fn resolve_data_dir(
    env_val: Option<String>,
    exe_relative: Option<PathBuf>,
    dev_fallback: PathBuf,
) -> PathBuf {
    if let Some(val) = env_val {
        return PathBuf::from(val);
    }
    if let Some(dir) = exe_relative {
        return dir;
    }
    dev_fallback
}

/// Pure helper: resolve a patterns directory from explicit inputs.
/// Same resolution order as `resolve_data_dir`.
pub fn resolve_patterns_dir(
    env_val: Option<String>,
    exe_relative: Option<PathBuf>,
    dev_fallback: PathBuf,
) -> PathBuf {
    if let Some(val) = env_val {
        return PathBuf::from(val);
    }
    if let Some(dir) = exe_relative {
        return dir;
    }
    dev_fallback
}

/// Runtime data dir (saved sets / user patterns).
///
/// Resolution order:
/// 1. `$MIDIP_DATA` env var if set.
/// 2. `<exe-dir>/data` if the exe dir is resolvable.
/// 3. Dev fallback: `CARGO_MANIFEST_DIR/data`.
pub fn data_dir() -> PathBuf {
    resolve_data_dir(
        std::env::var("MIDIP_DATA").ok(),
        exe_dir().map(|d| d.join("data")),
        project_root().join("data"),
    )
}

/// Vendored read-only pattern library dir.
///
/// Resolution order:
/// 1. `$MIDIP_ASSETS` env var if set.
/// 2. `<exe-dir>/assets/patterns` if that path exists.
/// 3. Dev fallback: `CARGO_MANIFEST_DIR/assets/patterns`.
pub fn patterns_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MIDIP_ASSETS") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = exe_dir() {
        let candidate = dir.join("assets").join("patterns");
        if candidate.exists() {
            return candidate;
        }
    }
    project_root().join("assets").join("patterns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bpm_is_120() {
        assert_eq!(DEFAULT_BPM, 120.0);
    }

    // --- pure resolve_data_dir tests (no env mutation) ---

    #[test]
    fn resolve_data_dir_prefers_env_val() {
        let result = resolve_data_dir(
            Some("/my/data".into()),
            Some(PathBuf::from("/exe/data")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(result, PathBuf::from("/my/data"));
    }

    #[test]
    fn resolve_data_dir_uses_exe_relative_when_no_env() {
        let result = resolve_data_dir(
            None,
            Some(PathBuf::from("/exe/data")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(result, PathBuf::from("/exe/data"));
    }

    #[test]
    fn resolve_data_dir_uses_dev_fallback_last() {
        let result = resolve_data_dir(None, None, PathBuf::from("/fallback"));
        assert_eq!(result, PathBuf::from("/fallback"));
    }

    // --- pure resolve_patterns_dir tests (no env mutation) ---

    #[test]
    fn resolve_patterns_dir_prefers_env_val() {
        let result = resolve_patterns_dir(
            Some("/my/assets".into()),
            Some(PathBuf::from("/exe/assets/patterns")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(result, PathBuf::from("/my/assets"));
    }

    #[test]
    fn resolve_patterns_dir_uses_exe_relative_when_no_env() {
        let result = resolve_patterns_dir(
            None,
            Some(PathBuf::from("/exe/assets/patterns")),
            PathBuf::from("/fallback"),
        );
        assert_eq!(result, PathBuf::from("/exe/assets/patterns"));
    }

    #[test]
    fn resolve_patterns_dir_uses_dev_fallback_last() {
        let result = resolve_patterns_dir(None, None, PathBuf::from("/fallback"));
        assert_eq!(result, PathBuf::from("/fallback"));
    }

    /// Without env overrides the returned paths must end with the expected
    /// suffixes and must not panic.
    #[test]
    fn paths_end_with_expected_suffixes() {
        // Ensure neither env var is set for this test.
        unsafe { std::env::remove_var("MIDIP_DATA") };
        unsafe { std::env::remove_var("MIDIP_ASSETS") };

        let d = data_dir();
        let ds = d.to_string_lossy();
        assert!(
            ds.ends_with("data") || ds.ends_with("data/"),
            "data_dir was {ds}"
        );

        let p = patterns_dir();
        let ps = p.to_string_lossy();
        assert!(
            ps.ends_with("assets/patterns")
                || ps.ends_with("assets\\patterns")
                || ps.ends_with("assets/patterns/"),
            "patterns_dir was {ps}"
        );
    }
}
