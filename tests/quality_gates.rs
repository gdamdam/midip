//! Phase 10 CI quality gates for the shipped v2 factory library.
//!
//! These lock the acceptance thresholds so future bulk additions cannot regress
//! uniqueness or metadata quality:
//!   * every v2 file parses + validates (ranges, schema, factory_id format),
//!   * zero exact structural duplicates,
//!   * factory_ids globally unique,
//!   * required metadata complete (bpm/energy/density/tags/function),
//!   * provenance carries source + references,
//!   * every pattern is assigned a known function.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use midip::pattern::format_v2::parse_pattern_v2;

fn v2_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns/v2");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .expect("v2 dir exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    v.sort();
    v
}

const FUNCTIONS: &[&str] = &[
    "core",
    "variation_a",
    "variation_b",
    "fill",
    "breakdown",
    "peak",
];

#[test]
fn every_v2_pattern_passes_all_quality_gates() {
    let files = v2_files();
    assert!(
        files.len() >= 150,
        "expected the full shipped v2 set, got {}",
        files.len()
    );

    let mut structural: HashMap<String, String> = HashMap::new();
    let mut ids: HashSet<String> = HashSet::new();

    for path in &files {
        let label = path.file_name().unwrap().to_string_lossy().to_string();
        let json = std::fs::read_to_string(path).unwrap();

        // Gate: parses + validates (ranges, schema version, factory_id format,
        // role/kind consistency, length == step count).
        let loaded = parse_pattern_v2(&json, &label)
            .unwrap_or_else(|e| panic!("{label}: failed v2 validation: {e}"));

        // Gate: factory_id globally unique.
        assert!(
            ids.insert(loaded.factory_id.clone()),
            "{label}: duplicate factory_id {}",
            loaded.factory_id
        );

        // Gate: no exact structural duplicate (role + data + cc).
        let key = format!(
            "{:?}|{}|{}",
            loaded.role,
            serde_json::to_string(&loaded.pattern.data).unwrap(),
            serde_json::to_string(&loaded.pattern.cc).unwrap(),
        );
        if let Some(prev) = structural.insert(key, loaded.factory_id.clone()) {
            panic!("{label}: structural duplicate of {prev}");
        }

        // Gate: required metadata complete.
        let m = &loaded.metadata;
        for req in [
            "bpm_min", "bpm_max", "energy", "density", "tags", "function",
        ] {
            assert!(m.contains_key(req), "{label}: missing metadata '{req}'");
        }
        assert!(
            m.get("tags")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "{label}: tags must be a non-empty array"
        );

        // Gate: every pattern assigned a KNOWN function.
        let func = m.get("function").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            FUNCTIONS.contains(&func),
            "{label}: function {func:?} is not a known function"
        );

        // Gate: provenance carries source + non-empty references.
        let prov = loaded
            .provenance
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: no provenance"));
        assert!(
            prov.get("source").and_then(|v| v.as_str()).is_some(),
            "{label}: provenance.source"
        );
        assert!(
            prov.get("references")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "{label}: provenance.references must be a non-empty array"
        );
    }
}
