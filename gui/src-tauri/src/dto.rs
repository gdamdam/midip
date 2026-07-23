//! GUI-facing DTOs. A deliberate projection of the engine state the interface
//! needs — NOT a serialization of the internal `App`/`Set` (those are not serde
//! types by design). The already-`Serialize` model leaf types (`DrumHit`,
//! `CcLock`, `TrigCond`) are reused verbatim so the wire format never drifts
//! from the engine's own representation.

use midip::app::{App, Overlay};
use midip::devices::profiles;
use midip::pattern::generate::{ArpChord, ArpShape, GenMode};
use midip::pattern::index::{self, Density, Energy, Feel, Poly, Query};
use midip::pattern::library::{GenreMap, LibRole, Library, PatternFunction};
use midip::pattern::model::{CcLock, DrumHit, LaneKind, PatternData, TrigCond};
use midip::pattern::refs::PatternRef;
use midip::pattern::store::Favorites;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone)]
pub struct Snapshot {
    pub transport: TransportDto,
    pub lanes: Vec<LaneDto>,
    pub focused_lane: usize,
    pub focused_pattern: PatternDto,
    pub selection: SelectionDto,
    pub inspector: InspectorDto,
    pub song: SongDto,
    pub gen: GenDto,
    pub crates: Vec<CrateDto>,
    /// True when an unclean previous shutdown left recoverable work (set by Core).
    pub recovery_available: bool,
    pub status: String,
}

// --- Crates (pattern collections) --------------------------------------

#[derive(Serialize, Clone)]
pub struct CrateDto {
    pub index: usize,
    pub name: String,
    pub entries: Vec<CrateEntryDto>,
}

#[derive(Serialize, Clone)]
pub struct CrateEntryDto {
    pub label: String,
}

fn build_crates(app: &App) -> Vec<CrateDto> {
    app.crates
        .crates
        .iter()
        .enumerate()
        .map(|(i, c)| CrateDto {
            index: i,
            name: c.name.clone(),
            entries: c
                .entries
                .iter()
                .map(|e| CrateEntryDto {
                    label: e.label.clone().unwrap_or_else(|| e.pattern.display_name()),
                })
                .collect(),
        })
        .collect()
}

// --- Generative tool ----------------------------------------------------

#[derive(Serialize, Clone)]
pub struct GenDto {
    /// True while the generative preview is live (Commit/Cancel pending).
    pub active: bool,
    /// "generate" | "vary" | "arp"
    pub mode: String,
    pub density: u8,
    pub range: u8,
    pub mutate: u8,
    /// "power" | "triad" | "seventh" | "octaves"
    pub arp_chord: String,
    pub arp_octaves: u8,
    /// "up" | "down" | "updown" | "random"
    pub arp_shape: String,
    pub arp_gate: f32,
    pub arp_vel_var: u8,
    /// Whether the focused lane is melodic (Arp mode is melodic-only).
    pub melodic: bool,
}

fn build_gen(app: &App) -> GenDto {
    let p = &app.gen_params;
    let melodic = app
        .set
        .lanes
        .get(app.focus)
        .map(|l| l.pattern.kind() == LaneKind::Melodic)
        .unwrap_or(false);
    GenDto {
        active: app.overlay == Some(Overlay::Generative),
        mode: match p.mode {
            GenMode::Generate => "generate",
            GenMode::Vary => "vary",
            GenMode::Arp => "arp",
        }
        .into(),
        density: p.density,
        range: p.range,
        mutate: p.mutate,
        arp_chord: match p.arp_chord {
            ArpChord::Power => "power",
            ArpChord::Triad => "triad",
            ArpChord::Seventh => "seventh",
            ArpChord::Octaves => "octaves",
        }
        .into(),
        arp_octaves: p.arp_octaves,
        arp_shape: match p.arp_shape {
            ArpShape::Up => "up",
            ArpShape::Down => "down",
            ArpShape::UpDown => "updown",
            ArpShape::Random => "random",
        }
        .into(),
        arp_gate: p.arp_gate,
        arp_vel_var: p.arp_vel_var,
        melodic,
    }
}

