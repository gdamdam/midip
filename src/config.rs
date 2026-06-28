//! Paths and defaults.

use std::path::PathBuf;

/// Default tempo for a fresh set.
pub const DEFAULT_BPM: f64 = 120.0;

/// Project root: `CARGO_MANIFEST_DIR` at build time (stable for this single-binary app).
fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Runtime data dir (saved sets / user patterns). `$MIDIP_DATA` overrides.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MIDIP_DATA") {
        return PathBuf::from(dir);
    }
    project_root().join("data")
}

/// Vendored read-only pattern library dir.
pub fn patterns_dir() -> PathBuf {
    project_root().join("assets").join("patterns")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_dir_ends_with_assets_patterns() {
        let p = patterns_dir();
        let s = p.to_string_lossy();
        assert!(
            s.ends_with("assets/patterns") || s.ends_with("assets\\patterns"),
            "patterns_dir was {s}"
        );
    }

    #[test]
    fn default_bpm_is_120() {
        assert_eq!(DEFAULT_BPM, 120.0);
    }
}
