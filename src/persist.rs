use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// 16-hex stable identifier. Nil = all-zero, meaning "unassigned".
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Id(String);

impl Id {
    /// The nil id: all zeros, meaning "unassigned".
    pub fn nil() -> Id {
        Id("0000000000000000".to_string())
    }

    pub fn is_nil(&self) -> bool {
        self.0 == "0000000000000000"
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First up to 8 *characters* of the id, for filenames/display.
    /// Char-boundary-safe: never panics regardless of length or byte content,
    /// unlike a raw `&id.as_str()[..8]` byte slice.
    pub fn short(&self) -> String {
        match self.0.char_indices().nth(8) {
            Some((idx, _)) => self.0[..idx].to_string(),
            None => self.0.clone(),
        }
    }

    /// True if this id matches the format produced by `generate`/`mint_id`:
    /// exactly 16 ASCII hex digits. Used to detect corrupt/foreign ids on load
    /// so they can be regenerated before anything slices them.
    pub fn is_valid(&self) -> bool {
        self.0.len() == 16 && self.0.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Deterministic generator: mix seed + counter via xorshift64 → 16 lowercase hex chars.
    /// Pure and injected: same seed+counter always yields the same Id.
    pub fn generate(seed: u64, counter: u64) -> Id {
        // Mix seed and counter together before the xorshift rounds so that
        // different counters always produce different outputs even with the same seed.
        // Use the same shift constants as the existing scheduler xorshift64 (13, 7, 17).
        let mut x = seed ^ counter.wrapping_mul(0x9E3779B97F4A7C15);
        // Prevent the degenerate all-zero state.
        if x == 0 {
            x = 0x2545F4914F6CDD1D;
        }
        // Two rounds of xorshift64 to produce 128 bits of output (two u64s → 16 hex chars).
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let hi = x;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let lo = x;
        Id(format!("{:08x}{:08x}", hi >> 32, lo >> 32))
    }
}

impl Default for Id {
    fn default() -> Self {
        Id::nil()
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Process-wide id minting using SystemTime nanos as seed + a static atomic counter.
/// Delegates the pure mixing to `Id::generate`.
pub fn mint_id() -> Id {
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    Id::generate(seed, counter)
}

/// Atomic write: write to `<path>.tmp`, flush+sync_all, then rename over `path`.
/// A crash mid-write leaves the prior file intact (or a leftover `.tmp`, never a
/// half-written target).
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension({
        let orig_ext = path.extension().unwrap_or_default().to_string_lossy();
        if orig_ext.is_empty() {
            "tmp".to_string()
        } else {
            format!("{}.tmp", orig_ext)
        }
    });

    {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
    }

    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("midip-persist-{}", nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generate_is_deterministic_and_16_hex() {
        let a = Id::generate(0xDEADBEEF, 1);
        let b = Id::generate(0xDEADBEEF, 1);
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 16);
        assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(Id::generate(0xDEADBEEF, 1), Id::generate(0xDEADBEEF, 2)); // counter varies output
    }

    #[test]
    fn nil_roundtrips_and_is_default() {
        assert!(Id::nil().is_nil());
        assert_eq!(Id::default(), Id::nil());
    }

    #[test]
    fn short_is_char_boundary_safe_for_bad_ids() {
        // Shorter than 8 chars: must return the whole string, not panic.
        let short: Id = serde_json::from_str(r#""abc""#).unwrap();
        assert_eq!(short.short(), "abc");

        // Multibyte chars where a raw byte-index[..8] would land mid-character.
        let multibyte: Id = serde_json::from_str(r#""日本語abc😀""#).unwrap();
        let out = multibyte.short();
        assert_eq!(out.chars().count(), 8.min(multibyte.as_str().chars().count()));

        // Well-formed 16-hex id: first 8 chars.
        let good = Id::generate(1, 1);
        assert_eq!(good.short(), &good.as_str()[..8]);
    }

    #[test]
    fn is_valid_accepts_generated_and_nil_rejects_malformed() {
        assert!(Id::generate(0xDEADBEEF, 1).is_valid());
        assert!(Id::nil().is_valid(), "nil id is well-formed hex, just unassigned");

        let too_short: Id = serde_json::from_str(r#""abc""#).unwrap();
        assert!(!too_short.is_valid());

        let multibyte: Id = serde_json::from_str(r#""日本語abc😀""#).unwrap();
        assert!(!multibyte.is_valid());

        let non_hex: Id = serde_json::from_str(r#""zzzzzzzzzzzzzzzz""#).unwrap();
        assert!(!non_hex.is_valid());
    }

    #[test]
    fn write_atomic_creates_file_and_overwrites() {
        let dir = unique_tmp_dir();
        let p = dir.join("x.json");
        write_atomic(&p, b"first").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"first");
        write_atomic(&p, b"second").unwrap(); // overwrite via rename, not truncate
        assert_eq!(std::fs::read(&p).unwrap(), b"second");
        assert!(
            !dir.join("x.json.tmp").exists(),
            "no leftover temp on success"
        );
        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