// --- Song (scenes + chains) --------------------------------------------

#[derive(Serialize, Clone)]
pub struct SongDto {
    pub scenes: Vec<SceneDto>,
    pub chains: Vec<ChainDto>,
    /// Index of the chain currently auto-advancing, if any.
    pub playing_chain: Option<usize>,
}

#[derive(Serialize, Clone)]
pub struct SceneDto {
    pub index: usize,
    pub name: String,
}

#[derive(Serialize, Clone)]
pub struct ChainDto {
    pub index: usize,
    pub name: String,
    pub looped: bool,
    pub entries: Vec<ChainEntryDto>,
    /// Currently-playing entry index within this chain (when it is the active chain).
    pub current_entry: Option<usize>,
}

#[derive(Serialize, Clone)]
pub struct ChainEntryDto {
    pub scene: String,
    pub repeats: u32,
    pub bars: u32,
}

#[derive(Serialize, Clone)]
pub struct TransportDto {
    pub playing: bool,
    pub armed: bool,
    /// Tempo to display: the Link tempo while Link is enabled, else the set BPM.
    pub bpm: f64,
    pub set_bpm: f64,
    /// 0.0..1.0 (0.5 = straight).
    pub swing: f32,
    pub link_enabled: bool,
    pub link_peers: u64,
    pub position: PositionDto,
    pub playhead: usize,
    pub dirty: bool,
    pub mirror: bool,
    pub clock_in: Option<ClockInDto>,
    pub set_name: String,
}

#[derive(Serialize, Clone)]
pub struct PositionDto {
    pub bar: u32,
    pub beat: u32,
    pub sixteenth: u32,
}

#[derive(Serialize, Clone)]
pub struct ClockInDto {
    pub locked: bool,
    pub tempo: f64,
    pub port: String,
}

#[derive(Serialize, Clone)]
pub struct LaneDto {
    pub index: usize,
    /// "drums" | "melodic"
    pub kind: String,
    /// device profile id, e.g. "t8-drums" | "t8-bass" | "s1"
    pub role: String,
    pub label: String,
    pub pattern_name: String,
    pub connected: bool,
    /// Connected port name, or the profile's port-match hint when disconnected.
    pub device: String,
    /// 1-based MIDI channel for display (engine is 0-based).
    pub channel: u8,
    pub mute: bool,
    pub solo: bool,
    pub length: usize,
    pub queued: Option<String>,
    pub transpose: i8,
    pub octave: i8,
    pub focused: bool,
    /// Effective output-port name (explicit route, or the profile default).
    pub route_port: String,
    /// True when the lane uses its profile default route (no explicit override).
    pub route_default: bool,
    /// Whether MIDI clock is sent to this lane's port.
    pub clock_out: bool,
    /// Per-lane swing override (0..1), or null when it inherits the set swing.
    pub swing: Option<f32>,
    /// Per-lane clock division, or null when it inherits the default.
    pub clock_div: Option<u8>,
}

#[derive(Serialize, Clone)]
pub struct PatternDto {
    /// "drums" | "melodic"
    pub kind: String,
    pub name: String,
    pub length: usize,
    /// Drum-lane voice rows (note + label + muted). Empty for melodic lanes.
    pub voices: Vec<VoiceDto>,
    /// Drums: per-step list of hits. Empty for melodic lanes.
    pub drum_steps: Vec<Vec<DrumHit>>,
    /// Melodic: per-step list of notes (with resolved pitch). Empty for drums.
    pub melodic_steps: Vec<Vec<MelodicNoteDto>>,
    pub scale: String,
    pub root: u8,
    pub octave: i8,
    pub transpose: i8,
    /// Per-step CC locks (length-synced with the pattern).
    pub cc: Vec<Vec<CcLock>>,
    pub muted_voices: Vec<u8>,
}

#[derive(Serialize, Clone)]
pub struct VoiceDto {
    pub note: u8,
    pub label: String,
    pub muted: bool,
}

