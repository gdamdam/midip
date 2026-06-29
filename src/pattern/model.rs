use crate::devices::profiles::DeviceProfile;
use crate::pattern::refs::PatternRef;
use crate::persist;

/// serde defaults for the per-step fields absent from the vendored library data.
fn default_prob() -> f32 {
    1.0
}
fn default_ratchet() -> u8 {
    1
}

/// A CC value locked to a single step. `cc` is the MIDI CC number (0–127), `val` the value (0–127).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CcLock {
    pub cc: u8,
    pub val: u8,
}

/// Trigger condition controlling whether a step fires on a given playback cycle.
/// Default is `Always` (unconditional). Serde uses the externally-tagged representation
/// so every variant round-trips cleanly.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum TrigCond {
    #[default]
    Always,
    /// Fire on cycle `x` of every `y` cycles (1-indexed, so x=1,y=2 = every other bar).
    Ratio {
        x: u8,
        y: u8,
    },
    Fill,
    NotFill,
    First,
    NotFirst,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrumHit {
    pub note: u8,
    pub vel: u8, // 1..=127
    #[serde(default = "default_prob")]
    pub prob: f32, // 0..1 trigger probability, default 1.0
    #[serde(default = "default_ratchet")]
    pub ratchet: u8, // 1..8 intra-step retriggers, default 1
    /// Microtiming offset in ticks (-128..=127); positive = later, negative = earlier.
    #[serde(default)]
    pub micro: i16,
    /// Trigger condition for this hit.
    #[serde(default)]
    pub cond: TrigCond,
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
    /// Microtiming offset in ticks (-128..=127); positive = later, negative = earlier.
    #[serde(default)]
    pub micro: i16,
    /// Trigger condition for this note.
    #[serde(default)]
    pub cond: TrigCond,
}

/// A single melodic step: zero notes (rest), one note (mono — today's behavior), or
/// many notes (a chord — used by poly lanes in later M5b tasks). Stored as a `Vec`.
///
/// Backward-compat (M5b Task 1): old data serialized each step as `null` (rest) or a
/// single `MelodicNote` object; the new format is a JSON array. A custom `Deserialize`
/// accepts ALL THREE shapes — `null` → `[]`, a single object → `[note]`, an array →
/// as-is — so every existing pattern (vendored library + user/set JSON) loads unchanged.
/// `Serialize` is adaptive for backward compatibility: a rest emits `null`, a single
/// note emits a bare object (the legacy mono shape), and a chord (2+ notes) emits an
/// array. So mono patterns written by this version still load in pre-chord builds; only
/// chord-containing patterns become this-version-only. The shim lives here, in one place,
/// so it applies everywhere serde reads a step. A `Deref`/`DerefMut` to `Vec<MelodicNote>`
/// keeps call sites ergonomic (they use plain `Vec` methods); the wrapper only intercepts
/// (de)serde.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MelodicStep(pub Vec<MelodicNote>);

impl std::ops::Deref for MelodicStep {
    type Target = Vec<MelodicNote>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for MelodicStep {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<Vec<MelodicNote>> for MelodicStep {
    fn from(v: Vec<MelodicNote>) -> Self {
        MelodicStep(v)
    }
}

impl serde::Serialize for MelodicStep {
    /// Adaptive on-disk shape for backward compatibility: a rest serializes as `null`,
    /// a single note as a bare object (the legacy mono shape), and a chord (2+ notes) as
    /// an array. The `Deserialize` shim above accepts all three, so this round-trips while
    /// keeping mono patterns readable by pre-chord builds.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.as_slice() {
            [] => serializer.serialize_none(),
            [one] => one.serialize(serializer),
            many => many.serialize(serializer),
        }
    }
}

impl<'de> serde::Deserialize<'de> for MelodicStep {
    /// Accept the OLD shapes (`null` → rest, single object → one note) and the NEW
    /// shape (array). An untagged helper enum dispatches on the JSON value.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Shim {
            // `null` maps here: `Option<()>` deserializes JSON null to `None`. The payload
            // is intentionally ignored (we only need to know the step is a rest), hence the
            // `dead_code` allow on the unread field.
            Rest(#[allow(dead_code)] Option<()>),
            // A bare array of notes (new format). Must precede `One` so `[]`/`[..]`
            // is not misread as a single (struct) note.
            Many(Vec<MelodicNote>),
            // A single note object (old mono format).
            One(MelodicNote),
        }
        Ok(match Shim::deserialize(deserializer)? {
            Shim::Rest(_) => MelodicStep(Vec::new()),
            Shim::Many(v) => MelodicStep(v),
            Shim::One(n) => MelodicStep(vec![n]),
        })
    }
}

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
    /// Per-step CC locks. One slot per step, kept length-synced with `length`.
    /// Each slot holds 0..N CC locks that fire when that step triggers.
    #[serde(default)]
    pub cc: Vec<Vec<CcLock>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Drums,
    Melodic,
}

