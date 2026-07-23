use crate::music::scale::{Scale, fold_to_scale};
use crate::pattern::euclid::bjorklund;
use crate::pattern::model::{
    DrumHit, Lane, MelodicNote, MelodicStep, Pattern, PatternData, TrigCond,
};

/// Which generation strategy to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenMode {
    /// Generate a new pattern from scratch (density-driven).
    Generate,
    /// Vary the source pattern by mutating existing steps.
    Vary,
    /// Offline arpeggio/sequence writer (melodic lanes only).
    Arp,
}

/// Chord/degree preset the arp cycles through, resolved against the lane scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpChord {
    Power,   // 1,5
    Triad,   // 1,3,5
    Seventh, // 1,3,5,7
    Octaves, // root only; octave stacking via arp_octaves
}

/// Order in which the degree pool is walked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArpShape {
    Up,
    Down,
    UpDown,
    Random,
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
    /// Arp: chord/degree preset.
    pub arp_chord: ArpChord,
    /// Arp: number of octave registers to stack (1..=4).
    pub arp_octaves: u8,
    /// Arp: walk direction / order.
    pub arp_shape: ArpShape,
    /// Arp: note length in steps (0.05..=1.0); short = staccato.
    pub arp_gate: f32,
    /// Arp: seeded velocity variation amount, 0..=100.
    pub arp_vel_var: u8,
}

impl Default for GenParams {
    fn default() -> Self {
        GenParams {
            mode: GenMode::Generate,
            density: 50,
            range: 12,
            mutate: 25,
            seed: 1,
            arp_chord: ArpChord::Octaves,
            arp_octaves: 2,
            arp_shape: ArpShape::UpDown,
            arp_gate: 0.5,
            arp_vel_var: 20,
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

// ── Velocity band for generated melodic notes ────────────────────────────────
// vel is a float multiplier (0..=1.3 typical). We generate in [0.8, 1.2).
// Implemented as: 0.8 + (rng % 4096) / 4096.0 * 0.4
const MEL_VEL_BASE: f32 = 0.8;
const MEL_VEL_SPREAD: f32 = 0.4;
const MEL_VEL_DENOM: u64 = 4096;

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
                vec![DrumHit {
                    note,
                    vel,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }]
            } else {
                vec![]
            }
        })
        .collect();

    out.data = PatternData::Drums(steps);
    out
}

/// Generate a melodic pattern from scratch.
///
/// For each active step (placed via Euclidean distribution on `density`), pick a
/// raw semitone offset in `[-range, +range]` using the seeded RNG, then fold it
/// to `lane.scale` via the M5 `fold_to_scale` function. Velocity is a seeded float
/// multiplier in `[MEL_VEL_BASE, MEL_VEL_BASE + MEL_VEL_SPREAD)`.
///
/// Each active step produces exactly one `MelodicNote` (mono). Inactive steps are
/// rests (`MelodicStep::default()` = empty vec).
///
/// `range = 0` → all active notes land on degree 0 (root-fold of 0 = 0 for any scale).
fn generate_melodic(params: &GenParams, source: &Pattern, lane: &Lane) -> Pattern {
    let length = source.length;
    let mut out = source.clone();

    let pulses = if params.density == 0 {
        0
    } else if params.density >= 100 {
        length
    } else {
        let d = params.density as usize;
        (d * length + 50) / 100
    };

    let mask = bjorklund(pulses, length, 0);
    let mut rng = params.seed;

    // The chromatic span to draw from: [-range, +range]. Total candidates = 2*range+1.
    // When range == 0 the only candidate is 0 (root offset, folds to root degree).
    let span = params.range as i32 * 2 + 1; // always ≥ 1

    let steps: Vec<MelodicStep> = mask
        .into_iter()
        .map(|active| {
            if active {
                // Pick a raw offset in [-range, +range].
                let raw_offset = if params.range == 0 {
                    0i32
                } else {
                    // rng % span gives [0, span); subtract range to center around 0.
                    let r = next_rng(&mut rng);
                    (r % span as u64) as i32 - params.range as i32
                };

                // Fold to lane scale (reuses M5 scale-fold; no-op for Chromatic).
                let folded = fold_to_scale(raw_offset, lane.scale);

                // Clamp to i8 (semitone offset field is i8).
                let semi = folded.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                // Seeded velocity multiplier in [MEL_VEL_BASE, MEL_VEL_BASE + MEL_VEL_SPREAD).
                let v = next_rng(&mut rng);
                let vel = MEL_VEL_BASE
                    + (v % MEL_VEL_DENOM) as f32 / MEL_VEL_DENOM as f32 * MEL_VEL_SPREAD;

                MelodicStep::from(vec![MelodicNote {
                    semi,
                    vel,
                    slide: false,
                    len: lane.profile.gate_fraction,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }])
            } else {
                MelodicStep::default()
            }
        })
        .collect();

    out.data = PatternData::Melodic(steps);
    out
}