#[derive(Serialize, Clone)]
pub struct MelodicNoteDto {
    pub semi: i8,
    /// Resolved absolute MIDI pitch (root + semi + transpose + octave).
    pub pitch: u8,
    /// Velocity multiplier (0.5..=1.3), NOT MIDI velocity.
    pub vel: f32,
    pub slide: bool,
    pub len: f32,
    pub prob: f32,
    pub ratchet: u8,
    pub micro: i16,
    pub cond: TrigCond,
}

#[derive(Serialize, Clone)]
pub struct SelectionDto {
    pub row: usize,
    pub col: usize,
}

/// The selected step's editable parameters. Fields are `None` when they don't
/// apply to the lane kind or when the step is empty.
#[derive(Serialize, Clone)]
pub struct InspectorDto {
    pub kind: String,
    pub present: bool,
    /// Drum MIDI velocity (1..=127).
    pub velocity: Option<u8>,
    /// Melodic velocity multiplier.
    pub vel_mult: Option<f32>,
    pub probability: Option<f32>,
    pub ratchet: Option<u8>,
    pub length: Option<f32>,
    pub slide: Option<bool>,
    pub micro: Option<i16>,
    pub cond: Option<TrigCond>,
    pub pitch: Option<u8>,
    pub cc: Vec<CcLock>,
}

impl Snapshot {
    pub fn build(app: &App) -> Snapshot {
        let set = &app.set;

        // --- transport ---
        let ph = app.playhead;
        let position = PositionDto {
            bar: app.bar + 1,
            beat: ((ph / 4) % 4 + 1) as u32,
            sixteenth: (ph % 4 + 1) as u32,
        };
        let clock_in = app.clock_in_port.as_ref().map(|p| ClockInDto {
            locked: app.clock_in_locked.unwrap_or(false),
            tempo: app.clock_in_tempo,
            port: p.name.clone(),
        });
        let transport = TransportDto {
            playing: app.engine_playing,
            armed: app.armed,
            bpm: if app.link_enabled {
                app.link_tempo
            } else {
                set.bpm
            },
            set_bpm: set.bpm,
            swing: set.swing,
            link_enabled: app.link_enabled,
            link_peers: app.link_peers,
            position,
            playhead: ph,
            dirty: app.dirty,
            mirror: app.mirror_on,
            clock_in,
            set_name: set.name.clone(),
        };

        // --- lanes ---
        let lanes = set
            .lanes
            .iter()
            .enumerate()
            .map(|(i, lane)| {
                let (connected, port) = app
                    .device_status
                    .get(i)
                    .cloned()
                    .unwrap_or((false, String::new()));
                let device = if connected && !port.is_empty() {
                    port
                } else {
                    lane.profile.port_match.to_string()
                };
                LaneDto {
                    index: i,
                    kind: kind_str(lane.pattern.kind()),
                    role: lane.profile.id.to_string(),
                    label: lane.profile.label.to_string(),
                    pattern_name: lane.pattern.name.clone(),
                    connected,
                    device,
                    channel: lane.route_channel().saturating_add(1),
                    mute: lane.mute,
                    solo: lane.solo,
                    length: lane.pattern.length,
                    queued: app.queued.get(i).cloned().flatten(),
                    transpose: lane.transpose,
                    octave: lane.octave,
                    focused: i == app.focus,
                    route_port: lane.effective_route().port.name,
                    route_default: lane.route.is_none(),
                    clock_out: lane.effective_route().clock_out,
                    swing: lane.swing,
                    clock_div: lane.clock_div,
                }
            })
            .collect();

        // --- focused pattern + inspector ---
        let focused_pattern = build_pattern(app);
        let selection = SelectionDto {
            row: app.cur_row,
            col: app.cur_col,
        };
        let inspector = build_inspector(app);
        let song = build_song(app);
        let gen = build_gen(app);
        let crates = build_crates(app);

        Snapshot {
            transport,
            lanes,
            focused_lane: app.focus,
            focused_pattern,
            selection,
            inspector,
            song,
            gen,
            crates,
            recovery_available: false, // overridden by Core::snapshot
            status: app.status.clone(),
        }
    }
}