/// Reserved `stable_key` denoting midip's own engine-managed virtual MIDI source.
/// A lane whose route targets this key delivers to the virtual "midip" port instead of a
/// watcher-connected hardware port. The leading `@` cannot collide with a real device name
/// (CoreMIDI/midir port names never begin with it in practice) so the dedup-by-key logic
/// treats it as a distinct, always-present destination.
pub const VIRTUAL_PORT_KEY: &str = "@midip";
/// Human-readable display name of the virtual port (what other apps see as the MIDI source).
pub const VIRTUAL_PORT_NAME: &str = "midip";

/// A stable reference to a MIDI output port, by key and human-readable name.
/// Matching at runtime uses `stable_key` first, falls back to `name`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortRef {
    pub stable_key: String,
    pub name: String,
}

impl PortRef {
    /// The synthetic `PortRef` for midip's own virtual "midip" source.
    pub fn virtual_midip() -> PortRef {
        PortRef {
            stable_key: VIRTUAL_PORT_KEY.to_string(),
            name: VIRTUAL_PORT_NAME.to_string(),
        }
    }

    /// True when this `PortRef` denotes the engine-managed virtual "midip" port.
    pub fn is_virtual(&self) -> bool {
        self.stable_key == VIRTUAL_PORT_KEY
    }
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
            cc: vec![Vec::new(); length],
        }
    }

    /// An empty melodic pattern named "init" with `length` rests.
    pub fn empty_melodic(length: usize) -> Pattern {
        Pattern {
            name: "init".to_string(),
            desc: String::new(),
            length,
            data: PatternData::Melodic(vec![MelodicStep::default(); length]),
            id: persist::Id::nil(),
            cc: vec![Vec::new(); length],
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

    /// Returns the CC locks for `step`. Returns `&[]` when `step` is out of bounds
    /// (e.g. a deserialized pattern whose `cc` vec was not length-synced).
    pub fn step_cc(&self, step: usize) -> &[CcLock] {
        // Gracefully handle deserialized patterns whose `cc` vec wasn't length-synced
        // (e.g. old JSON that had no `cc` field → serde default gives empty Vec).
        self.cc.get(step).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Replace the CC locks for `step`.
    pub fn set_step_cc(&mut self, step: usize, locks: Vec<CcLock>) {
        // Grow if needed (handles old-JSON deserialized patterns with empty cc vec).
        if self.cc.len() < self.length {
            self.cc.resize(self.length, Vec::new());
        }
        if step < self.cc.len() {
            self.cc[step] = locks;
        }
    }

    /// Sync `cc` length to `new_len` after a length change, preserving existing locks.
    /// Call this whenever `self.length` is changed.
    pub fn sync_cc_len(&mut self, new_len: usize) {
        self.cc.resize(new_len, Vec::new());
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
    /// Per-voice mute: MIDI notes whose playback is silenced (non-destructive, latched).
    /// Persisted via LaneDto; old files without this field get an empty vec on load.
    pub muted_voices: Vec<u8>,
    /// The scale applied to this lane when folding/stepping melodic notes.
    /// Defaults to `Scale::Chromatic` (identity) so old sets are unchanged.
    pub scale: crate::music::scale::Scale,
    /// Per-lane root note override (MIDI 0–127). `None` → use `profile.root_note`.
    pub root: Option<u8>,
    /// Per-lane swing amount override (0.0..=1.0). `None` → use global set swing.
    pub swing: Option<f32>,
    /// Per-lane clock divisor override. `None` → use default (1 step = 1 tick).
    pub clock_div: Option<u8>,
}

impl Lane {
    /// Returns `true` when `note` is in the per-voice mute list (silenced).
    pub fn is_voice_muted(&self, note: u8) -> bool {
        self.muted_voices.contains(&note)
    }

    /// The effective root note for this lane: the per-lane override when set,
    /// else the device profile's `root_note`.
    pub fn effective_root(&self) -> u8 {
        self.root.unwrap_or(self.profile.root_note)
    }

    /// The MIDI channel this lane emits on: the explicit route's channel when set,
    /// else the profile channel. Allocation-free — safe for the scheduler hot path
    /// (unlike `effective_route()`, which clones the port's `String`s).
    pub fn route_channel(&self) -> u8 {
        self.route
            .as_ref()
            .map(|r| r.channel)
            .unwrap_or(self.profile.channel)
    }

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

/// A snapshot of a single lane's performance state for a scene.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LaneAssignment {
    pub pattern: PatternRef,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub transpose: i8,
    #[serde(default)]
    pub octave: i8,
}

/// A named snapshot of per-lane performance state (pattern + mute/solo/transpose/octave).
/// Stored inside the `Set`; `#[serde(default)]` on `Set::scenes` so old sets load with `[]`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scene {
    #[serde(default)]
    pub id: persist::Id,
    pub name: String,
    /// One `LaneAssignment` per lane, in lane order.
    pub assignments: Vec<LaneAssignment>,
}

impl Scene {
    /// Capture the current performance state of every lane in `set` into a new `Scene`.
    ///
    /// Each lane's assignment is built from the lane's current pattern id (as a
    /// `PatternRef::User`), mute, solo, transpose, and octave. Mints a fresh id.
    pub fn from_set(set: &Set, name: String) -> Scene {
        let assignments = set
            .lanes
            .iter()
            .map(|lane| LaneAssignment {
                // PatternRef::User is always correct here: Set lanes hold inline user
                // patterns (identified by id), never vendored library patterns. There is
                // no vendored-lane capture path, so a Vendored ref can never appear in
                // a live lane's pattern field.
                pattern: PatternRef::User(lane.pattern.id.clone()),
                mute: lane.mute,
                solo: lane.solo,
                transpose: lane.transpose,
                octave: lane.octave,
            })
            .collect();
        Scene {
            id: persist::mint_id(),
            name,
            assignments,
        }
    }
}

// ---------------------------------------------------------------------------
// Chain / ChainEntry — song-mode sequencing (M7)
// ---------------------------------------------------------------------------

/// One step in a chain: play `scene_id` for `repeats` passes of `bars` bars.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChainEntry {
    pub scene_id: persist::Id,
    pub repeats: u32,
    pub bars: u32,
}

impl ChainEntry {
    /// Total steps this entry occupies (4/4, 16 steps/bar).
    pub fn dwell_steps(&self) -> u64 {
        self.bars as u64 * self.repeats as u64 * 16
    }
}

/// An ordered sequence of scene references that plays as a song.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Chain {
    pub id: persist::Id,
    pub name: String,
    #[serde(default)]
    pub entries: Vec<ChainEntry>,
    /// Whether the chain loops back to the start after the last entry.
    #[serde(rename = "loop", default)]
    pub looped: bool,
}

