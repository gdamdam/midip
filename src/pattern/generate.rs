use crate::pattern::model::{Lane, Pattern};

/// Which generation strategy to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenMode {
    /// Generate a new pattern from scratch (density-driven).
    Generate,
    /// Vary the source pattern by mutating existing steps.
    Vary,
}

/// Parameters controlling pattern generation.
#[derive(Clone, Debug, PartialEq)]
pub struct GenParams {
    pub mode: GenMode,
    /// 0..=100 — how densely to fill steps.
    pub density: u8,
    /// Semitone range for melodic generation.
    pub range: u8,
    /// 0..=100 — how aggressively to mutate (used by Vary).
    pub mutate: u8,
    /// Seed for the deterministic RNG.
    pub seed: u64,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams {
            mode: GenMode::Generate,
            density: 50,
            range: 12,
            mutate: 25,
            seed: 1,
        }
    }
}

/// One step of xorshift64 using the same shift constants (13/7/17) as persist.rs.
///
/// This is a free function so Tasks 2–4 can call it without importing persist.
/// The caller owns the state u64 and passes it by mutable reference; the new
/// state is written back and the output (same value) is returned.
///
/// The all-zero state would cycle forever producing zeros, so callers should
/// seed with a non-zero value (GenParams::seed defaults to 1).
// Tasks 2–4 will call this; allow dead_code until then.
#[allow(dead_code)]
pub(crate) fn next_rng(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Dispatch generation based on `params.mode` and the kind of `source`.
///
/// Tasks 2–4 replace the `todo!()` arms with real drum/melodic/vary logic.
/// For now every arm returns `source.clone()` so the crate compiles and the
/// purity test (same seed → same output) passes trivially.
pub fn generate(params: &GenParams, source: &Pattern, _lane: &Lane) -> Pattern {
    use crate::pattern::model::LaneKind;
    match (&params.mode, source.kind()) {
        (GenMode::Generate, LaneKind::Drums) => {
            // Task 2: drum generation
            source.clone()
        }
        (GenMode::Generate, LaneKind::Melodic) => {
            // Task 3: melodic generation
            source.clone()
        }
        (GenMode::Vary, LaneKind::Drums) => {
            // Task 4: drum variation
            source.clone()
        }
        (GenMode::Vary, LaneKind::Melodic) => {
            // Task 4: melodic variation
            source.clone()
        }
    }
}

#[cfg(test)]
mod gen_core_tests {
    use super::*;
    use crate::devices::profiles::T8_DRUMS;
    use crate::music::scale::Scale;
    use crate::pattern::model::Pattern;

    /// Minimal fixture: a 4-step empty drum pattern + a matching Lane.
    fn fixture() -> (GenParams, Pattern, Lane) {
        let src = Pattern::empty_drums(4);
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern::empty_drums(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: vec![],
            scale: Scale::Chromatic,
            root: None,
        };
        let params = GenParams::default();
        (params, src, lane)
    }

    #[test]
    fn rng_is_deterministic() {
        let mut a = 12345u64;
        let mut b = 12345u64;
        assert_eq!(next_rng(&mut a), next_rng(&mut b));
        assert_ne!(next_rng(&mut a), 0);
    }

    #[test]
    fn genparams_default_sane() {
        let p = GenParams::default();
        assert!(p.density <= 100 && p.mutate <= 100);
    }

    #[test]
    fn generate_is_pure_same_seed_same_output() {
        let (p, src, lane) = fixture();
        assert_eq!(generate(&p, &src, &lane), generate(&p, &src, &lane));
    }
}