fn build_song(app: &App) -> SongDto {
    let set = &app.set;
    let scenes = set
        .scenes
        .iter()
        .enumerate()
        .map(|(i, s)| SceneDto {
            index: i,
            name: s.name.clone(),
        })
        .collect();

    // Which chain (if any) is actively auto-advancing, and at which entry.
    let (active_chain_id, active_entry) = match &app.chain_playback {
        Some(pb) if pb.active => (Some(pb.chain_id.clone()), Some(pb.entry_idx)),
        _ => (None, None),
    };
    let mut playing_chain = None;

    let chains = set
        .chains
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let is_active = active_chain_id.as_ref() == Some(&c.id);
            if is_active {
                playing_chain = Some(i);
            }
            let entries = c
                .entries
                .iter()
                .map(|e| ChainEntryDto {
                    scene: set
                        .scenes
                        .iter()
                        .find(|s| s.id == e.scene_id)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "(missing)".into()),
                    repeats: e.repeats,
                    bars: e.bars,
                })
                .collect();
            ChainDto {
                index: i,
                name: c.name.clone(),
                looped: c.looped,
                entries,
                current_entry: if is_active { active_entry } else { None },
            }
        })
        .collect();

    SongDto {
        scenes,
        chains,
        playing_chain,
    }
}

// --- Library browsing ---------------------------------------------------

#[derive(Serialize, Clone)]
pub struct LibraryDto {
    pub roles: Vec<LibRoleDto>,
}

#[derive(Serialize, Clone)]
pub struct LibRoleDto {
    /// "drums" | "bass" | "synth"
    pub role: String,
    pub genres: Vec<LibGenreDto>,
}

#[derive(Serialize, Clone)]
pub struct LibGenreDto {
    pub name: String,
    pub patterns: Vec<LibPatternDto>,
}

#[derive(Serialize, Clone)]
pub struct LibPatternDto {
    pub name: String,
    pub length: usize,
    /// "drums" | "melodic"
    pub kind: String,
    pub favorite: bool,
    /// Performance-family label this pattern belongs to (Phase 3), if any.
    pub family: Option<String>,
    /// Stable family id (for grouping/filtering in the UI), if any.
    pub family_id: Option<String>,
    /// Function within the family: "Core"/"Variation A"/…, if any.
    pub function: Option<String>,
}

impl LibraryDto {
    pub fn build(lib: &Library, favs: &Favorites) -> LibraryDto {
        LibraryDto {
            roles: vec![
                role_dto(lib, "drums", LibRole::Drums, &lib.drums, favs),
                role_dto(lib, "bass", LibRole::Bass, &lib.bass, favs),
                role_dto(lib, "synth", LibRole::Synth, &lib.synth, favs),
            ],
        }
    }
}

