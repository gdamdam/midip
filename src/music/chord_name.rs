//! Chord-name parsing + voice-led progression building — pure, deterministic, no I/O.
//!
//! Turns a typed progression like `"Dm7 G7 Cmaj7 A7"` into a melodic `Pattern`
//! whose steps hold voiced chords (one sustained chord per bar). Voicings are
//! capped at four notes (the Roland J-6's polyphony) and voice-led so successive
//! chords move their voices minimally. The parser and voicer are independent of
//! any lane/device; `build_progression_pattern` layers the pattern assembly on top.

use crate::pattern::model::{MelodicNote, MelodicStep, Pattern, PatternData, TrigCond};

/// Maximum simultaneous notes per chord — the four-voice baseline of the J-6.
const MAX_VOICES: usize = 4;

/// A parsed chord: its root pitch-class (0 = C … 11 = B) and the interval set
/// (semitones above the root) of its quality, already reduced to ≤4 tones. The
/// original `symbol` is retained for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedChord {
    pub root_pc: i32,
    pub intervals: Vec<i32>,
    pub symbol: String,
}

impl ParsedChord {
    /// Distinct pitch classes (0..12) this chord sounds.
    fn pitch_classes(&self) -> Vec<i32> {
        let mut pcs: Vec<i32> = self
            .intervals
            .iter()
            .map(|i| (self.root_pc + i).rem_euclid(12))
            .collect();
        pcs.sort_unstable();
        pcs.dedup();
        pcs.truncate(MAX_VOICES);
        pcs
    }
}

/// The interval set (semitones from root) for a quality suffix, or `None` if the
/// suffix is unknown. Every set is pre-reduced to at most four tones (7th/9th
/// chords omit the fifth) so a voicing never exceeds the J-6's four voices.
fn quality_intervals(suffix: &str) -> Option<Vec<i32>> {
    let set: &[i32] = match suffix {
        "" | "maj" | "major" | "M" | "ma" | "Δ" => &[0, 4, 7],
        "m" | "min" | "-" | "mi" => &[0, 3, 7],
        "dim" | "o" | "°" => &[0, 3, 6],
        "aug" | "+" => &[0, 4, 8],
        "5" => &[0, 7],
        "sus2" => &[0, 2, 7],
        "sus4" | "sus" => &[0, 5, 7],
        "6" | "maj6" | "M6" => &[0, 4, 7, 9],
        "m6" | "min6" | "-6" => &[0, 3, 7, 9],
        "7" | "dom7" => &[0, 4, 7, 10],
        "maj7" | "M7" | "Δ7" | "ma7" => &[0, 4, 7, 11],
        "m7" | "min7" | "-7" => &[0, 3, 7, 10],
        "m7b5" | "ø" | "min7b5" | "-7b5" | "halfdim" => &[0, 3, 6, 10],
        "dim7" | "o7" | "°7" => &[0, 3, 6, 9],
        // 9th chords: keep root, 3rd, 7th, 9th — the 5th is dropped to stay ≤4.
        "9" => &[0, 4, 10, 14],
        "maj9" | "M9" => &[0, 4, 11, 14],
        "m9" | "min9" | "-9" => &[0, 3, 10, 14],
        "add9" => &[0, 4, 7, 14],
        "7sus4" | "7sus" => &[0, 5, 7, 10],
        _ => return None,
    };
    Some(set.to_vec())
}

/// Parse one chord token (e.g. `"Dm7"`, `"Bb"`, `"F#m7b5"`, `"C/G"`). A trailing
/// slash-bass (`/G`) is accepted but ignored — only the upper chord is voiced.
pub fn parse_chord(token: &str) -> Result<ParsedChord, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("empty chord".to_string());
    }
    // Drop a slash-bass: "C/G" → "C".
    let core = token.split('/').next().unwrap_or(token);
    let mut chars = core.chars().peekable();

    let letter = chars.next().unwrap();
    let mut root_pc: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("'{token}': chord must start with A–G")),
    };
    // Accidentals directly after the letter.
    while let Some(&c) = chars.peek() {
        match c {
            '#' | '♯' => root_pc += 1,
            'b' | '♭' => root_pc -= 1,
            _ => break,
        }
        chars.next();
    }
    let suffix: String = chars.collect();
    let intervals = quality_intervals(&suffix)
        .ok_or_else(|| format!("'{token}': unknown chord quality '{suffix}'"))?;
    Ok(ParsedChord {
        root_pc: root_pc.rem_euclid(12),
        intervals,
        symbol: token.to_string(),
    })
}

/// Parse a whole progression. Tokens are separated by whitespace, commas, or bar
/// lines (`|`); `-` is NOT a separator (it is a minor-chord symbol). Returns an
/// error naming the first token that fails.
pub fn parse_progression(text: &str) -> Result<Vec<ParsedChord>, String> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == '|')
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err("no chords entered".to_string());
    }
    tokens.iter().map(|t| parse_chord(t)).collect()
}

