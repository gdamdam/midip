use crate::pattern::euclid::bjorklund;
use crate::pattern::model::{DrumHit, Lane, Pattern, PatternData};

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

// ── Velocity band for generated drum hits ────────────────────────────────────
// A range that gives lively dynamics without silent ghost notes or hard-clipping.
const VEL_MIN: u64 = 80;
const VEL_RANGE: u64 = 40; // 80..=119

/// Generate a drum pattern from scratch using Euclidean distribution for placement
/// and seeded xorshift for velocity.
///
/// `pulses = round(density / 100 * length)`; Bjorklund spreads them evenly.
/// Each active step gets one hit on the lane's root note with a velocity drawn
/// from the seeded RNG in the range `[VEL_MIN, VEL_MIN + VEL_RANGE)`.
fn generate_drums(params: &GenParams, source: &Pattern) -> Pattern {
    let length = source.length;
    let mut out = source.clone();

    // Compute pulse count: round(density/100 * length), clamped to [0, length].
    let pulses = if params.density == 0 {
        0
    } else if params.density >= 100 {
        length
    } else {
        // Use integer arithmetic to avoid f32/f64: round(d * L / 100).
        let d = params.density as usize;
        (d * length + 50) / 100
    };

    let mask = bjorklund(pulses, length, 0);
    let mut rng = params.seed;

    let steps: Vec<_> = mask
        .into_iter()
        .map(|active| {
            if active {
                let v = next_rng(&mut rng);
                let vel = (VEL_MIN + v % VEL_RANGE) as u8;
                // note: use source's first hit note if present, else 36 (kick default).
                let note = match &source.data {
                    PatternData::Drums(src_steps) => src_steps
                        .iter()
                        .flat_map(|s| s.iter())
                        .map(|h| h.note)
                        .next()
                        .unwrap_or(36),
                    _ => 36,
                };
                vec![DrumHit { note, vel, prob: 1.0, ratchet: 1 }]
            } else {
                vec![]
            }
        })
        .collect();

    out.data = PatternData::Drums(steps);
    out
}

/// Dispatch generation based on `params.mode` and the kind of `source`.
///
/// Tasks 2–4 replace the `todo!()` arms with real drum/melodic/vary logic.
/// For now every arm returns `source.clone()` so the crate compiles and the
/// purity test (same seed → same output) passes trivially.
pub fn generate(params: &GenParams, source: &Pattern, _lane: &Lane) -> Pattern {
    use crate::pattern::model::LaneKind;
    match (&params.mode, source.kind()) {
        (GenMode::Generate, LaneKind::Drums) => generate_drums(params, source),
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

    // ── Task 2: drums Generate arm ───────────────────────────────────────────

    fn active_count(pat: &Pattern) -> usize {
        match &pat.data {
            crate::pattern::model::PatternData::Drums(steps) => {
                steps.iter().filter(|s| !s.is_empty()).count()
            }
            _ => panic!("expected drums"),
        }
    }

    #[test]
    fn drums_generate_density_0_produces_no_hits() {
        let (mut p, src, lane) = fixture();
        p.density = 0;
        let out = generate(&p, &src, &lane);
        assert_eq!(active_count(&out), 0, "density=0 must produce no hits");
    }

    #[test]
    fn drums_generate_density_100_produces_all_hits() {
        let (mut p, src, lane) = fixture();
        p.density = 100;
        let out = generate(&p, &src, &lane);
        assert_eq!(
            active_count(&out),
            src.length,
            "density=100 must fill all steps"
        );
    }

    #[test]
    fn drums_generate_density_50_gives_approximately_half() {
        // 16-step pattern at density 50 → round(0.5 * 16) = 8 active steps.
        let src = Pattern::empty_drums(16);
        let lane = {
            use crate::devices::profiles::T8_DRUMS;
            use crate::music::scale::Scale;
            Lane {
                profile: T8_DRUMS,
                pattern: Pattern::empty_drums(16),
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
                route: None,
                muted_voices: vec![],
                scale: Scale::Chromatic,
                root: None,
            }
        };
        let params = GenParams { density: 50, ..GenParams::default() };
        let out = generate(&params, &src, &lane);
        assert_eq!(active_count(&out), 8);
    }

    #[test]
    fn drums_generate_is_deterministic_for_fixed_seed() {
        let (p, src, lane) = fixture();
        let a = generate(&p, &src, &lane);
        let b = generate(&p, &src, &lane);
        assert_eq!(a, b, "same seed must produce identical output");
    }

    #[test]
    fn drums_generate_different_seeds_differ() {
        let src = Pattern::empty_drums(16);
        let lane = {
            use crate::devices::profiles::T8_DRUMS;
            use crate::music::scale::Scale;
            Lane {
                profile: T8_DRUMS,
                pattern: Pattern::empty_drums(16),
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
                route: None,
                muted_voices: vec![],
                scale: Scale::Chromatic,
                root: None,
            }
        };
        let p1 = GenParams { seed: 1, density: 50, ..GenParams::default() };
        let p2 = GenParams { seed: 99999, density: 50, ..GenParams::default() };
        // Velocities are seeded so at least one step will differ.
        let a = generate(&p1, &src, &lane);
        let b = generate(&p2, &src, &lane);
        assert_ne!(a, b, "different seeds must (almost certainly) produce different velocities");
    }

    #[test]
    fn drums_generate_velocities_in_band() {
        let src = Pattern::empty_drums(16);
        let lane = {
            use crate::devices::profiles::T8_DRUMS;
            use crate::music::scale::Scale;
            Lane {
                profile: T8_DRUMS,
                pattern: Pattern::empty_drums(16),
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
                route: None,
                muted_voices: vec![],
                scale: Scale::Chromatic,
                root: None,
            }
        };
        let params = GenParams { density: 100, ..GenParams::default() };
        let out = generate(&params, &src, &lane);
        match &out.data {
            crate::pattern::model::PatternData::Drums(steps) => {
                for step in steps {
                    for hit in step {
                        assert!(
                            hit.vel >= VEL_MIN as u8 && hit.vel < (VEL_MIN + VEL_RANGE) as u8,
                            "velocity {} out of band [{}, {})",
                            hit.vel,
                            VEL_MIN,
                            VEL_MIN + VEL_RANGE
                        );
                        assert_eq!(hit.prob, 1.0);
                        assert_eq!(hit.ratchet, 1);
                    }
                }
            }
            _ => panic!("expected drums"),
        }
    }
}