/// Vary a drum pattern by randomly toggling hits per step.
///
/// For each step, roll `next_rng` and compare `rng % 100 < mutate` to decide
/// whether to perturb. Perturbation: if the step has hits, clear it (mute);
/// if empty, add a hit (using the source's most common note or 36). Velocity
/// of any new hit is drawn from the same [VEL_MIN, VEL_MIN+VEL_RANGE) band.
///
/// `mutate = 0` ⇒ no step is ever perturbed (identity).
fn vary_drums(params: &GenParams, source: &Pattern) -> Pattern {
    let mut out = source.clone();
    let mut rng = params.seed;

    // Pre-scan: find the most frequent note in source for new hits.
    let fallback_note = match &source.data {
        PatternData::Drums(steps) => steps
            .iter()
            .flat_map(|s| s.iter())
            .map(|h| h.note)
            .next()
            .unwrap_or(36),
        _ => 36,
    };

    if let PatternData::Drums(ref mut steps) = out.data {
        for step in steps.iter_mut() {
            let r = next_rng(&mut rng);
            let perturb = params.mutate > 0 && (r % 100) < params.mutate as u64;
            if perturb {
                if step.is_empty() {
                    // add a hit
                    let v = next_rng(&mut rng);
                    let vel = (VEL_MIN + v % VEL_RANGE) as u8;
                    step.push(DrumHit {
                        note: fallback_note,
                        vel,
                        prob: 1.0,
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
                    });
                } else {
                    // remove hits (toggle off)
                    step.clear();
                }
            }
        }
    }
    out
}

/// Vary a melodic pattern by randomly nudging pitches and jittering velocity.
///
/// For each step that has notes, roll `next_rng` and compare `rng % 100 < mutate`
/// to decide whether to perturb. Perturbation on an active step: nudge the first
/// note's pitch by a random step in `[-range, +range]` semitones, then re-fold via
/// `fold_to_scale` so the result stays in scale; also jitter velocity in the same
/// `[MEL_VEL_BASE, MEL_VEL_BASE + MEL_VEL_SPREAD)` band.
///
/// Empty steps (rests): with the same probability, toggle on (add a root note).
///
/// `mutate = 0` ⇒ identity.
fn vary_melodic(params: &GenParams, source: &Pattern, lane: &Lane) -> Pattern {
    let mut out = source.clone();
    let mut rng = params.seed;
    let span = params.range as i32 * 2 + 1; // always ≥ 1

    if let PatternData::Melodic(ref mut steps) = out.data {
        for step in steps.iter_mut() {
            let r = next_rng(&mut rng);
            let perturb = params.mutate > 0 && (r % 100) < params.mutate as u64;
            if perturb {
                if step.is_empty() {
                    // Turn rest into a root note.
                    let v = next_rng(&mut rng);
                    let vel = MEL_VEL_BASE
                        + (v % MEL_VEL_DENOM) as f32 / MEL_VEL_DENOM as f32 * MEL_VEL_SPREAD;
                    step.0.push(MelodicNote {
                        semi: 0,
                        vel,
                        slide: false,
                        len: lane.profile.gate_fraction,
                        prob: 1.0,
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
                    });
                } else {
                    // Nudge pitch and jitter velocity.
                    let nudge_raw = if params.range == 0 {
                        0i32
                    } else {
                        let v = next_rng(&mut rng);
                        (v % span as u64) as i32 - params.range as i32
                    };
                    let current = step[0].semi as i32;
                    let raw = current + nudge_raw;
                    let folded = fold_to_scale(raw, lane.scale);
                    let semi = folded.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                    let v = next_rng(&mut rng);
                    let vel = MEL_VEL_BASE
                        + (v % MEL_VEL_DENOM) as f32 / MEL_VEL_DENOM as f32 * MEL_VEL_SPREAD;

                    step[0].semi = semi;
                    step[0].vel = vel;
                }
            }
        }
    }
    out
}

