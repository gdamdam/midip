use crate::devices::profiles::DeviceProfile;
use crate::persist;

/// serde defaults for the per-step fields absent from the vendored library data.
fn default_prob() -> f32 {
    1.0
}
fn default_ratchet() -> u8 {
    1
}

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
    #[serde(default)]
    pub desc: String,
    pub length: usize, // 1..=64
    pub data: PatternData,
    #[serde(default)]
    pub id: persist::Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Drums,
    Melodic,
}

/// A stable reference to a MIDI output port, by key and human-readable name.
/// Matching at runtime uses `stable_key` first, falls back to `name`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortRef {
    pub stable_key: String,
    pub name: String,
}

/// Per-lane MIDI routing: which port, channel, and whether to send MIDI Clock.
/// `None` on `Lane::route` means "derive from profile" via `Lane::effective_route`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LaneRoute {
    pub port: PortRef,
    pub channel: u8,
    pub clock_out: bool,
}

impl Pattern {
    /// An empty drum pattern named "init" with `length` empty steps.
    pub fn empty_drums(length: usize) -> Pattern {
        Pattern {
            name: "init".to_string(),
            desc: String::new(),
            length,
            data: PatternData::Drums(vec![Vec::new(); length]),
            id: persist::Id::nil(),
        }
    }

    /// An empty melodic pattern named "init" with `length` rests.
    pub fn empty_melodic(length: usize) -> Pattern {
        Pattern {
            name: "init".to_string(),
            desc: String::new(),
            length,
            data: PatternData::Melodic(vec![None; length]),
            id: persist::Id::nil(),
        }
    }

    /// Set `id` to a fresh minted id only when it is currently nil.
    pub fn ensure_id(&mut self) {
        if self.id.is_nil() {
            self.id = persist::mint_id();
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
    /// Explicit routing override. `None` = derive from profile via `effective_route()`.
    pub route: Option<LaneRoute>,
}

impl Lane {
    /// The MIDI route to use for this lane.
    /// Returns the explicit `route` if set; otherwise derives from the device profile.
    pub fn effective_route(&self) -> LaneRoute {
        if let Some(r) = &self.route {
            return r.clone();
        }
        LaneRoute {
            port: PortRef {
                stable_key: self.profile.port_match.to_string(),
                name: self.profile.port_match.to_string(),
            },
            channel: self.profile.channel,
            clock_out: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub name: String,
    pub bpm: f64,
    pub swing: f32,
    pub lanes: Vec<Lane>,
    pub id: persist::Id,
}

impl Set {
    /// Set `id` to a fresh minted id only when it is currently nil.
    pub fn ensure_id(&mut self) {
        if self.id.is_nil() {
            self.id = persist::mint_id();
        }
    }

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
                    route: None,
                }
            })
            .collect();
        Set {
            name: "init".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
            id: persist::Id::nil(),
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
    fn ensure_id_assigns_only_when_nil() {
        // nil → assigned non-nil
        let mut p = Pattern::empty_drums(4);
        assert!(p.id.is_nil());
        p.ensure_id();
        assert!(!p.id.is_nil());

        // preset → unchanged
        let preset = crate::persist::Id::generate(0xABCD, 42);
        let mut p2 = Pattern::empty_drums(4);
        p2.id = preset.clone();
        p2.ensure_id();
        assert_eq!(p2.id, preset);

        // same for Set
        let profiles = crate::devices::profiles::default_profiles();
        let mut s = Set::default_set(profiles);
        assert!(s.id.is_nil());
        s.ensure_id();
        assert!(!s.id.is_nil());

        let preset_set = crate::persist::Id::generate(0x1234, 99);
        let profiles2 = crate::devices::profiles::default_profiles();
        let mut s2 = Set::default_set(profiles2);
        s2.id = preset_set.clone();
        s2.ensure_id();
        assert_eq!(s2.id, preset_set);
    }

    #[test]
    fn drum_pattern_serde_round_trips() {
        let p = Pattern {
            name: "techno #03".to_string(),
            desc: "a cool pattern".to_string(),
            length: 2,
            data: PatternData::Drums(vec![
                vec![
                    DrumHit {
                        note: 36,
                        vel: 120,
                        prob: 1.0,
                        ratchet: 1,
                    },
                    DrumHit {
                        note: 42,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                    },
                ],
                vec![],
            ]),
            id: crate::persist::Id::nil(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn melodic_pattern_serde_round_trips() {
        let p = Pattern {
            name: "acid #11".to_string(),
            desc: "another pattern".to_string(),
            length: 2,
            data: PatternData::Melodic(vec![
                Some(MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 0.5,
                    prob: 1.0,
                    ratchet: 1,
                }),
                None,
            ]),
            id: crate::persist::Id::nil(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn pattern_without_desc_deserializes_desc_to_empty() {
        // Old saved sets won't have the "desc" field; serde(default) fills it as "".
        let json = r#"{"name":"old #01","length":1,"data":{"Drums":[[]]}}"#;
        let p: Pattern = serde_json::from_str(json).unwrap();
        assert_eq!(p.name, "old #01");
        assert_eq!(p.desc, "");
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

    // ── Task 5: LaneRoute / effective_route ──────────────────────────────────

    #[test]
    fn effective_route_defaults_from_profile_when_none() {
        let profiles = crate::devices::profiles::default_profiles();
        let lane = Lane {
            profile: profiles[0], // T8_DRUMS: port_match="T-8", channel=9
            pattern: Pattern::empty_drums(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
        };
        let r = lane.effective_route();
        assert_eq!(r.channel, profiles[0].channel);
        assert_eq!(r.port.stable_key, profiles[0].port_match);
        assert_eq!(r.port.name, profiles[0].port_match);
        assert!(r.clock_out, "default clock_out must be true");
    }

    #[test]
    fn explicit_route_overrides_profile() {
        let profiles = crate::devices::profiles::default_profiles();
        let explicit = LaneRoute {
            port: PortRef {
                stable_key: "X".to_string(),
                name: "X".to_string(),
            },
            channel: 5,
            clock_out: false,
        };
        let lane = Lane {
            profile: profiles[0],
            pattern: Pattern::empty_drums(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: Some(explicit.clone()),
        };
        let r = lane.effective_route();
        assert_eq!(r, explicit);
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
