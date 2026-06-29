/// Scale theory utilities — pure, deterministic, no I/O.
///
/// # Fold tie-breaking rule
/// When a pitch class is equidistant between two in-scale degrees, we round DOWN
/// (toward the lower degree, i.e., the one with the smaller semitone value within
/// the octave). This keeps melodic motion predictable and biased toward the root.
///
/// # Note-name octave convention
/// C4 = MIDI 60 (middle C). Octave number = (midi / 12) - 1.
/// Black keys are named with sharps: C#, D#, F#, G#, A#.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum Scale {
    /// Identity: all 12 semitones are in-scale. Used as the safe default so that
    /// old sets (no `scale` field) behave identically to before M5a.
    #[default]
    Chromatic,
    Major,
    NaturalMinor,
    HarmonicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
}

impl Scale {
    /// Every variant, in a fixed order suitable for UI cycling.
    pub fn all() -> &'static [Scale] {
        &[
            Scale::Chromatic,
            Scale::Major,
            Scale::NaturalMinor,
            Scale::HarmonicMinor,
            Scale::Dorian,
            Scale::Phrygian,
            Scale::Lydian,
            Scale::Mixolydian,
            Scale::MajorPentatonic,
            Scale::MinorPentatonic,
            Scale::Blues,
        ]
    }

    /// Human-readable name for display.
    pub fn name(&self) -> &'static str {
        match self {
            Scale::Chromatic => "Chromatic",
            Scale::Major => "Major",
            Scale::NaturalMinor => "Natural Minor",
            Scale::HarmonicMinor => "Harmonic Minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::MajorPentatonic => "Major Pentatonic",
            Scale::MinorPentatonic => "Minor Pentatonic",
            Scale::Blues => "Blues",
        }
    }

    /// Semitone offsets from root within one octave (ascending, no duplicates).
    pub fn degrees(&self) -> &'static [u8] {
        match self {
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Scale::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::MajorPentatonic => &[0, 2, 4, 7, 9],
            Scale::MinorPentatonic => &[0, 3, 5, 7, 10],
            Scale::Blues => &[0, 3, 5, 6, 7, 10],
        }
    }
}

/// Snap `semi` (semitone offset relative to root, any sign) to the nearest in-scale degree.
///
/// For [`Scale::Chromatic`] this is the identity function.
///
/// Tie-breaking: equidistant → round DOWN (lower/smaller degree wins).
/// Octave is preserved: only the pitch class within the octave is snapped.
///
/// Handles negative `semi` via `rem_euclid`.
pub fn fold_to_scale(semi: i32, scale: Scale) -> i32 {
    if scale == Scale::Chromatic {
        return semi;
    }
    let degrees = scale.degrees();
    // Decompose into octave + pitch-class using Euclidean remainder (handles negatives).
    let octave = semi.div_euclid(12);
    let pc = semi.rem_euclid(12) as u8;

    // Find the nearest degree. Tie → lower degree (round down).
    let best = degrees.iter().copied().min_by_key(|&d| {
        // Distance on the chromatic circle within the octave (linear, not circular).
        // We want the nearest degree *within* [0,11]; no wrapping across octave boundary
        // for the distance computation (matches the "keep the octave" requirement).
        let diff = (d as i32 - pc as i32).abs();
        // Secondary key: use `d` itself so ties go to the lower degree.
        (diff, d)
    });

    // Safety: degrees is never empty for any Scale variant.
    let best_d = best.unwrap_or(0);
    octave * 12 + best_d as i32
}

