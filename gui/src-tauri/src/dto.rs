//! GUI-facing DTOs. A deliberate projection of the engine state the interface
//! needs — NOT a serialization of the internal `App`/`Set` (those are not serde
//! types by design). The already-`Serialize` model leaf types (`DrumHit`,
//! `CcLock`, `TrigCond`) are reused verbatim so the wire format never drifts
//! from the engine's own representation.

use midip::app::App;
use midip::devices::profiles;
use midip::pattern::library::{GenreMap, Library};
use midip::pattern::model::{CcLock, DrumHit, LaneKind, PatternData, TrigCond};
use midip::pattern::refs::PatternRef;
use midip::pattern::store::Favorites;
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct Snapshot {
    pub transport: TransportDto,
    pub lanes: Vec<LaneDto>,
    pub focused_lane: usize,
    pub focused_pattern: PatternDto,
    pub selection: SelectionDto,
    pub inspector: InspectorDto,
    pub song: SongDto,
    pub status: String,
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

        Snapshot {
            transport,
            lanes,
            focused_lane: app.focus,
            focused_pattern,
            selection,
            inspector,
            song,
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
}

impl LibraryDto {
    pub fn build(lib: &Library, favs: &Favorites) -> LibraryDto {
        LibraryDto {
            roles: vec![
                role_dto("drums", &lib.drums, favs),
                role_dto("bass", &lib.bass, favs),
                role_dto("synth", &lib.synth, favs),
            ],
        }
    }
}

fn role_dto(role: &str, genres: &GenreMap, favs: &Favorites) -> LibRoleDto {
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
                        LibPatternDto {
                            name: p.name.clone(),
                            length: p.length,
                            kind: kind_str(p.kind()),
                            favorite: favs.contains(&pref),
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