fn role_dto(
    lib: &Library,
    role: &str,
    lib_role: LibRole,
    genres: &GenreMap,
    favs: &Favorites,
) -> LibRoleDto {
    LibRoleDto {
        role: role.to_string(),
        genres: genres
            .iter()
            .map(|(genre, pats)| LibGenreDto {
                name: genre.clone(),
                patterns: pats
                    .iter()
                    .map(|p| {
                        let pref = PatternRef::Vendored {
                            role: role.to_string(),
                            genre: genre.clone(),
                            name: p.name.clone(),
                        };
                        let fam = lib.family_of(lib_role, genre, &p.name);
                        LibPatternDto {
                            name: p.name.clone(),
                            length: p.length,
                            kind: kind_str(p.kind()),
                            favorite: favs.contains(&pref),
                            family: fam.map(|(f, _)| f.label.clone()),
                            family_id: fam.map(|(f, _)| f.id.clone()),
                            function: fam.map(|(_, func)| func.label().to_string()),
                        }
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn kind_str(k: LaneKind) -> String {
    match k {
        LaneKind::Drums => "drums".into(),
        LaneKind::Melodic => "melodic".into(),
    }
}

fn build_pattern(app: &App) -> PatternDto {
    // A GUI with zero lanes is not a real state, but stay panic-free.
    let Some(lane) = app.set.lanes.get(app.focus) else {
        return PatternDto {
            kind: "drums".into(),
            name: String::new(),
            length: 0,
            voices: vec![],
            drum_steps: vec![],
            melodic_steps: vec![],
            scale: String::new(),
            root: 0,
            octave: 0,
            transpose: 0,
            cc: vec![],
            muted_voices: vec![],
        };
    };
    let pat = &lane.pattern;
    let cc: Vec<Vec<CcLock>> = (0..pat.length).map(|s| pat.step_cc(s).to_vec()).collect();

    match &pat.data {
        PatternData::Drums(steps) => {
            // Drum rows follow the engine's fixed editor kit (`DRUM_VOICES`), which
            // is what `App`'s cursor row and `toggle_step` index — NOT the profile's
            // own voice list — so the GUI grid stays aligned with the engine.
            let voices = profiles::DRUM_VOICES
                .iter()
                .map(|v| VoiceDto {
                    note: v.note,
                    label: v.label.to_string(),
                    muted: lane.is_voice_muted(v.note),
                })
                .collect();
            PatternDto {
                kind: "drums".into(),
                name: pat.name.clone(),
                length: pat.length,
                voices,
                drum_steps: steps.clone(),
                melodic_steps: vec![],
                scale: lane.scale.name().to_string(),
                root: lane.effective_root(),
                octave: lane.octave,
                transpose: lane.transpose,
                cc,
                muted_voices: lane.muted_voices.clone(),
            }
        }
        PatternData::Melodic(steps) => {
            let root = lane.effective_root();
            let melodic_steps = steps
                .iter()
                .map(|step| {
                    step.iter()
                        .map(|n| MelodicNoteDto {
                            semi: n.semi,
                            pitch: profiles::resolve_melodic_pitch(
                                root,
                                n.semi,
                                lane.transpose,
                                lane.octave,
                            ),
                            vel: n.vel,
                            slide: n.slide,
                            len: n.len,
                            prob: n.prob,
                            ratchet: n.ratchet,
                            micro: n.micro,
                            cond: n.cond.clone(),
                        })
                        .collect()
                })
                .collect();
            PatternDto {
                kind: "melodic".into(),
                name: pat.name.clone(),
                length: pat.length,
                voices: vec![],
                drum_steps: vec![],
                melodic_steps,
                scale: lane.scale.name().to_string(),
                root,
                octave: lane.octave,
                transpose: lane.transpose,
                cc,
                muted_voices: vec![],
            }
        }
    }
}

fn build_inspector(app: &App) -> InspectorDto {
    let empty = |kind: String| InspectorDto {
        kind,
        present: false,
        velocity: None,
        vel_mult: None,
        probability: None,
        ratchet: None,
        length: None,
        slide: None,
        micro: None,
        cond: None,
        pitch: None,
        cc: vec![],
    };
    let Some(lane) = app.set.lanes.get(app.focus) else {
        return empty("drums".into());
    };
    let pat = &lane.pattern;
    let col = app.cur_col;
    let cc = pat.step_cc(col).to_vec();

    match &pat.data {
        PatternData::Drums(steps) => {
            let note = profiles::DRUM_VOICES.get(app.cur_row).map(|v| v.note);
            let hit = note.and_then(|n| {
                steps
                    .get(col)
                    .and_then(|hits| hits.iter().find(|h| h.note == n))
            });
            match hit {
                Some(h) => InspectorDto {
                    kind: "drums".into(),
                    present: true,
                    velocity: Some(h.vel),
                    vel_mult: None,
                    probability: Some(h.prob),
                    ratchet: Some(h.ratchet),
                    length: None,
                    slide: None,
                    micro: Some(h.micro),
                    cond: Some(h.cond.clone()),
                    pitch: note,
                    cc,
                },
                None => InspectorDto {
                    kind: "drums".into(),
                    present: false,
                    pitch: note,
                    cc,
                    ..empty("drums".into())
                },
            }
        }
        PatternData::Melodic(steps) => {
            let note = steps.get(col).and_then(|s| s.first());
            match note {
                Some(n) => {
                    let pitch = profiles::resolve_melodic_pitch(
                        lane.effective_root(),
                        n.semi,
                        lane.transpose,
                        lane.octave,
                    );
                    InspectorDto {
                        kind: "melodic".into(),
                        present: true,
                        velocity: None,
                        vel_mult: Some(n.vel),
                        probability: Some(n.prob),
                        ratchet: Some(n.ratchet),
                        length: Some(n.len),
                        slide: Some(n.slide),
                        micro: Some(n.micro),
                        cond: Some(n.cond.clone()),
                        pitch: Some(pitch),
                        cc,
                    }
                }
                None => InspectorDto {
                    kind: "melodic".into(),
                    present: false,
                    cc,
                    ..empty("melodic".into())
                },
            }
        }
    }
}

// ── Phase 8: shared library query (one engine, both frontends) ──────────────

/// A filtered library record for the GUI. Mirrors `index::Record` plus favorite +
/// family_id (resolved from the library), reusing the same identity the rest of the
/// GUI keys on (role+genre+name).
#[derive(Serialize, Clone)]
pub struct RecordDto {
    pub role: String,
    pub genre: String,
    pub name: String,
    pub kind: String,
    pub length: usize,
    pub favorite: bool,
    pub family: Option<String>,
    pub family_id: Option<String>,
    pub function: Option<String>,
    pub feel: String,
    pub energy: String,
    pub density: String,
    pub poly: String,
    pub bpm_min: Option<u16>,
    pub bpm_max: Option<u16>,
    pub subgenre: Option<String>,
    pub harmonic: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub source: Option<String>,
    pub factory_id: Option<String>,
}

/// Query parameters from the front-end (all optional; omitted = no constraint).
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct QueryDto {
    pub text: String,
    pub role: Option<String>,
    pub genre: Option<String>,
    pub function: Option<String>,
    pub feel: Option<String>,
    pub energy: Option<String>,
    pub density: Option<String>,
    pub poly: Option<String>,
    pub length_min: Option<usize>,
    pub length_max: Option<usize>,
    pub favorites_only: bool,
}

fn parse_role(s: &str) -> Option<LibRole> {
    match s {
        "drums" => Some(LibRole::Drums),
        "bass" => Some(LibRole::Bass),
        "synth" => Some(LibRole::Synth),
        _ => None,
    }
}

fn parse_function(s: &str) -> Option<PatternFunction> {
    match s {
        "core" => Some(PatternFunction::Core),
        "variation_a" => Some(PatternFunction::VariationA),
        "variation_b" => Some(PatternFunction::VariationB),
        "fill" => Some(PatternFunction::Fill),
        "breakdown" => Some(PatternFunction::Breakdown),
        "peak" => Some(PatternFunction::Peak),
        _ => None,
    }
}

impl QueryDto {
    fn to_query(&self) -> Query {
        let mut q = Query::default().with_text(&self.text);
        q.role = self.role.as_deref().and_then(parse_role);
        q.genre = self.genre.clone().filter(|s| !s.is_empty());
        q.function = self.function.as_deref().and_then(parse_function);
        q.feel = self.feel.as_deref().map(Feel::parse).filter(|f| *f != Feel::Unknown);
        q.energy = self.energy.as_deref().map(Energy::parse).filter(|e| *e != Energy::Unknown);
        q.density = self.density.as_deref().map(Density::parse).filter(|d| *d != Density::Unknown);
        q.poly = self.poly.as_deref().and_then(|s| match s {
            "mono" => Some(Poly::Mono),
            "poly" => Some(Poly::Poly),
            _ => None,
        });
        q.length = match (self.length_min, self.length_max) {
            (None, None) => None,
            (lo, hi) => Some((lo.unwrap_or(1), hi.unwrap_or(usize::MAX))),
        };
        q.favorites_only = self.favorites_only;
        q
    }
}

/// Run a query against the library and project the matches to `RecordDto`.
pub fn library_query(lib: &Library, favs: &Favorites, qdto: &QueryDto) -> Vec<RecordDto> {
    let q = qdto.to_query();
    index::filter(lib.records(), &q, favs)
        .into_iter()
        .map(|i| {
            let r = &lib.records()[i];
            let role = match r.role {
                LibRole::Drums => "drums",
                LibRole::Bass => "bass",
                LibRole::Synth => "synth",
            };
            let fam_id = lib
                .family_of(r.role, &r.genre, &r.name)
                .map(|(f, _)| f.id.clone());
            RecordDto {
                role: role.to_string(),
                genre: r.genre.clone(),
                name: r.name.clone(),
                kind: kind_str(r.kind),
                length: r.length,
                favorite: favs.contains(&r.pattern_ref()),
                family: r.family.clone(),
                family_id: fam_id,
                function: r.function.map(|f| f.label().to_string()),
                feel: r.feel.label().to_string(),
                energy: r.energy.label().to_string(),
                density: r.density.label().to_string(),
                poly: match r.poly {
                    Poly::Mono => "mono".into(),
                    Poly::Poly => "poly".into(),
                },
                bpm_min: r.bpm.map(|b| b.0),
                bpm_max: r.bpm.map(|b| b.1),
                subgenre: r.subgenre.clone(),
                harmonic: r.harmonic.clone(),
                tags: r.tags.clone(),
                author: r.author.clone(),
                source: r.source.clone(),
                factory_id: r.factory_id.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_dto_carries_family_metadata_and_serializes() {
        // Loads the real vendored library (patterns_dir resolves to the midip
        // crate's assets at compile time).
        let lib = Library::load(&midip::config::patterns_dir()).expect("load library");
        let favs = Favorites::default();
        let dto = LibraryDto::build(&lib, &favs);

        // Find the drums/techno "Four on Floor" pattern DTO.
        let drums = dto.roles.iter().find(|r| r.role == "drums").unwrap();
        let techno = drums.genres.iter().find(|g| g.name == "techno").unwrap();
        let four = techno
            .patterns
            .iter()
            .find(|p| p.name == "Four on Floor")
            .unwrap();
        assert_eq!(four.family.as_deref(), Some("Warehouse Techno"));
        assert_eq!(four.family_id.as_deref(), Some("techno-drive-drums"));
        assert_eq!(four.function.as_deref(), Some("Core"));

        // Non-member patterns leave the fields null (the family tags only the
        // six enrolled members, not the whole genre).
        assert!(
            techno.patterns.iter().any(|p| p.family.is_none()),
            "genre should contain patterns outside the family"
        );

        // JSON round-trip surfaces the camelCase-free snake keys used by the UI.
        let json = serde_json::to_string(four).unwrap();
        assert!(json.contains("\"family\":\"Warehouse Techno\""));
        assert!(json.contains("\"family_id\":\"techno-drive-drums\""));
        assert!(json.contains("\"function\":\"Core\""));
    }

    #[test]
    fn library_query_projects_filtered_records() {
        let lib = Library::load(&midip::config::patterns_dir()).expect("load library");
        let favs = Favorites::default();

        // Facet query: swung drums.
        let q = QueryDto {
            role: Some("drums".into()),
            feel: Some("swing".into()),
            ..Default::default()
        };
        let out = library_query(&lib, &favs, &q);
        assert!(!out.is_empty());
        assert!(out.iter().all(|r| r.role == "drums" && r.feel == "swing"));

        // A v2 record carries projected metadata + resolves its family_id.
        let bb = out.iter().find(|r| r.genre == "boom-bap");
        if let Some(bb) = bb {
            assert!(bb.bpm_min.is_some());
            assert!(bb.family_id.is_some());
            let json = serde_json::to_string(bb).unwrap();
            assert!(json.contains("\"feel\":\"swing\""));
        }

        // Text query is case-insensitive and hits a known genre token.
        let q = QueryDto { text: "TRAP".into(), ..Default::default() };
        assert!(library_query(&lib, &favs, &q).iter().any(|r| r.genre == "trap"));
    }
}