/// Dispatch generation based on `params.mode` and the kind of `source`.
///
/// Tasks 2–4 replace the `todo!()` arms with real drum/melodic/vary logic.
/// For now every arm returns `source.clone()` so the crate compiles and the
/// purity test (same seed → same output) passes trivially.
pub fn generate(params: &GenParams, source: &Pattern, lane: &Lane) -> Pattern {
    use crate::pattern::model::LaneKind;
    match (&params.mode, source.kind()) {
        (GenMode::Generate, LaneKind::Drums) => generate_drums(params, source),
        (GenMode::Generate, LaneKind::Melodic) => generate_melodic(params, source, lane),
        (GenMode::Vary, LaneKind::Drums) => vary_drums(params, source),
        (GenMode::Vary, LaneKind::Melodic) => vary_melodic(params, source, lane),
        (GenMode::Arp, LaneKind::Melodic) => generate_arp(params, source, lane),
        (GenMode::Arp, LaneKind::Drums) => source.clone(),
    }
}

/// Offline arp generator. Stubbed in Task 1; implemented in Task 4.
fn generate_arp(_params: &GenParams, source: &Pattern, _lane: &Lane) -> Pattern {
    source.clone()
}

impl ArpChord {
    /// Indices into `Scale::degrees()` for non-chromatic scales.
    fn degree_indices(self) -> &'static [usize] {
        match self {
            ArpChord::Power => &[0, 4],
            ArpChord::Triad => &[0, 2, 4],
            ArpChord::Seventh => &[0, 2, 4, 6],
            ArpChord::Octaves => &[0],
        }
    }

    /// Fixed semitone intervals used when the lane scale is Chromatic
    /// (where degree indexing would not yield a real chord).
    fn chromatic_intervals(self) -> &'static [i32] {
        match self {
            ArpChord::Power => &[0, 7],
            ArpChord::Triad => &[0, 4, 7],
            ArpChord::Seventh => &[0, 4, 7, 11],
            ArpChord::Octaves => &[0],
        }
    }
}