/// Move `semi` by `delta` scale degrees (signed).
///
/// For [`Scale::Chromatic`], delta is in semitones (identity movement).
/// For other scales: snap `semi` to the nearest degree index first, then
/// advance/retreat `delta` indices — wrapping across octave boundaries as needed.
pub fn step_by_degree(semi: i32, delta: i32, scale: Scale) -> i32 {
    if scale == Scale::Chromatic {
        return semi + delta;
    }
    let degrees = scale.degrees();
    let n = degrees.len() as i32;

    // Find the current snapped position.
    let snapped = fold_to_scale(semi, scale);
    let octave = snapped.div_euclid(12);
    let pc = snapped.rem_euclid(12) as u8;

    // Find the index of the snapped pitch class within degrees.
    let idx = degrees.iter().position(|&d| d == pc).unwrap_or(0) as i32;

    // Move by delta, wrapping across octave boundaries.
    let new_idx_raw = idx + delta;
    let new_octave_offset = new_idx_raw.div_euclid(n);
    let new_idx = new_idx_raw.rem_euclid(n) as usize;

    (octave + new_octave_offset) * 12 + degrees[new_idx] as i32
}

/// Absolute MIDI note (0–127) → note name with octave, e.g. 60 → "C4".
///
/// Convention: C4 = MIDI 60. Black keys use sharps: C#, D#, F#, G#, A#.
pub fn note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let pc = (midi % 12) as usize;
    // octave: C4=60, so octave = (midi / 12) - 1.
    let octave = (midi as i32 / 12) - 1;
    format!("{}{}", NAMES[pc], octave)
}

