//! Documented timing templates (Phase 7).
//!
//! A timing template deterministically maps a step's position to a microtiming
//! offset in **permille of a step** (the unit of `MelodicNote.micro`/`DrumHit.micro`;
//! see `engine::scheduler::micro_offset_micros`). Templates are the single source of
//! truth for "feel": factory patterns bake their per-note `micro` from a named
//! template, record the name in `metadata.timing`, and a lint recomputes the offsets
//! and asserts the baked values match — so a "swing"/"laid-back" claim can never
//! drift from what the pattern actually encodes.
//!
//! Swing % ↔ fraction on a 16th grid: the off-16ths (odd indices) are delayed by
//! `(swing% − 0.5) × 2` of a step. 58% ⇒ +0.16 step ⇒ +160 permille.
//!
//! Genuine triplet *grids* (12-step bars) are deferred to a later phase; here
//! `triplet-shuffle` approximates a triplet feel by pulling the off-16th to the 2/3
//! position (+1/3 step) on the normal 16-grid.

/// A named timing feel. `snake_case` names are the on-disk `metadata.timing` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Straight,
    LightSwing,
    MpcSwing,
    HardSwing,
    TripletShuffle,
    LaidBack,
    Pushed,
    Humanized,
}

impl Timing {
    pub fn parse(s: &str) -> Option<Timing> {
        Some(match s {
            "straight" => Timing::Straight,
            "light-swing" => Timing::LightSwing,
            "mpc-swing" => Timing::MpcSwing,
            "hard-swing" => Timing::HardSwing,
            "triplet-shuffle" => Timing::TripletShuffle,
            "laid-back" => Timing::LaidBack,
            "pushed" => Timing::Pushed,
            "humanized" => Timing::Humanized,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Timing::Straight => "straight",
            Timing::LightSwing => "light-swing",
            Timing::MpcSwing => "mpc-swing",
            Timing::HardSwing => "hard-swing",
            Timing::TripletShuffle => "triplet-shuffle",
            Timing::LaidBack => "laid-back",
            Timing::Pushed => "pushed",
            Timing::Humanized => "humanized",
        }
    }

    /// True when this template actually displaces at least one step (i.e. any
    /// pattern claiming it must carry non-zero `micro`).
    pub fn is_straight(&self) -> bool {
        matches!(self, Timing::Straight)
    }
}

/// Deterministic per-position jitter for `Humanized`, in permille of a step.
/// Pure integer math so it is byte-reproducible and mirrorable in the generator:
/// range is −50..=+49 permille (±~0.05 step).
fn humanized_permille(step_index: usize, seed: u32) -> i16 {
    let mut h = ((step_index as u32).wrapping_add(1)).wrapping_mul(2_654_435_761) ^ seed;
    h ^= h >> 13;
    let lo = (h & 0xFFFF) as i32;
    (lo * 100 / 65_535 - 50) as i16
}

/// The microtiming offset (permille of a step) a template assigns to `step_index`
/// on a 16-per-bar grid. `seed` is used only by `Humanized`.
///
/// Offsets never exceed ±0.49 step, so combined with any legal swing they stay
/// within the scheduler's ±½-step clamp.
pub fn offset_permille(t: Timing, step_index: usize, seed: u32) -> i16 {
    let odd = !step_index.is_multiple_of(2); // the "e"/"a" 16ths
    match t {
        Timing::Straight => 0,
        Timing::LightSwing => {
            if odd {
                80
            } else {
                0
            }
        }
        Timing::MpcSwing => {
            if odd {
                160
            } else {
                0
            }
        }
        Timing::HardSwing => {
            if odd {
                280
            } else {
                0
            }
        }
        Timing::TripletShuffle => {
            if odd {
                333
            } else {
                0
            }
        }
        // Backbeat (beats 2 & 4 = steps 4 and 12) dragged late; kicks stay on grid.
        Timing::LaidBack => {
            if step_index == 4 || step_index == 12 {
                100
            } else {
                0
            }
        }
        // The off-beat eighths (the "and"s) nudged early.
        Timing::Pushed => {
            if matches!(step_index, 2 | 6 | 10 | 14) {
                -60
            } else {
                0
            }
        }
        Timing::Humanized => humanized_permille(step_index, seed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swing_delays_only_odd_16ths() {
        for t in [Timing::LightSwing, Timing::MpcSwing, Timing::HardSwing] {
            for i in 0..16 {
                let o = offset_permille(t, i, 0);
                if i % 2 == 0 {
                    assert_eq!(o, 0, "even step {i} must stay on grid for {t:?}");
                } else {
                    assert!(o > 0, "odd step {i} must be delayed for {t:?}");
                }
            }
        }
        assert_eq!(offset_permille(Timing::MpcSwing, 1, 0), 160);
    }

    #[test]
    fn straight_is_all_zero() {
        assert!((0..16).all(|i| offset_permille(Timing::Straight, i, 0) == 0));
        assert!(Timing::Straight.is_straight());
        assert!(!Timing::MpcSwing.is_straight());
    }

    #[test]
    fn laid_back_only_drags_backbeat() {
        for i in 0..16 {
            let o = offset_permille(Timing::LaidBack, i, 0);
            assert_eq!(o, if i == 4 || i == 12 { 100 } else { 0 });
        }
    }

    #[test]
    fn humanized_is_deterministic_and_bounded() {
        let a: Vec<i16> = (0..16)
            .map(|i| offset_permille(Timing::Humanized, i, 42))
            .collect();
        let b: Vec<i16> = (0..16)
            .map(|i| offset_permille(Timing::Humanized, i, 42))
            .collect();
        assert_eq!(a, b, "same seed must reproduce identical offsets");
        assert!(a.iter().all(|&o| (-50..=49).contains(&o)), "bounded jitter");
        // A different seed yields a different sequence.
        let c: Vec<i16> = (0..16)
            .map(|i| offset_permille(Timing::Humanized, i, 7))
            .collect();
        assert_ne!(a, c);
    }

    #[test]
    fn all_offsets_within_half_step() {
        for name in [
            "straight",
            "light-swing",
            "mpc-swing",
            "hard-swing",
            "triplet-shuffle",
            "laid-back",
            "pushed",
            "humanized",
        ] {
            let t = Timing::parse(name).unwrap();
            assert_eq!(t.name(), name);
            for i in 0..16 {
                assert!(
                    offset_permille(t, i, 1).abs() <= 490,
                    "{name} step {i} exceeds ±0.49 step"
                );
            }
        }
    }
}
