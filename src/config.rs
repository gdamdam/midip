//! Application configuration.
//!
//! This is a minimal stub for Task 13. Task 21 will expand this module with
//! full config file loading, set auto-save, and device preferences.

use std::path::PathBuf;

/// Root data directory: `$MIDIP_DATA` if set, otherwise `"data"` relative to the
/// current working directory. Task 21 will add platform-default paths.
pub fn data_dir() -> PathBuf {
    std::env::var("MIDIP_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"))
}