/// Scale-degree label for an absolute MIDI note relative to `root` within `scale`.
///
/// Returns e.g. "1", "b3", "5", "#4/b5", or "—" if the pitch class is out of scale.
/// For [`Scale::Chromatic`], all 12 pitch classes are labelled by semitone interval.
pub fn degree_label(midi: u8, root: u8, scale: Scale) -> String {
    // Interval from root (pitch class, 0..12).
    let interval = ((midi as i32 - root as i32).rem_euclid(12)) as u8;

    // Chromatic labels for all 12 semitones.
    const CHROMATIC_LABELS: [&str; 12] = [
        "1", "b2", "2", "b3", "3", "4", "#4/b5", "5", "b6", "6", "b7", "7",
    ];

    if scale == Scale::Chromatic {
        return CHROMATIC_LABELS[interval as usize].to_string();
    }

    let degrees = scale.degrees();
    if !degrees.contains(&interval) {
        return "\u{2014}".to_string(); // "—"
    }

    CHROMATIC_LABELS[interval as usize].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_has_11_scales() {
        assert_eq!(Scale::all().len(), 11);
    }

    #[test]
    fn major_degrees() {
        assert_eq!(Scale::Major.degrees(), &[0u8, 2, 4, 5, 7, 9, 11]);
    }

    #[test]
    fn natural_minor_degrees() {
        assert_eq!(Scale::NaturalMinor.degrees(), &[0u8, 2, 3, 5, 7, 8, 10]);
    }

    #[test]
    fn minor_pentatonic_degrees() {
        assert_eq!(Scale::MinorPentatonic.degrees(), &[0u8, 3, 5, 7, 10]);
    }

    #[test]
    fn chromatic_fold_is_identity() {
        // Chromatic: fold returns input unchanged for any semi, including negatives.
        for semi in [-12i32, -1, 0, 1, 6, 11, 12, 23, 24] {
            assert_eq!(fold_to_scale(semi, Scale::Chromatic), semi);
        }
    }

    #[test]
    fn fold_snaps_to_nearest_in_major() {
        // semi=1 (C#): nearest in Major are C(0) dist=1 and D(2) dist=1 → tie → round DOWN → 0
        assert_eq!(fold_to_scale(1, Scale::Major), 0);
        // semi=6 (F#): nearest in Major are F(5) dist=1 and G(7) dist=1 → tie → round DOWN → 5
        assert_eq!(fold_to_scale(6, Scale::Major), 5);
        // semi=3 (D#): nearest in Major are D(2) dist=1 and E(4) dist=1 → tie → round DOWN → 2
        assert_eq!(fold_to_scale(3, Scale::Major), 2);
        // semi=10 (Bb): nearest in Major are A(9) dist=1 and B(11) dist=1 → tie → round DOWN → 9
        assert_eq!(fold_to_scale(10, Scale::Major), 9);
        // semi=2 (D): already in Major → unchanged
        assert_eq!(fold_to_scale(2, Scale::Major), 2);
        // semi=-1: pitch class = 11 (B), which is in Major → -1 stays as-is
        assert_eq!(fold_to_scale(-1, Scale::Major), -1);
    }

    #[test]
    fn fold_preserves_octave() {
        // semi=13 is pitch class 1 (C#), nearest in Major: C(0) tie D(2) → round DOWN → 0
        // octave=1, so result = 12 + 0 = 12
        assert_eq!(fold_to_scale(13, Scale::Major), 12);
        // semi=14 is pitch class 2 (D), in Major → 12+2=14
        assert_eq!(fold_to_scale(14, Scale::Major), 14);
    }

    #[test]
    fn step_by_degree_major_plus_one_is_two_semis() {
        // From semi=0 (root=C in Major), +1 degree → D = semi 2
        assert_eq!(step_by_degree(0, 1, Scale::Major), 2);
        // From semi=0, +2 degrees → E = semi 4
        assert_eq!(step_by_degree(0, 2, Scale::Major), 4);
        // Chromatic: +1 → +1 semitone
        assert_eq!(step_by_degree(0, 1, Scale::Chromatic), 1);
        assert_eq!(step_by_degree(5, 1, Scale::Chromatic), 6);
    }

    #[test]
    fn step_by_degree_wraps_octave() {
        // In Major, degrees are [0,2,4,5,7,9,11], 7 degrees.
        // From semi=11 (B, last degree in octave 0), +1 → next octave's degree 0 = semi 12 (C).
        assert_eq!(step_by_degree(11, 1, Scale::Major), 12);
        // From semi=0, +7 degrees → wraps to next octave's degree 0 = semi 12
        assert_eq!(step_by_degree(0, 7, Scale::Major), 12);
    }

    #[test]
    fn step_by_degree_negative() {
        // From semi=2 (D in Major), -1 degree → C = semi 0
        assert_eq!(step_by_degree(2, -1, Scale::Major), 0);
        // From semi=0 (C in Major), -1 degree → B in previous octave = semi -1
        assert_eq!(step_by_degree(0, -1, Scale::Major), -1);
        // Chromatic negative
        assert_eq!(step_by_degree(5, -3, Scale::Chromatic), 2);
    }

    #[test]
    fn note_name_60_is_c4() {
        assert_eq!(note_name(60), "C4");
    }

    #[test]
    fn note_name_61_is_csharp4() {
        assert_eq!(note_name(61), "C#4");
    }

    #[test]
    fn note_name_various() {
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(127), "G9");
        assert_eq!(note_name(48), "C3");
    }

    #[test]
    fn degree_label_root_is_1() {
        // Root note always labels as "1"
        assert_eq!(degree_label(60, 60, Scale::Major), "1");
        assert_eq!(degree_label(60, 60, Scale::Chromatic), "1");
    }

    #[test]
    fn degree_label_out_of_scale_is_dash() {
        // C#(61) relative to C(60) → interval=1 → not in Major → "—"
        assert_eq!(degree_label(61, 60, Scale::Major), "\u{2014}");
        // D(62) relative to C(60) → interval=2 → in Major → "2"
        assert_eq!(degree_label(62, 60, Scale::Major), "2");
        // Eb(63) relative to C(60) → interval=3 → not in Major → "—"
        assert_eq!(degree_label(63, 60, Scale::Major), "\u{2014}");
    }

    #[test]
    fn degree_label_chromatic_all_labelled() {
        // Chromatic: no "—", all 12 get labels
        for i in 0u8..12 {
            let label = degree_label(60 + i, 60, Scale::Chromatic);
            assert_ne!(label, "\u{2014}");
        }
    }
}