/// Build the ascending semitone pool for one arp: resolve chord degrees through
/// the lane scale (fixed intervals for Chromatic), then stack across `octaves`
/// registers (+12 each). No preset includes the octave, so endpoints never
/// duplicate across registers. A final `fold_to_scale` guards edge cases.
fn arp_pool(chord: ArpChord, octaves: u8, scale: Scale) -> Vec<i32> {
    let base: Vec<i32> = if scale == Scale::Chromatic {
        chord.chromatic_intervals().to_vec()
    } else {
        let degs = scale.degrees();
        chord
            .degree_indices()
            .iter()
            .filter_map(|&i| degs.get(i).map(|&s| s as i32))
            .collect()
    };

    let octaves = octaves.max(1) as i32;
    let mut pool = Vec::with_capacity(base.len() * octaves as usize);
    for r in 0..octaves {
        for &interval in &base {
            pool.push(fold_to_scale(interval + 12 * r, scale));
        }
    }
    pool
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
            swing: None,
            clock_div: None,
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
                swing: None,
                clock_div: None,
            }
        };
        let params = GenParams {
            density: 50,
            ..GenParams::default()
        };
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
                swing: None,
                clock_div: None,
            }
        };
        let p1 = GenParams {
            seed: 1,
            density: 50,
            ..GenParams::default()
        };
        let p2 = GenParams {
            seed: 99999,
            density: 50,
            ..GenParams::default()
        };
        // Velocities are seeded so at least one step will differ.
        let a = generate(&p1, &src, &lane);
        let b = generate(&p2, &src, &lane);
        assert_ne!(
            a, b,
            "different seeds must (almost certainly) produce different velocities"
        );
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
                swing: None,
                clock_div: None,
            }
        };
        let params = GenParams {
            density: 100,
            ..GenParams::default()
        };
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

    // ── Task 3: melodic Generate arm ─────────────────────────────────────────

    use crate::devices::profiles::S1;

    /// Melodic fixture: 16-step empty melodic pattern + Lane with given scale.
    fn melodic_fixture(scale: Scale) -> (GenParams, Pattern, Lane) {
        let src = Pattern::empty_melodic(16);
        let lane = Lane {
            profile: S1,
            pattern: Pattern::empty_melodic(16),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: vec![],
            scale,
            root: None,
            swing: None,
            clock_div: None,
        };
        let params = GenParams {
            density: 75,
            range: 12,
            ..GenParams::default()
        };
        (params, src, lane)
    }

    /// Extract all active notes from a melodic pattern.
    fn melodic_notes(pat: &Pattern) -> Vec<i8> {
        match &pat.data {
            PatternData::Melodic(steps) => steps
                .iter()
                .flat_map(|s| s.iter().map(|n| n.semi))
                .collect(),
            _ => panic!("expected melodic"),
        }
    }

    fn melodic_active_count(pat: &Pattern) -> usize {
        match &pat.data {
            PatternData::Melodic(steps) => steps.iter().filter(|s| !s.is_empty()).count(),
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn melodic_generate_density_0_produces_no_notes() {
        let (mut p, src, lane) = melodic_fixture(Scale::Chromatic);
        p.density = 0;
        let out = generate(&p, &src, &lane);
        assert_eq!(
            melodic_active_count(&out),
            0,
            "density=0 must produce no notes"
        );
    }

    #[test]
    fn melodic_generate_density_100_fills_all_steps() {
        let (mut p, src, lane) = melodic_fixture(Scale::Chromatic);
        p.density = 100;
        let out = generate(&p, &src, &lane);
        assert_eq!(
            melodic_active_count(&out),
            src.length,
            "density=100 must fill all steps"
        );
    }

    #[test]
    fn melodic_generate_is_deterministic() {
        let (p, src, lane) = melodic_fixture(Scale::Major);
        let a = generate(&p, &src, &lane);
        let b = generate(&p, &src, &lane);
        assert_eq!(a, b, "same seed must produce identical output");
    }

    #[test]
    fn melodic_generate_different_seeds_differ() {
        let (p, src, lane) = melodic_fixture(Scale::Major);
        let p2 = GenParams {
            seed: 99999,
            ..p.clone()
        };
        let a = generate(&p, &src, &lane);
        let b = generate(&p2, &src, &lane);
        assert_ne!(a, b, "different seeds must produce different patterns");
    }

    #[test]
    fn melodic_generate_range_0_all_root_degree() {
        // range=0 → raw offset is always 0 → fold_to_scale(0, any) = 0
        let (mut p, src, lane) = melodic_fixture(Scale::Major);
        p.range = 0;
        p.density = 100;
        let out = generate(&p, &src, &lane);
        let notes = melodic_notes(&out);
        assert!(!notes.is_empty(), "density=100 must produce notes");
        for semi in &notes {
            assert_eq!(
                *semi, 0,
                "range=0 must produce only root-degree (semi=0) notes"
            );
        }
    }

    #[test]
    fn melodic_generate_all_semis_within_range() {
        let range = 7u8;
        let (mut p, src, lane) = melodic_fixture(Scale::Chromatic);
        p.range = range;
        p.density = 100;
        let out = generate(&p, &src, &lane);
        let notes = melodic_notes(&out);
        for semi in &notes {
            assert!(
                *semi >= -(range as i8) && *semi <= range as i8,
                "semi {} outside ±{} range",
                semi,
                range
            );
        }
    }

    #[test]
    fn melodic_generate_major_scale_all_in_scale() {
        let (mut p, src, lane) = melodic_fixture(Scale::Major);
        p.density = 100;
        p.range = 11; // cover all pitch classes
        let out = generate(&p, &src, &lane);
        let notes = melodic_notes(&out);
        assert!(!notes.is_empty());
        let major_degrees: &[u8] = &[0, 2, 4, 5, 7, 9, 11];
        for semi in &notes {
            let pc = semi.rem_euclid(12) as u8;
            assert!(
                major_degrees.contains(&pc),
                "semi {} (pc={}) not in Major scale",
                semi,
                pc
            );
        }
    }

    #[test]
    fn melodic_generate_minor_pentatonic_all_in_scale() {
        let (mut p, src, lane) = melodic_fixture(Scale::MinorPentatonic);
        p.density = 100;
        p.range = 11;
        let out = generate(&p, &src, &lane);
        let notes = melodic_notes(&out);
        assert!(!notes.is_empty());
        let minor_penta: &[u8] = &[0, 3, 5, 7, 10];
        for semi in &notes {
            let pc = semi.rem_euclid(12) as u8;
            assert!(
                minor_penta.contains(&pc),
                "semi {} (pc={}) not in MinorPentatonic scale",
                semi,
                pc
            );
        }
    }

    #[test]
    fn melodic_generate_chromatic_allows_any_in_range() {
        // Chromatic = identity; any semitone in [-range, +range] is valid.
        let (mut p, src, lane) = melodic_fixture(Scale::Chromatic);
        p.density = 100;
        p.range = 12;
        let out = generate(&p, &src, &lane);
        let notes = melodic_notes(&out);
        assert!(!notes.is_empty());
        // Just verify in-range (fold is identity for Chromatic).
        for semi in &notes {
            assert!(
                *semi >= -12 && *semi <= 12,
                "chromatic semi {} out of ±12 range",
                semi
            );
        }
    }

    #[test]
    fn melodic_generate_single_note_per_active_step() {
        // Mono: each active step must have exactly one note.
        let (mut p, src, lane) = melodic_fixture(Scale::Major);
        p.density = 100;
        let out = generate(&p, &src, &lane);
        match &out.data {
            PatternData::Melodic(steps) => {
                for step in steps {
                    assert!(
                        step.len() <= 1,
                        "each step must have at most 1 note (mono), got {}",
                        step.len()
                    );
                }
            }
            _ => panic!("expected melodic"),
        }
    }

    // ── Task 4: Vary arm ─────────────────────────────────────────────────────

    /// Build a non-trivial drum pattern (alternating hit/rest) for Vary tests.
    fn vary_drums_fixture() -> (Pattern, Lane) {
        let mut src = Pattern::empty_drums(8);
        if let crate::pattern::model::PatternData::Drums(ref mut steps) = src.data {
            for (i, step) in steps.iter_mut().enumerate() {
                if i % 2 == 0 {
                    step.push(DrumHit {
                        note: 36,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
                    });
                }
            }
        }
        let lane = {
            use crate::devices::profiles::T8_DRUMS;
            use crate::music::scale::Scale;
            Lane {
                profile: T8_DRUMS,
                pattern: Pattern::empty_drums(8),
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
                route: None,
                muted_voices: vec![],
                scale: Scale::Chromatic,
                root: None,
                swing: None,
                clock_div: None,
            }
        };
        (src, lane)
    }

    /// Build a non-trivial melodic pattern for Vary tests.
    fn vary_melodic_fixture(scale: Scale) -> (Pattern, Lane) {
        let notes: Vec<MelodicStep> = (0i8..8)
            .map(|i| {
                MelodicStep::from(vec![MelodicNote {
                    semi: i * 2,
                    vel: 1.0,
                    slide: false,
                    len: 0.5,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }])
            })
            .collect();
        let src = Pattern {
            length: 8,
            data: PatternData::Melodic(notes),
            ..Pattern::empty_melodic(8)
        };
        let lane = Lane {
            profile: S1,
            pattern: Pattern::empty_melodic(8),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: vec![],
            scale,
            root: None,
            swing: None,
            clock_div: None,
        };
        (src, lane)
    }

    fn count_changed_drum_steps(a: &Pattern, b: &Pattern) -> usize {
        match (&a.data, &b.data) {
            (PatternData::Drums(sa), PatternData::Drums(sb)) => {
                sa.iter().zip(sb.iter()).filter(|(x, y)| x != y).count()
            }
            _ => panic!("expected drums"),
        }
    }

    fn count_changed_melodic_steps(a: &Pattern, b: &Pattern) -> usize {
        match (&a.data, &b.data) {
            (PatternData::Melodic(sa), PatternData::Melodic(sb)) => {
                sa.iter().zip(sb.iter()).filter(|(x, y)| x != y).count()
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn vary_drums_mutate_0_is_identity() {
        let (src, lane) = vary_drums_fixture();
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 0,
            ..GenParams::default()
        };
        let out = generate(&params, &src, &lane);
        assert_eq!(out, src, "mutate=0 must leave source unchanged");
    }

    #[test]
    fn vary_drums_is_deterministic() {
        let (src, lane) = vary_drums_fixture();
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            ..GenParams::default()
        };
        let a = generate(&params, &src, &lane);
        let b = generate(&params, &src, &lane);
        assert_eq!(a, b, "same seed must produce identical vary output");
    }

    #[test]
    fn vary_drums_different_seeds_differ() {
        let (src, lane) = vary_drums_fixture();
        let p1 = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            seed: 1,
            ..GenParams::default()
        };
        let p2 = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            seed: 77777,
            ..GenParams::default()
        };
        let a = generate(&p1, &src, &lane);
        let b = generate(&p2, &src, &lane);
        assert_ne!(a, b, "different seeds must produce different vary output");
    }

    #[test]
    fn vary_drums_higher_mutate_changes_more_steps() {
        let (src, lane) = vary_drums_fixture();
        let p_low = GenParams {
            mode: GenMode::Vary,
            mutate: 10,
            seed: 42,
            ..GenParams::default()
        };
        let p_high = GenParams {
            mode: GenMode::Vary,
            mutate: 90,
            seed: 42,
            ..GenParams::default()
        };
        let low_changes = count_changed_drum_steps(&src, &generate(&p_low, &src, &lane));
        let high_changes = count_changed_drum_steps(&src, &generate(&p_high, &src, &lane));
        assert!(
            high_changes >= low_changes,
            "mutate=90 ({} changes) should change >= mutate=10 ({} changes)",
            high_changes,
            low_changes
        );
    }

    #[test]
    fn vary_melodic_mutate_0_is_identity() {
        let (src, lane) = vary_melodic_fixture(Scale::Major);
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 0,
            ..GenParams::default()
        };
        let out = generate(&params, &src, &lane);
        assert_eq!(out, src, "mutate=0 must leave melodic source unchanged");
    }

    #[test]
    fn vary_melodic_is_deterministic() {
        let (src, lane) = vary_melodic_fixture(Scale::Major);
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            ..GenParams::default()
        };
        let a = generate(&params, &src, &lane);
        let b = generate(&params, &src, &lane);
        assert_eq!(a, b, "same seed must produce identical melodic vary output");
    }

    #[test]
    fn vary_melodic_different_seeds_differ() {
        let (src, lane) = vary_melodic_fixture(Scale::Major);
        let p1 = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            seed: 1,
            ..GenParams::default()
        };
        let p2 = GenParams {
            mode: GenMode::Vary,
            mutate: 50,
            seed: 77777,
            ..GenParams::default()
        };
        let a = generate(&p1, &src, &lane);
        let b = generate(&p2, &src, &lane);
        assert_ne!(
            a, b,
            "different seeds must produce different melodic vary output"
        );
    }

    #[test]
    fn vary_melodic_higher_mutate_changes_more_steps() {
        let (src, lane) = vary_melodic_fixture(Scale::Chromatic);
        let p_low = GenParams {
            mode: GenMode::Vary,
            mutate: 10,
            seed: 42,
            range: 4,
            ..GenParams::default()
        };
        let p_high = GenParams {
            mode: GenMode::Vary,
            mutate: 90,
            seed: 42,
            range: 4,
            ..GenParams::default()
        };
        let low_changes = count_changed_melodic_steps(&src, &generate(&p_low, &src, &lane));
        let high_changes = count_changed_melodic_steps(&src, &generate(&p_high, &src, &lane));
        assert!(
            high_changes >= low_changes,
            "mutate=90 ({} changes) should change >= mutate=10 ({} changes)",
            high_changes,
            low_changes
        );
    }

    #[test]
    fn vary_melodic_pitches_stay_in_major_scale() {
        let (src, lane) = vary_melodic_fixture(Scale::Major);
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 100,
            range: 12,
            ..GenParams::default()
        };
        let out = generate(&params, &src, &lane);
        let major_degrees: &[u8] = &[0, 2, 4, 5, 7, 9, 11];
        match &out.data {
            PatternData::Melodic(steps) => {
                for step in steps {
                    for note in step.iter() {
                        let pc = note.semi.rem_euclid(12) as u8;
                        assert!(
                            major_degrees.contains(&pc),
                            "varied semi {} (pc={}) not in Major scale",
                            note.semi,
                            pc
                        );
                    }
                }
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn vary_melodic_velocity_in_band_after_vary() {
        let (src, lane) = vary_melodic_fixture(Scale::Chromatic);
        let params = GenParams {
            mode: GenMode::Vary,
            mutate: 100,
            range: 4,
            ..GenParams::default()
        };
        let out = generate(&params, &src, &lane);
        match &out.data {
            PatternData::Melodic(steps) => {
                for step in steps {
                    for note in step.iter() {
                        assert!(
                            note.vel >= MEL_VEL_BASE && note.vel < MEL_VEL_BASE + MEL_VEL_SPREAD,
                            "varied velocity {} out of band",
                            note.vel
                        );
                    }
                }
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn melodic_generate_velocity_in_band() {
        let (mut p, src, lane) = melodic_fixture(Scale::Chromatic);
        p.density = 100;
        let out = generate(&p, &src, &lane);
        match &out.data {
            PatternData::Melodic(steps) => {
                for step in steps {
                    for note in step.iter() {
                        assert!(
                            note.vel >= MEL_VEL_BASE && note.vel < MEL_VEL_BASE + MEL_VEL_SPREAD,
                            "velocity {} out of band [{}, {})",
                            note.vel,
                            MEL_VEL_BASE,
                            MEL_VEL_BASE + MEL_VEL_SPREAD
                        );
                        assert_eq!(note.prob, 1.0);
                        assert_eq!(note.ratchet, 1);
                        assert!(!note.slide);
                    }
                }
            }
            _ => panic!("expected melodic"),
        }
    }

    // ── Task 1: GenMode::Arp scaffolding ─────────────────────────────────────

    #[test]
    fn arp_on_drum_lane_is_noop() {
        // Arp is melodic-only; on a drum source it must return the source unchanged.
        let (mut params, source, lane) = fixture(); // drum fixture at line ~339
        params.mode = GenMode::Arp;
        let out = generate(&params, &source, &lane);
        assert_eq!(out.data, source.data, "Arp on a drum lane must be a no-op");
    }

    #[test]
    fn genparams_default_has_arp_fields() {
        let p = GenParams::default();
        // Sensible arp defaults (only used when mode == Arp).
        assert_eq!(p.arp_octaves, 2);
        assert_eq!(p.arp_chord, ArpChord::Octaves);
        assert_eq!(p.arp_shape, ArpShape::UpDown);
        assert!((p.arp_gate - 0.5).abs() < f32::EPSILON);
        assert_eq!(p.arp_vel_var, 20);
    }

    // ── Task 2: arp_pool construction ────────────────────────────────────────

    #[test]
    fn arp_pool_octaves_stacks_root_per_register() {
        use crate::music::scale::Scale;
        // Octaves preset = root only; 2 registers => [0, 12], NOT [0,12,24].
        assert_eq!(arp_pool(ArpChord::Octaves, 2, Scale::Major), vec![0, 12]);
        assert_eq!(arp_pool(ArpChord::Octaves, 1, Scale::Major), vec![0]);
    }

    #[test]
    fn arp_pool_triad_major_two_registers() {
        use crate::music::scale::Scale;
        // Major degrees [0,2,4,5,7,9,11]; triad indices 0,2,4 => [0,4,7].
        assert_eq!(
            arp_pool(ArpChord::Triad, 2, Scale::Major),
            vec![0, 4, 7, 12, 16, 19]
        );
    }

    #[test]
    fn arp_pool_chromatic_uses_fixed_chord_intervals() {
        use crate::music::scale::Scale;
        // Under Chromatic, degrees()==0..11, so scale-index would not be a chord.
        // Use fixed intervals instead.
        assert_eq!(arp_pool(ArpChord::Triad, 1, Scale::Chromatic), vec![0, 4, 7]);
        assert_eq!(arp_pool(ArpChord::Seventh, 1, Scale::Chromatic), vec![0, 4, 7, 11]);
        assert_eq!(arp_pool(ArpChord::Power, 1, Scale::Chromatic), vec![0, 7]);
        assert_eq!(arp_pool(ArpChord::Octaves, 1, Scale::Chromatic), vec![0]);
    }

    #[test]
    fn arp_pool_skips_missing_degrees_on_pentatonic() {
        use crate::music::scale::Scale;
        // MajorPentatonic degrees [0,2,4,7,9] (5 entries); Seventh wants indices
        // 0,2,4,6 — index 6 does not exist and is skipped.
        assert_eq!(
            arp_pool(ArpChord::Seventh, 1, Scale::MajorPentatonic),
            vec![0, 4, 9]
        );
    }
}