/// Voice a progression into absolute-pitch chords (≤4 notes each), centered near
/// `center` and voice-led so each chord moves minimally from the previous one.
/// The first chord is placed nearest `center`; later chords pick the octave
/// placement (inversion) that minimizes total voice movement.
pub fn voice_progression(chords: &[ParsedChord], center: i32) -> Vec<Vec<i32>> {
    let (lo, hi) = (center - 12, center + 18);
    let mut out: Vec<Vec<i32>> = Vec::with_capacity(chords.len());
    let mut prev: Option<Vec<i32>> = None;
    for ch in chords {
        let pcs = ch.pitch_classes();
        let best = best_voicing(&pcs, lo, hi, center, prev.as_deref());
        prev = Some(best.clone());
        out.push(best);
    }
    out
}

/// Choose the voicing of `pcs` within `[lo, hi]` that best voice-leads from
/// `prev` (or sits nearest `center` for the first chord). Enumerates every
/// per-voice octave placement (≤4 voices × a few octaves = a small search).
fn best_voicing(pcs: &[i32], lo: i32, hi: i32, center: i32, prev: Option<&[i32]>) -> Vec<i32> {
    // Candidate pitches per pitch-class within the register window.
    let per_pc: Vec<Vec<i32>> = pcs
        .iter()
        .map(|&pc| (lo..=hi).filter(|m| m.rem_euclid(12) == pc).collect())
        .collect();

    let mut best: Option<(f64, Vec<i32>)> = None;
    let mut cand = vec![0i32; pcs.len()];
    // Cartesian product over the per-voice octave choices.
    let mut idx = vec![0usize; pcs.len()];
    loop {
        for (v, opts) in per_pc.iter().enumerate() {
            cand[v] = opts[idx[v]];
        }
        let mut voicing = cand.clone();
        voicing.sort_unstable();
        let span = voicing.last().unwrap() - voicing.first().unwrap();
        // Keep only compact voicings (within ~1.5 octaves) so chords stay readable.
        if span <= 18 {
            let score = voicing_score(&voicing, span, center, prev);
            if best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
                best = Some((score, voicing));
            }
        }
        // Advance the odometer over `idx`.
        let mut k = 0;
        loop {
            if k == idx.len() {
                return best.map(|(_, v)| v).unwrap_or_else(|| {
                    // Degenerate fallback: root-position stack from `lo`.
                    pcs.iter()
                        .map(|&pc| lo + (pc - lo).rem_euclid(12))
                        .collect()
                });
            }
            idx[k] += 1;
            if idx[k] < per_pc[k].len() {
                break;
            }
            idx[k] = 0;
            k += 1;
        }
    }
}

/// Lower is better: total voice-leading distance to the previous chord (nearest
/// note per voice), plus a small compactness penalty. For the first chord,
/// distance of the voicing's centroid to `center`.
fn voicing_score(voicing: &[i32], span: i32, center: i32, prev: Option<&[i32]>) -> f64 {
    let compact = span as f64 * 0.1;
    match prev {
        None => {
            let centroid = voicing.iter().sum::<i32>() as f64 / voicing.len() as f64;
            (centroid - center as f64).abs() + compact
        }
        Some(prev) => {
            let lead: i32 = voicing
                .iter()
                .map(|&n| prev.iter().map(|&p| (n - p).abs()).min().unwrap_or(0))
                .sum();
            lead as f64 + compact
        }
    }
}

