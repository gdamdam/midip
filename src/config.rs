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

/// Runtime data dir (saved sets / user patterns).
///
/// Resolution order:
/// 1. `$MIDIP_DATA` env var if set.
/// 2. `<exe-dir>/data` if the exe dir is resolvable.
/// 3. Dev fallback: `CARGO_MANIFEST_DIR/data`.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MIDIP_DATA") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = exe_dir() {
        return dir.join("data");
    }
    project_root().join("data")
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

    /// Env-var override tests — set, assert, unset in ONE test to avoid
    /// cross-test env races when cargo runs tests in parallel.
    #[test]
    fn env_overrides_are_respected() {
        // --- MIDIP_DATA override ---
        // SAFETY: tests run in the same process; we restore immediately after.
        unsafe { std::env::set_var("MIDIP_DATA", "/tmp/my_data") };
        let d = data_dir();
        unsafe { std::env::remove_var("MIDIP_DATA") };
        assert_eq!(d, PathBuf::from("/tmp/my_data"));

        // --- MIDIP_ASSETS override ---
        unsafe { std::env::set_var("MIDIP_ASSETS", "/tmp/my_assets") };
        let p = patterns_dir();
        unsafe { std::env::remove_var("MIDIP_ASSETS") };
        assert_eq!(p, PathBuf::from("/tmp/my_assets"));
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
