use crate::devices::profiles::DeviceProfile;

/// serde defaults for the per-step fields absent from the vendored library data.
fn default_prob() -> f32 { 1.0 }
fn default_ratchet() -> u8 { 1 }

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrumHit {
    pub note: u8,
    pub vel: u8, // 1..=127
    #[serde(default = "default_prob")]
    pub prob: f32, // 0..1 trigger probability, default 1.0
    #[serde(default = "default_ratchet")]
    pub ratchet: u8, // 1..8 intra-step retriggers, default 1
}

/// 0..N simultaneous hits on a single step (polyphonic).
pub type DrumStep = Vec<DrumHit>;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MelodicNote {
    pub semi: i8,    // offset from the lane's root note
    pub vel: f32,    // 0.5..=1.3 multiplier (not MIDI velocity)
    pub slide: bool, // 303/SH-101 legato tie
    pub len: f32,    // length in steps (authoring)
    #[serde(default = "default_prob")]
    pub prob: f32, // 0..1 trigger probability, default 1.0
    #[serde(default = "default_ratchet")]
    pub ratchet: u8, // 1..8 intra-step retriggers, default 1
}

/// Monophonic: a rest (None) or exactly one note.
pub type MelodicStep = Option<MelodicNote>;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PatternData {
    Drums(Vec<DrumStep>),
    Melodic(Vec<MelodicStep>),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pattern {
    pub name: String,
    pub length: usize, // 1..=64
    pub data: PatternData,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Drums,
    Melodic,
}

impl Pattern {
    /// An empty drum pattern named "init" with `length` empty steps.
    pub fn empty_drums(length: usize) -> Pattern {
        Pattern {
            name: "init".to_string(),
            length,
            data: PatternData::Drums(vec![Vec::new(); length]),
        }
    }

    /// An empty melodic pattern named "init" with `length` rests.
    pub fn empty_melodic(length: usize) -> Pattern {
        Pattern {
            name: "init".to_string(),
            length,
            data: PatternData::Melodic(vec![None; length]),
        }
    }

    pub fn kind(&self) -> LaneKind {
        match &self.data {
            PatternData::Drums(_) => LaneKind::Drums,
            PatternData::Melodic(_) => LaneKind::Melodic,
        }
    }

    pub fn step_count(&self) -> usize {
        self.length
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub profile: DeviceProfile,
    pub pattern: Pattern,
    pub mute: bool,
    pub solo: bool,
    pub transpose: i8, // semitones (melodic)
    pub octave: i8,    // octaves (melodic)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub name: String,
    pub bpm: f64,
    pub swing: f32,
    pub lanes: Vec<Lane>,
}

impl Set {
    /// A fresh set: bpm 120, swing 0.5, one empty 16-step lane per profile.
    /// Drum-kind profiles get an empty drum pattern; melodic-kind get empty melodic.
    pub fn default_set(profiles: [DeviceProfile; 3]) -> Set {
        let lanes = profiles
            .iter()
            .map(|&profile| {
                let pattern = match profile.kind {
                    LaneKind::Drums => Pattern::empty_drums(16),
                    LaneKind::Melodic => Pattern::empty_melodic(16),
                };
                Lane {
                    profile,
                    pattern,
                    mute: false,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                }
            })
            .collect();
        Set {
            name: "init".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_drums_has_length_empty_steps_and_drum_kind() {
        let p = Pattern::empty_drums(16);
        assert_eq!(p.name, "init");
        assert_eq!(p.length, 16);
        assert_eq!(p.step_count(), 16);
        assert_eq!(p.kind(), LaneKind::Drums);
        match &p.data {
            PatternData::Drums(steps) => {
                assert_eq!(steps.len(), 16);
                assert!(steps.iter().all(|s| s.is_empty()));
            }
            _ => panic!("expected drums"),
        }
    }

    #[test]
    fn empty_melodic_has_length_rests_and_melodic_kind() {
        let p = Pattern::empty_melodic(16);
        assert_eq!(p.name, "init");
        assert_eq!(p.length, 16);
        assert_eq!(p.step_count(), 16);
        assert_eq!(p.kind(), LaneKind::Melodic);
        match &p.data {
            PatternData::Melodic(steps) => {
                assert_eq!(steps.len(), 16);
                assert!(steps.iter().all(|s| s.is_none()));
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn drum_pattern_serde_round_trips() {
        let p = Pattern {
            name: "techno #03".to_string(),
            length: 2,
            data: PatternData::Drums(vec![
                vec![DrumHit { note: 36, vel: 120, prob: 1.0, ratchet: 1 }, DrumHit { note: 42, vel: 100, prob: 1.0, ratchet: 1 }],
                vec![],
            ]),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn melodic_pattern_serde_round_trips() {
        let p = Pattern {
            name: "acid #11".to_string(),
            length: 2,
            data: PatternData::Melodic(vec![
                Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 0.5, prob: 1.0, ratchet: 1 }),
                None,
            ]),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn missing_prob_ratchet_deserialize_to_defaults() {
        // Vendored data has no prob/ratchet fields; serde defaults fill them in.
        let hit: DrumHit = serde_json::from_str(r#"{"note":36,"vel":100}"#).unwrap();
        assert_eq!(hit.prob, 1.0);
        assert_eq!(hit.ratchet, 1);
        let note: MelodicNote =
            serde_json::from_str(r#"{"semi":0,"vel":1.0,"slide":false,"len":0.5}"#).unwrap();
        assert_eq!(note.prob, 1.0);
        assert_eq!(note.ratchet, 1);
    }

    #[test]
    fn default_set_has_three_init_lanes_bpm120_swing_half() {
        let profiles = crate::devices::profiles::default_profiles();
        let set = Set::default_set(profiles);
        assert_eq!(set.bpm, 120.0);
        assert_eq!(set.swing, 0.5);
        assert_eq!(set.lanes.len(), 3);
        // Drums lane is drum-kind; the two melodic lanes are melodic-kind.
        assert_eq!(set.lanes[0].pattern.kind(), LaneKind::Drums);
        assert_eq!(set.lanes[1].pattern.kind(), LaneKind::Melodic);
        assert_eq!(set.lanes[2].pattern.kind(), LaneKind::Melodic);
        assert!(set.lanes.iter().all(|l| !l.mute && !l.solo));
    }
}