/// Build a melodic `Pattern` from typed chord names: one sustained chord per bar.
/// The pattern length is `chords × steps_per_bar`, capped at 64 (the bar length
/// shrinks to fit when the progression is long). Voicings are relative to
/// `root_note` (each note's `semi` is `pitch − root_note`).
pub fn build_progression_pattern(
    text: &str,
    steps_per_bar: usize,
    root_note: u8,
) -> Result<Pattern, String> {
    let chords = parse_progression(text)?;
    let center = root_note as i32 + 12; // voice chords an octave above the lane root
    let voicings = voice_progression(&chords, center);

    let n = chords.len();
    let spb = steps_per_bar.max(1);
    // One bar per chord, but never exceed the 64-step pattern maximum.
    let bar = if n * spb > 64 { (64 / n).max(1) } else { spb };
    let length = (n * bar).min(64).max(1);

    let mut steps: Vec<MelodicStep> = vec![MelodicStep(Vec::new()); length];
    for (i, voicing) in voicings.iter().enumerate() {
        let at = i * bar;
        if at >= length {
            break; // ran past the cap
        }
        let notes: Vec<MelodicNote> = voicing
            .iter()
            .map(|&pitch| MelodicNote {
                semi: (pitch - root_note as i32).clamp(-127, 127) as i8,
                vel: 1.0,
                slide: false,
                len: bar as f32, // sustain the chord across its bar
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            })
            .collect();
        steps[at] = MelodicStep(notes);
    }

    let label: String = chords
        .iter()
        .map(|c| c.symbol.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let name = if label.chars().count() > 24 {
        format!("{}…", label.chars().take(23).collect::<String>())
    } else {
        label.clone()
    };
    Ok(Pattern {
        name,
        desc: format!("Chord progression: {label}"),
        length,
        data: PatternData::Melodic(steps),
        id: crate::persist::Id::nil(),
        cc: vec![Vec::new(); length],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roots_accidentals_and_qualities() {
        assert_eq!(parse_chord("C").unwrap().root_pc, 0);
        assert_eq!(parse_chord("Dm7").unwrap().root_pc, 2);
        assert_eq!(parse_chord("F#m7b5").unwrap().root_pc, 6);
        assert_eq!(parse_chord("Bb").unwrap().root_pc, 10);
        assert_eq!(parse_chord("Cmaj7").unwrap().intervals, vec![0, 4, 7, 11]);
        assert_eq!(parse_chord("Dm7").unwrap().intervals, vec![0, 3, 7, 10]);
        // 9th chords drop the 5th to stay within four voices.
        assert_eq!(parse_chord("Cm9").unwrap().intervals, vec![0, 3, 10, 14]);
        // Slash-bass is ignored.
        assert_eq!(parse_chord("C/G").unwrap().root_pc, 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_chord("H").is_err()); // no H note
        assert!(parse_chord("Cwtf").is_err()); // unknown quality
        assert!(parse_progression("   ").is_err()); // no chords
    }

    #[test]
    fn progression_splits_on_space_comma_pipe_not_hyphen() {
        let p = parse_progression("Dm7, G7 | Cmaj7").unwrap();
        assert_eq!(p.len(), 3);
        // A leading-hyphen minor symbol survives (not treated as a separator).
        let q = parse_progression("C-7 F-7").unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].intervals, vec![0, 3, 7, 10]);
    }

    #[test]
    fn every_voiced_chord_has_at_most_four_notes() {
        let chords = parse_progression("Cmaj9 Dm9 G9 Am7 Fmaj7 Bdim7").unwrap();
        for v in voice_progression(&chords, 60) {
            assert!(v.len() <= 4, "voicing {v:?} exceeds four voices");
            assert!(!v.is_empty());
        }
    }

    #[test]
    fn voice_leading_keeps_successive_chords_close() {
        // ii–V–I: total top-to-bottom motion between chords should be modest.
        let chords = parse_progression("Dm7 G7 Cmaj7").unwrap();
        let v = voice_progression(&chords, 60);
        let motion = |a: &[i32], b: &[i32]| -> i32 {
            b.iter()
                .map(|&n| a.iter().map(|&p| (n - p).abs()).min().unwrap())
                .sum()
        };
        // Each step moves less than a naive root-position jump would (~>12).
        assert!(
            motion(&v[0], &v[1]) <= 10,
            "ii→V voice-leading too wide: {v:?}"
        );
        assert!(
            motion(&v[1], &v[2]) <= 10,
            "V→I voice-leading too wide: {v:?}"
        );
    }

    #[test]
    fn builds_one_sustained_chord_per_bar() {
        // Root 48 (J-6 C3), 16 steps/bar, 4 chords → 64 steps, chords at 0/16/32/48.
        let pat = build_progression_pattern("Dm7 G7 Cmaj7 A7", 16, 48).unwrap();
        assert_eq!(pat.length, 64);
        let PatternData::Melodic(steps) = &pat.data else {
            panic!("melodic")
        };
        for (i, s) in steps.iter().enumerate() {
            if i % 16 == 0 {
                assert!(
                    s.len() >= 3,
                    "step {i} should hold a chord, got {}",
                    s.len()
                );
                assert!(s.len() <= 4, "step {i} exceeds four voices");
                assert!(
                    s.iter().all(|n| (n.len - 16.0).abs() < 1e-6),
                    "chord should sustain a full bar"
                );
            } else {
                assert!(s.is_empty(), "step {i} should rest between chords");
            }
        }
    }

    #[test]
    fn long_progression_stays_within_64_steps() {
        // 6 chords × 16 = 96 > 64 → bar length shrinks so all fit.
        let pat = build_progression_pattern("C Dm Em F G Am", 16, 48).unwrap();
        assert!(
            pat.length <= 64,
            "length {} must be capped at 64",
            pat.length
        );
        let PatternData::Melodic(steps) = &pat.data else {
            panic!("melodic")
        };
        let chord_steps = steps.iter().filter(|s| !s.is_empty()).count();
        assert_eq!(chord_steps, 6, "all six chords must be placed");
    }
}