impl Chain {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: persist::mint_id(),
            name: name.into(),
            entries: Vec::new(),
            looped: false,
        }
    }

    /// Sum of all entry dwell steps.
    pub fn total_steps(&self) -> u64 {
        self.entries.iter().map(|e| e.dwell_steps()).sum()
    }
}

#[cfg(test)]
mod chain_model_tests {
    use super::*;

    #[test]
    fn dwell_steps_is_bars_times_repeats_times_16() {
        let e = ChainEntry {
            scene_id: persist::mint_id(),
            repeats: 2,
            bars: 4,
        };
        assert_eq!(e.dwell_steps(), 2 * 4 * 16); // 128 steps
    }

    #[test]
    fn chain_serde_roundtrip_and_loop_rename() {
        let mut c = Chain::new("verse->chorus");
        c.looped = true;
        c.entries.push(ChainEntry {
            scene_id: persist::mint_id(),
            repeats: 1,
            bars: 8,
        });
        let json = serde_json::to_string(&c).unwrap();
        assert!(
            json.contains("\"loop\""),
            "field must serialize as `loop`, got: {json}"
        );
        assert!(!json.contains("looped"));
        let back: Chain = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, c.name);
        assert!(back.looped);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].bars, 8);
    }

    #[test]
    fn total_steps_sums_entry_dwells() {
        let mut c = Chain::new("x");
        c.entries.push(ChainEntry {
            scene_id: persist::mint_id(),
            repeats: 1,
            bars: 2,
        }); // 32
        c.entries.push(ChainEntry {
            scene_id: persist::mint_id(),
            repeats: 3,
            bars: 1,
        }); // 48
        assert_eq!(c.total_steps(), 80);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub name: String,
    pub bpm: f64,
    pub swing: f32,
    pub lanes: Vec<Lane>,
    pub id: persist::Id,
    /// Named scenes stored inside this set. Defaults to empty so old set files load unchanged.
    pub scenes: Vec<Scene>,
    /// Song-mode chains (M7). Defaults to empty so old set files load unchanged.
    pub chains: Vec<Chain>,
    /// MIDI input port to receive clock from (M10). `None` = no clock-in selected.
    /// Defaults to `None` so old set files (pre-v4) load unchanged.
    pub clock_in_port: Option<PortRef>,
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
                    muted_voices: Vec::new(),
                    scale: crate::music::scale::Scale::Chromatic,
                    root: None,
                    swing: None,
                    clock_div: None,
                }
            })
            .collect();
        Set {
            name: "init".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
            id: persist::Id::nil(),
            scenes: Vec::new(),
            chains: Vec::new(),
            clock_in_port: None,
        }
    }

    /// Convenience alias: capture the current lane state as a new scene.
    pub fn capture_scene(&self, name: String) -> Scene {
        Scene::from_set(self, name)
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
                assert!(steps.iter().all(|s| s.is_empty()));
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
                        micro: 0,
                        cond: TrigCond::Always,
                    },
                    DrumHit {
                        note: 42,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                        micro: 0,
                        cond: TrigCond::Always,
                    },
                ],
                vec![],
            ]),
            id: crate::persist::Id::nil(),
            cc: vec![Vec::new(); 2],
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
                MelodicStep::from(vec![MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 0.5,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }]),
                MelodicStep::default(),
            ]),
            id: crate::persist::Id::nil(),
            cc: vec![Vec::new(); 2],
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    // ── M5b Task 1: MelodicStep -> Vec<MelodicNote> backward-compat shim ─────

    #[test]
    fn old_melodic_json_with_null_and_object_steps_loads() {
        // OLD per-step shape: a step is `null` (rest) or a single MelodicNote object.
        // The deserialize shim must map null -> empty step ([]) and object -> [note].
        let json = r#"{
            "name":"old-mono #01",
            "length":2,
            "data":{"Melodic":[null,{"semi":0,"vel":1.0,"slide":false,"len":0.5}]}
        }"#;
        let p: Pattern = serde_json::from_str(json).unwrap();
        match &p.data {
            PatternData::Melodic(steps) => {
                assert_eq!(steps.len(), 2);
                // null -> empty (rest)
                assert!(steps[0].is_empty(), "null step must map to an empty step");
                // object -> one-note step
                assert_eq!(steps[1].len(), 1, "object step must map to a one-note step");
                assert_eq!(steps[1][0].semi, 0);
                assert_eq!(steps[1][0].vel, 1.0);
                // missing prob/ratchet default in via MelodicNote serde defaults.
                assert_eq!(steps[1][0].prob, 1.0);
                assert_eq!(steps[1][0].ratchet, 1);
            }
            _ => panic!("expected melodic"),
        }
    }

    #[test]
    fn vendored_library_still_loads() {
        // The real vendored library uses the OLD object/null per-step shape. It must
        // load through the real loader unchanged, and a known melodic pattern (bass)
        // must carry its expected notes (mono: one note per active step).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns");
        let lib = crate::pattern::library::Library::load(&dir).expect("vendored library loads");
        let bass = lib.bass.values().next().expect("bass genre present");
        let pat = bass.first().expect("a bass pattern present");
        match &pat.data {
            PatternData::Melodic(steps) => {
                // First step in the vendored bass file is an object -> one-note step.
                assert_eq!(steps[0].len(), 1, "vendored object step -> one-note step");
                // The vendored file mixes object steps (notes) and null steps (rests):
                // notes map to one-note steps, nulls to empty steps, and (being old mono
                // data) NO step holds more than one note.
                assert!(
                    steps.iter().any(|s| s.is_empty()),
                    "vendored null steps must map to empty (rest) steps"
                );
                assert!(
                    steps.iter().any(|s| s.len() == 1),
                    "vendored object steps must map to one-note steps"
                );
                assert!(
                    steps.iter().all(|s| s.len() <= 1),
                    "vendored mono data must never produce multi-note steps"
                );
            }
            _ => panic!("expected melodic bass pattern"),
        }
    }

    #[test]
    fn melodic_step_serde_roundtrips_empty_one_and_many() {
        let n1 = MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        };
        let n2 = MelodicNote {
            semi: 7,
            vel: 1.1,
            slide: true,
            len: 1.0,
            prob: 0.8,
            ratchet: 2,
            micro: 0,
            cond: TrigCond::Always,
        };
        // Adaptive on-disk shape: rest -> null, one note -> object, chord -> array.
        let rest_json = serde_json::to_string(&MelodicStep::default()).unwrap();
        assert_eq!(
            rest_json, "null",
            "a rest serializes as null, got {rest_json}"
        );

        let one_json = serde_json::to_string(&MelodicStep::from(vec![n1.clone()])).unwrap();
        assert!(
            one_json.starts_with('{'),
            "a single note serializes as an object, got {one_json}"
        );

        let many_json =
            serde_json::to_string(&MelodicStep::from(vec![n1.clone(), n2.clone()])).unwrap();
        assert!(
            many_json.starts_with('['),
            "a chord serializes as an array, got {many_json}"
        );

        // All three shapes round-trip losslessly through the shim.
        for step in [
            MelodicStep::default(),                  // null  rest
            MelodicStep::from(vec![n1.clone()]),     // {..}  mono
            MelodicStep::from(vec![n1.clone(), n2]), // [..]  chord
        ] {
            let json = serde_json::to_string(&step).unwrap();
            let back: MelodicStep = serde_json::from_str(&json).unwrap();
            assert_eq!(step, back, "round-trip must be lossless for {json}");
        }
    }

    #[test]
    fn melodic_step_deserialize_accepts_null_object_and_array() {
        // null -> empty
        let rest: MelodicStep = serde_json::from_str("null").unwrap();
        assert!(rest.is_empty());
        // single object -> one note
        let one: MelodicStep =
            serde_json::from_str(r#"{"semi":3,"vel":1.0,"slide":false,"len":0.5}"#).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].semi, 3);
        // array -> as-is
        let many: MelodicStep = serde_json::from_str(
            r#"[{"semi":0,"vel":1.0,"slide":false,"len":0.5},{"semi":4,"vel":1.0,"slide":false,"len":0.5}]"#,
        )
        .unwrap();
        assert_eq!(many.len(), 2);
        assert_eq!(many[1].semi, 4);
    }

    #[test]
    fn mono_pattern_reserializes_to_legacy_shape() {
        // The backward-compat guarantee of adaptive serialize: a pattern containing only
        // rests and single notes must re-serialize with NO array-shaped steps (rests ->
        // null, notes -> objects), so a pre-chord build can still read what we wrote.
        let json = r#"{"name":"m #01","length":2,"data":{"Melodic":[null,{"semi":0,"vel":1.0,"slide":false,"len":0.5}]}}"#;
        let p: Pattern = serde_json::from_str(json).unwrap();
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        let steps = v["data"]["Melodic"].as_array().unwrap();
        assert!(
            steps[0].is_null(),
            "rest must re-serialize as null, got {}",
            steps[0]
        );
        assert!(
            steps[1].is_object(),
            "a single note must re-serialize as a bare object (legacy shape), got {}",
            steps[1]
        );
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
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
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
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        let r = lane.effective_route();
        assert_eq!(r, explicit);
    }

    #[test]
    fn route_channel_uses_route_when_set_else_profile() {
        let profiles = crate::devices::profiles::default_profiles();
        // No route → profile channel.
        let lane = Lane {
            profile: profiles[0], // channel 9
            pattern: Pattern::empty_drums(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        assert_eq!(lane.route_channel(), profiles[0].channel);

        // Explicit route → route channel, overriding the profile.
        let mut lane2 = lane.clone();
        lane2.route = Some(LaneRoute {
            port: PortRef {
                stable_key: "X".to_string(),
                name: "X".to_string(),
            },
            channel: 5,
            clock_out: true,
        });
        assert_eq!(lane2.route_channel(), 5);
        assert_ne!(lane2.route_channel(), profiles[0].channel);
    }

    #[test]
    fn virtual_midip_port_ref_uses_reserved_key_and_name() {
        let p = PortRef::virtual_midip();
        assert_eq!(p.stable_key, VIRTUAL_PORT_KEY);
        assert_eq!(p.name, VIRTUAL_PORT_NAME);
        assert!(
            p.is_virtual(),
            "virtual_midip() must be recognized as virtual"
        );
        // A real hardware port ref is NOT virtual.
        let hw = PortRef {
            stable_key: "Roland T-8".into(),
            name: "Roland T-8".into(),
        };
        assert!(!hw.is_virtual());
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

    // ── M6 Task 1: Scene + LaneAssignment ───────────────────────────────────

    #[test]
    fn capture_scene_snapshots_current_lane_state() {
        use crate::pattern::refs::PatternRef;
        let profiles = crate::devices::profiles::default_profiles();
        let mut set = Set::default_set(profiles);
        // Give each lane a non-nil pattern id so capture can build a User ref.
        for lane in &mut set.lanes {
            lane.pattern.ensure_id();
        }
        set.lanes[0].mute = true;
        set.lanes[1].solo = true;
        set.lanes[1].transpose = 5;
        set.lanes[2].octave = -1;

        let scene = set.capture_scene("Live".to_string());
        assert_eq!(scene.name, "Live");
        assert!(!scene.id.is_nil(), "capture_scene must mint a non-nil id");
        assert_eq!(scene.assignments.len(), set.lanes.len());

        // Each assignment mirrors the lane's current state.
        assert_eq!(
            scene.assignments[0].pattern,
            PatternRef::User(set.lanes[0].pattern.id.clone())
        );
        assert!(scene.assignments[0].mute);
        assert!(!scene.assignments[0].solo);
        assert_eq!(scene.assignments[0].transpose, 0);
        assert_eq!(scene.assignments[0].octave, 0);

        assert_eq!(
            scene.assignments[1].pattern,
            PatternRef::User(set.lanes[1].pattern.id.clone())
        );
        assert!(!scene.assignments[1].mute);
        assert!(scene.assignments[1].solo);
        assert_eq!(scene.assignments[1].transpose, 5);
        assert_eq!(scene.assignments[1].octave, 0);

        assert_eq!(scene.assignments[2].octave, -1);
    }

    #[test]
    fn scene_resolve_reports_missing_pattern() {
        use crate::pattern::library::Library;
        use crate::pattern::refs::{resolve_scene, PatternRef};
        let profiles = crate::devices::profiles::default_profiles();
        let mut set = Set::default_set(profiles);
        for lane in &mut set.lanes {
            lane.pattern.ensure_id();
        }

        // Build a scene with one resolvable vendored ref and one nonexistent user id.
        let missing_id = crate::persist::mint_id();
        let scene = Scene {
            id: crate::persist::mint_id(),
            name: "test".to_string(),
            assignments: vec![
                LaneAssignment {
                    pattern: PatternRef::User(missing_id),
                    mute: false,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                },
                LaneAssignment {
                    // Use the lane's actual id — resolvable from the set's inline pattern.
                    pattern: PatternRef::User(set.lanes[1].pattern.id.clone()),
                    mute: false,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                },
            ],
        };

        let lib = Library::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/patterns"),
        )
        .expect("library loads");
        // Provide the set's inline patterns as the user-pattern search space.
        let inline: Vec<Pattern> = set.lanes.iter().map(|l| l.pattern.clone()).collect();
        let results = resolve_scene(&scene, &lib, &inline);

        assert_eq!(results.len(), 2);
        assert!(results[0].is_err(), "missing user id must resolve to Err");
        assert!(results[1].is_ok(), "known lane pattern must resolve to Ok");
    }

    // ── M5a Task 2: per-lane scale + root ────────────────────────────────────

    #[test]
    fn lane_defaults_chromatic_and_profile_root() {
        let profiles = crate::devices::profiles::default_profiles();
        // Use the bass profile (profiles[1]) which has root_note=45.
        let lane = Lane {
            profile: profiles[1],
            pattern: Pattern::empty_melodic(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        assert_eq!(lane.scale, crate::music::scale::Scale::Chromatic);
        assert_eq!(lane.root, None);
        assert_eq!(
            lane.effective_root(),
            profiles[1].root_note,
            "effective_root must fall back to profile.root_note when root is None"
        );
    }

    #[test]
    fn effective_root_uses_override_when_set() {
        let profiles = crate::devices::profiles::default_profiles();
        let lane = Lane {
            profile: profiles[1], // root_note=45
            pattern: Pattern::empty_melodic(4),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Major,
            root: Some(50),
            swing: None,
            clock_div: None,
        };
        assert_eq!(
            lane.effective_root(),
            50,
            "effective_root must return the override when root is Some"
        );
    }

    // ── M8 Task 1: CcLock / TrigCond / micro+cond / cc store / lane swing+div ─

    const OLD_DRUM_HIT_JSON: &str = r#"{"note":36,"vel":100,"prob":1.0,"ratchet":1}"#;
    const OLD_MELODIC_NOTE_JSON: &str =
        r#"{"semi":0,"vel":1.0,"slide":false,"len":1.0,"prob":1.0,"ratchet":1}"#;

    fn sample_drum_hit() -> DrumHit {
        DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }
    }

    fn sample_melodic_note() -> MelodicNote {
        MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }
    }

    fn sample_pattern(length: usize) -> Pattern {
        Pattern::empty_drums(length)
    }

    fn sample_lane() -> Lane {
        let profiles = crate::devices::profiles::default_profiles();
        Lane {
            profile: profiles[0],
            pattern: Pattern::empty_drums(16),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn trigcond_default_is_always() {
        assert_eq!(TrigCond::default(), TrigCond::Always);
    }

    #[test]
    fn drumhit_micro_cond_roundtrip() {
        let h = DrumHit {
            micro: -120,
            cond: TrigCond::Ratio { x: 1, y: 4 },
            ..sample_drum_hit()
        };
        let j = serde_json::to_string(&h).unwrap();
        let b: DrumHit = serde_json::from_str(&j).unwrap();
        assert_eq!(b.micro, -120);
        assert_eq!(b.cond, TrigCond::Ratio { x: 1, y: 4 });
    }

    #[test]
    fn melodicnote_micro_cond_roundtrip() {
        let n = MelodicNote {
            micro: 50,
            cond: TrigCond::Fill,
            ..sample_melodic_note()
        };
        let j = serde_json::to_string(&n).unwrap();
        let b: MelodicNote = serde_json::from_str(&j).unwrap();
        assert_eq!(b.micro, 50);
        assert_eq!(b.cond, TrigCond::Fill);
    }

    #[test]
    fn old_hit_json_without_m8_fields_defaults() {
        let b: DrumHit = serde_json::from_str(OLD_DRUM_HIT_JSON).unwrap();
        assert_eq!(b.micro, 0);
        assert_eq!(b.cond, TrigCond::Always);
    }

    #[test]
    fn old_melodic_note_json_without_m8_fields_defaults() {
        let b: MelodicNote = serde_json::from_str(OLD_MELODIC_NOTE_JSON).unwrap();
        assert_eq!(b.micro, 0);
        assert_eq!(b.cond, TrigCond::Always);
    }

    #[test]
    fn pattern_cc_length_syncs_with_length() {
        let mut p = sample_pattern(16);
        assert_eq!(p.step_cc(3).len(), 0);
        p.set_step_cc(3, vec![CcLock { cc: 74, val: 80 }]);
        assert_eq!(p.step_cc(3), &[CcLock { cc: 74, val: 80 }]);
        // Shrink: cc stays accessible for all valid indices; no panic
        p.sync_cc_len(8);
        p.length = 8;
        assert!(p.step_cc(7).len() <= 1);
        // Grow back: new slots are empty
        p.sync_cc_len(16);
        p.length = 16;
        assert_eq!(p.step_cc(15).len(), 0);
    }

    #[test]
    fn pattern_cc_empty_on_new_pattern() {
        let p = sample_pattern(16);
        assert_eq!(p.cc.len(), 16);
        for i in 0..16 {
            assert!(p.step_cc(i).is_empty());
        }
    }

    #[test]
    fn lane_swing_clockdiv_default_none() {
        let l = sample_lane();
        assert_eq!(l.swing, None);
        assert_eq!(l.clock_div, None);
    }
}
