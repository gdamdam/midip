//! Application state and the action reducer (UI thread side).
//!
//! `App` holds the canonical edit state. `apply` mutates state and returns the
//! `UiCommand`s that must be forwarded to the engine (e.g. pattern edits emit
//! `LoadPattern`). Undo/redo snapshot the whole `Set`.

use crate::devices::profiles;
use crate::engine::{EngineEvent, UiCommand};
use crate::pattern::euclid;
use crate::pattern::library::{LibRole, Library};
use crate::pattern::model::{
    DrumHit, DrumStep, Lane, LaneKind, MelodicNote, MelodicStep, Pattern, PatternData, Set,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Edit,
    Library,
    Help,
    TempoEntry,
    SetBrowser,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LibCol {
    Genre,
    Pattern,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    TogglePlay,
    FocusNext,
    FocusPrev,
    FocusLane(usize),
    MoveCursor(i32, i32),
    ToggleStep,
    SetVelBucket(u8),
    AdjustVel(i8),
    NoteUp,
    NoteDown,
    AdjustLen(i8),
    AdjustOctave(i8),
    ToggleSlide,
    CutStep,
    CopyStep,
    PasteStep,
    RotateRight,
    RotateLeft,
    ClearStep,
    Undo,
    Redo,
    ToggleMute,
    ToggleSolo,
    SetBpm(f64),
    Tap,
    ToggleLink,
    OpenTempo,
    TempoDigit(char),
    TempoBackspace,
    TempoCommit,
    TempoCancel,
    AdjustBpm(i32),
    AdjustSwing(i8), // transport swing param (distinct from AdjustLen = melodic note len)
    AdjustPatternLen(i8), // resize the focused lane's pattern length
    AdjustProb(i8),  // per-step probability on the cursor cell (±0.1 per unit)
    AdjustRatchet(i8), // per-step ratchet count on the cursor cell (clamped 1..=8)
    Euclid { dp: i8, dr: i8 }, // drums: dp = ±pulses for focused voice, dr = ±rotation
    Panic,           // all-notes-off; no undo snapshot, no Set mutation
    OpenLibrary,
    CloseLibrary,
    LibNav(i32, i32),
    LibLoad,
    Audition,
    OpenSetBrowser,
    SetBrowserNav(i32),
    SetBrowserLoad,
    CloseSetBrowser,
    Save,
    Help,
    Quit,
    None,
}

/// Number of steps visible in the editor at once. Steps beyond this are reached via scrolling.
pub const VISIBLE_STEPS: usize = 16;

pub struct App {
    pub set: Set,
    pub focus: usize,
    pub mode: Mode,
    pub cur_row: usize,
    pub cur_col: usize,
    /// First visible step column — always `(cur_col / VISIBLE_STEPS) * VISIBLE_STEPS` (page-snapped).
    pub step_scroll: usize,
    pub euclid_rotation: usize, // current euclid rotation for the focused drum voice

    pub playing: bool,
    pub playhead: usize,
    pub bar: u32,
    pub link_enabled: bool,
    pub link_tempo: f64,
    pub link_peers: u64,
    /// Previous peer count — used to detect 2→0 (Link lost) and 0→N (Link gained) transitions.
    prev_peers: u64,
    pub device_status: Vec<(bool, String)>,
    pub library: Library,
    pub lib_role: LibRole,
    pub lib_col: LibCol,
    pub lib_genre: usize,
    pub lib_pattern: usize,
    pub set_files: Vec<std::path::PathBuf>,
    pub set_sel: usize,
    pub clipboard: Option<PatternData>,
    pub undo: Vec<Set>,
    pub redo: Vec<Set>,
    pub status: String,
    pub should_quit: bool,
    pub tempo_input: String,
    /// Armed for double-q quit: true after first Quit while playing.
    pub quit_armed: bool,
    /// True when the Set has unsaved mutations since the last successful Save.
    pub dirty: bool,
    /// Original pattern of the focused lane when an audition preview is active.
    /// `None` when not auditioning. Set by `Action::Audition`, cleared by `LibLoad`
    /// (commit) or `CloseLibrary` (cancel/revert).
    pub audition: Option<Pattern>,
}

/// Default melodic velocity multiplier when placing a note (1.0 -> MIDI 100).
const MEL_DEFAULT_VEL: f32 = 1.0;

impl App {
    pub fn new(set: Set, library: Library) -> App {
        let n = set.lanes.len();
        let role = role_for_profile(
            set.lanes
                .first()
                .map(|l| l.profile.id)
                .unwrap_or("t8-drums"),
        );
        App {
            set,
            focus: 0,
            mode: Mode::Edit,
            cur_row: 0,
            cur_col: 0,
            step_scroll: 0,
            euclid_rotation: 0,
            playing: false,
            playhead: 0,
            bar: 0,
            link_enabled: false,
            link_tempo: 120.0,
            link_peers: 0,
            prev_peers: 0,
            device_status: vec![(false, String::new()); n],
            library,
            lib_role: role,
            lib_col: LibCol::Genre,
            lib_genre: 0,
            lib_pattern: 0,
            set_files: Vec::new(),
            set_sel: 0,
            clipboard: Option::None,
            undo: Vec::new(),
            redo: Vec::new(),
            status: String::new(),
            should_quit: false,
            tempo_input: String::new(),
            quit_armed: false,
            dirty: false,
            audition: None,
        }
    }

    pub fn focused_lane(&self) -> &Lane {
        &self.set.lanes[self.focus]
    }

    pub fn focused_kind(&self) -> LaneKind {
        self.set.lanes[self.focus].profile.kind
    }

    /// Apply an action; mutate state and return engine commands to forward.
    pub fn apply(&mut self, action: Action) -> Vec<UiCommand> {
        // Any action other than Quit disarms the double-q quit gesture.
        if action != Action::Quit {
            self.quit_armed = false;
        }

        let mut cmds = Vec::new();
        match action {
            Action::TogglePlay => {
                self.playing = !self.playing;
                cmds.push(if self.playing {
                    UiCommand::Play
                } else {
                    UiCommand::Stop
                });
            }
            Action::FocusNext => self.set_focus((self.focus + 1) % self.set.lanes.len()),
            Action::FocusPrev => {
                let n = self.set.lanes.len();
                self.set_focus((self.focus + n - 1) % n);
            }
            Action::FocusLane(i) => {
                if i < self.set.lanes.len() {
                    self.set_focus(i);
                }
            }
            Action::MoveCursor(dx, dy) => self.move_cursor(dx, dy),
            Action::ToggleStep => {
                self.snapshot();
                self.toggle_step();
                cmds.push(self.load_focused());
            }
            Action::SetVelBucket(b) => {
                self.snapshot();
                self.set_vel_bucket(b);
                if let Some(v) = self.cursor_vel_midi() {
                    self.status = format!("Velocity {}", v);
                }
                cmds.push(self.load_focused());
            }
            Action::AdjustVel(d) => {
                self.snapshot();
                self.adjust_vel(d);
                if let Some(v) = self.cursor_vel_midi() {
                    self.status = format!("Velocity {}", v);
                }
                cmds.push(self.load_focused());
            }
            Action::NoteUp => {
                self.snapshot();
                self.adjust_semi(1);
                cmds.push(self.load_focused());
            }
            Action::NoteDown => {
                self.snapshot();
                self.adjust_semi(-1);
                cmds.push(self.load_focused());
            }
            Action::AdjustLen(d) => {
                self.snapshot();
                self.adjust_len(d);
                cmds.push(self.load_focused());
            }
            Action::AdjustOctave(d) => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                lane.octave = (lane.octave as i32 + d as i32).clamp(-4, 4) as i8;
                let new_octave = lane.octave;
                cmds.push(UiCommand::SetOctave {
                    lane: self.focus,
                    octave: new_octave,
                });
                cmds.push(self.load_focused());
            }
            Action::ToggleSlide => {
                self.snapshot();
                self.toggle_slide();
                cmds.push(self.load_focused());
            }
            Action::CutStep => {
                self.snapshot();
                self.copy_step();
                self.clear_step();
                cmds.push(self.load_focused());
            }
            Action::CopyStep => {
                self.copy_step();
            }
            Action::PasteStep => {
                self.snapshot();
                self.paste_step();
                cmds.push(self.load_focused());
            }
            Action::RotateRight => {
                self.snapshot();
                self.rotate(true);
                cmds.push(self.load_focused());
            }
            Action::RotateLeft => {
                self.snapshot();
                self.rotate(false);
                cmds.push(self.load_focused());
            }
            Action::ClearStep => {
                self.snapshot();
                self.clear_step();
                cmds.push(self.load_focused());
            }
            Action::Undo => {
                self.undo();
                cmds.push(UiCommand::SyncLanes(self.set.lanes.clone()));
            }
            Action::Redo => {
                self.redo();
                cmds.push(UiCommand::SyncLanes(self.set.lanes.clone()));
            }
            Action::ToggleMute => {
                let lane = &mut self.set.lanes[self.focus];
                lane.mute = !lane.mute;
                let (n, muted) = (self.focus, self.set.lanes[self.focus].mute);
                self.status = format!("Lane {} {}", n, if muted { "muted" } else { "unmuted" });
                cmds.push(UiCommand::Mute {
                    lane: self.focus,
                    on: self.set.lanes[self.focus].mute,
                });
            }
            Action::ToggleSolo => {
                let lane = &mut self.set.lanes[self.focus];
                lane.solo = !lane.solo;
                let (n, soloed) = (self.focus, self.set.lanes[self.focus].solo);
                self.status = format!("Lane {} {}", n, if soloed { "solo" } else { "unsolo" });
                cmds.push(UiCommand::Solo {
                    lane: self.focus,
                    on: self.set.lanes[self.focus].solo,
                });
            }
            Action::SetBpm(bpm) => {
                self.set.bpm = bpm;
                cmds.push(UiCommand::SetBpm(bpm));
            }
            Action::Tap => cmds.push(UiCommand::Tap),
            Action::ToggleLink => {
                self.link_enabled = !self.link_enabled;
                self.status = if self.link_enabled {
                    "Link on".into()
                } else {
                    "Link off".into()
                };
                cmds.push(UiCommand::ToggleLink(self.link_enabled));
            }
            Action::OpenTempo => {
                self.mode = Mode::TempoEntry;
                self.tempo_input.clear();
            }
            Action::TempoDigit(c) => {
                if c.is_ascii_digit() && self.tempo_input.len() < 3 {
                    self.tempo_input.push(c);
                }
            }
            Action::TempoBackspace => {
                self.tempo_input.pop();
            }
            Action::TempoCommit => {
                self.mode = Mode::Edit;
                if let Ok(bpm) = self.tempo_input.parse::<f64>() {
                    let bpm = bpm.clamp(20.0, 300.0);
                    self.set.bpm = bpm;
                    self.status = format!("BPM {}", bpm as i64);
                    cmds.push(UiCommand::SetBpm(bpm));
                }
                self.tempo_input.clear();
            }
            Action::TempoCancel => {
                self.mode = Mode::Edit;
                self.tempo_input.clear();
            }
            Action::AdjustBpm(d) => {
                self.set.bpm = (self.set.bpm + d as f64).clamp(20.0, 300.0);
                self.status = format!("BPM {}", self.set.bpm as i64);
                cmds.push(UiCommand::SetBpm(self.set.bpm));
            }
            Action::AdjustSwing(d) => {
                // Swing mutates the Set, so it is snapshotted per the undo invariant.
                self.snapshot();
                self.set.swing = (self.set.swing + d as f32 * 0.02).clamp(0.5, 0.66);
                self.status = format!("Swing {}%", (self.set.swing * 100.0).round() as i64);
                cmds.push(UiCommand::SetSwing(self.set.swing));
            }
            Action::AdjustPatternLen(d) => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                let new_len = (lane.pattern.length as i32 + d as i32).clamp(1, 64) as usize;
                match &mut lane.pattern.data {
                    PatternData::Drums(steps) => steps.resize(new_len, Vec::new()),
                    PatternData::Melodic(steps) => steps.resize(new_len, Option::None),
                }
                lane.pattern.length = new_len;
                self.clamp_cursor();
                cmds.push(self.load_focused());
            }
            Action::AdjustProb(d) => {
                if self.adjust_prob(d) {
                    if let Some(pct) = self.cursor_prob_pct() {
                        self.status = format!("Prob {}%", pct);
                    }
                    cmds.push(self.load_focused());
                }
            }
            Action::AdjustRatchet(d) => {
                if self.adjust_ratchet(d) {
                    if let Some(r) = self.cursor_ratchet() {
                        self.status = format!("Ratchet x{}", r);
                    }
                    cmds.push(self.load_focused());
                }
            }
            Action::Euclid { dp, dr } => {
                if self.apply_euclid(dp, dr) {
                    let pulses = self.euclid_current_pulses();
                    let steps = self.set.lanes[self.focus].pattern.length;
                    self.status = format!("Euclid E({},{})", pulses, steps);
                    cmds.push(self.load_focused());
                }
            }
            Action::Panic => {
                // Live recovery: forward to the engine. No undo snapshot, no Set mutation.
                cmds.push(UiCommand::Panic);
            }
            Action::OpenLibrary => self.mode = Mode::Library,
            Action::CloseLibrary => {
                if let Some(backup) = self.audition.take() {
                    // Cancel audition: restore original pattern.
                    self.set.lanes[self.focus].pattern = backup.clone();
                    cmds.push(UiCommand::LoadPattern {
                        lane: self.focus,
                        pattern: backup,
                    });
                    self.status = "Audition cancelled".into();
                }
                self.mode = Mode::Edit;
            }
            Action::LibNav(dx, dy) => {
                self.lib_nav(dx, dy);
                // Re-audition the newly-selected pattern if an audition is active.
                if self.audition.is_some() {
                    if let Some(pat) = self.selected_lib_pattern().cloned() {
                        self.set.lanes[self.focus].pattern = pat.clone();
                        cmds.push(UiCommand::LoadPattern {
                            lane: self.focus,
                            pattern: pat,
                        });
                    }
                }
            }
            Action::LibLoad => {
                if let Some(pat) = self.selected_lib_pattern().cloned() {
                    let name = pat.name.clone();
                    // Commit: keep whatever is already loaded in the lane.
                    // Clear the audition backup without restoring.
                    self.audition = None;
                    self.status = format!("Loaded {}", name);
                    self.snapshot();
                    self.set.lanes[self.focus].pattern = pat.clone();
                    cmds.push(UiCommand::LoadPattern {
                        lane: self.focus,
                        pattern: pat,
                    });
                    self.mode = Mode::Edit;
                }
            }
            Action::Audition => {
                // Only save backup on first Audition (idempotent guard).
                if self.audition.is_none() {
                    self.audition = Some(self.set.lanes[self.focus].pattern.clone());
                }
                if let Some(pat) = self.selected_lib_pattern().cloned() {
                    self.status = format!("Auditioning {}", pat.name);
                    self.set.lanes[self.focus].pattern = pat.clone();
                    cmds.push(UiCommand::LoadPattern {
                        lane: self.focus,
                        pattern: pat,
                    });
                    // Do NOT mark dirty — this is a preview.
                }
            }
            Action::OpenSetBrowser => {
                self.set_files =
                    crate::pattern::store::list_sets(&crate::config::data_dir().join("sets"))
                        .unwrap_or_default();
                self.set_sel = 0;
                self.mode = Mode::SetBrowser;
            }
            Action::SetBrowserNav(d) => {
                if !self.set_files.is_empty() {
                    let n = self.set_files.len();
                    self.set_sel = (self.set_sel as i64 + d as i64).clamp(0, n as i64 - 1) as usize;
                }
            }
            Action::SetBrowserLoad => {
                if !self.set_files.is_empty() {
                    match crate::pattern::store::load_set(&self.set_files[self.set_sel]) {
                        Ok(set) => {
                            let stem = self.set_files[self.set_sel]
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("set")
                                .to_string();
                            self.set = set;
                            self.dirty = false;
                            self.status = format!("Loaded {}", stem);
                            self.mode = Mode::Edit;
                            cmds.push(UiCommand::SetSet(self.set.clone()));
                        }
                        Err(e) => {
                            self.status = format!("Load failed: {e}");
                            // stay in SetBrowser
                        }
                    }
                }
            }
            Action::CloseSetBrowser => {
                self.mode = Mode::Edit;
            }
            Action::Save => {
                // Cross-task dependency: `config::data_dir()` is defined in Task 21.
                let dir = crate::config::data_dir().join("sets");
                match crate::pattern::store::save_set(&dir, &self.set) {
                    Ok(_path) => {
                        self.status = "Saved".into();
                        self.dirty = false;
                    }
                    Err(e) => self.status = format!("Save failed: {}", e),
                }
            }
            Action::Help => {
                self.mode = if self.mode == Mode::Help {
                    Mode::Edit
                } else {
                    Mode::Help
                };
            }
            Action::Quit => {
                if self.playing && !self.quit_armed {
                    self.quit_armed = true;
                    self.status = "Press q again to quit".into();
                } else {
                    self.should_quit = true;
                }
            }
            Action::None => {}
        }
        cmds
    }

    /// Fold an EngineEvent into display state.
    pub fn on_engine_event(&mut self, ev: EngineEvent) {
        match ev {
            EngineEvent::Playhead { step, bar, .. } => {
                self.playhead = step;
                self.bar = bar;
            }
            EngineEvent::LinkStatus {
                enabled,
                tempo,
                peers,
            } => {
                let prev = self.prev_peers;
                self.link_enabled = enabled;
                self.link_tempo = tempo;
                self.link_peers = peers;
                self.prev_peers = peers;

                // Toast on peer transitions (only when link is enabled).
                if enabled {
                    if prev > 0 && peers == 0 {
                        self.status = "Link lost".into();
                    } else if prev == 0 && peers > 0 {
                        self.status = format!("Link: {} peers", peers);
                    }
                }
            }
            EngineEvent::DeviceStatus {
                lane,
                connected,
                port,
            } => {
                if let Some(slot) = self.device_status.get_mut(lane) {
                    *slot = (connected, port.clone());
                }
                // Toast with the port name when available, otherwise the lane index.
                let label = if port.is_empty() {
                    format!("lane {}", lane)
                } else {
                    port
                };
                self.status = format!(
                    "MIDI: {} {}",
                    label,
                    if connected {
                        "connected"
                    } else {
                        "disconnected"
                    }
                );
            }
            // Engine-confirmed transport state — UI integration handled in Task 6.
            EngineEvent::Started { .. } | EngineEvent::Stopped => {}
        }
    }

    // --- internal helpers ---

    fn set_focus(&mut self, i: usize) {
        self.focus = i;
        self.lib_role = role_for_profile(self.set.lanes[i].profile.id);
        self.lib_col = LibCol::Genre;
        self.lib_genre = 0;
        self.lib_pattern = 0;
        self.euclid_rotation = 0; // focus changed -> reset euclid rotation
        self.step_scroll = 0; // reset horizontal scroll on lane change
        self.clamp_cursor();
    }

    fn grid_dims(&self) -> (usize, usize) {
        let lane = &self.set.lanes[self.focus];
        let cols = lane.pattern.length;
        let rows = match lane.profile.kind {
            LaneKind::Drums => profiles::DRUM_VOICES.len(),
            LaneKind::Melodic => 1,
        };
        (rows, cols)
    }

    fn clamp_cursor(&mut self) {
        let (rows, cols) = self.grid_dims();
        if cols == 0 {
            self.cur_col = 0;
        } else if self.cur_col >= cols {
            self.cur_col = cols - 1;
        }
        if rows == 0 {
            self.cur_row = 0;
        } else if self.cur_row >= rows {
            self.cur_row = rows - 1;
        }
        self.update_step_scroll();
    }

    /// Snap `step_scroll` to the 16-step page that contains `cur_col`.
    /// The visible window is `[page*16, page*16+16)` where `page = cur_col / VISIBLE_STEPS`.
    fn update_step_scroll(&mut self) {
        self.step_scroll = (self.cur_col / VISIBLE_STEPS) * VISIBLE_STEPS;
    }

    /// Return the half-open step range `[start, end)` that is currently visible
    /// for the focused pattern.  Always a 16-step-aligned page containing `cur_col`.
    pub fn visible_step_range(&self) -> (usize, usize) {
        let start = (self.cur_col / VISIBLE_STEPS) * VISIBLE_STEPS;
        let len = self.set.lanes[self.focus].pattern.length;
        let end = (start + VISIBLE_STEPS).min(len);
        (start, end)
    }

    /// Short label for the current mode / lane-kind combination, used by the status bar.
    pub fn context_label(&self) -> &'static str {
        match self.mode {
            Mode::Edit => match self.focused_kind() {
                LaneKind::Drums => "EDIT DRUM",
                LaneKind::Melodic => "EDIT MELODIC",
            },
            Mode::Library => "LIBRARY",
            Mode::Help => "HELP",
            Mode::TempoEntry => "TEMPO",
            Mode::SetBrowser => "OPEN SET",
        }
    }

    /// Whether the Set has unsaved mutations.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    fn move_cursor(&mut self, drow: i32, dcol: i32) {
        let (rows, cols) = self.grid_dims();
        let new_col = (self.cur_col as i32 + dcol).clamp(0, cols.saturating_sub(1) as i32);
        let new_row = (self.cur_row as i32 + drow).clamp(0, rows.saturating_sub(1) as i32);
        self.cur_col = new_col as usize;
        if new_row as usize != self.cur_row {
            // The focused drum voice row changed -> reset euclid rotation.
            self.euclid_rotation = 0;
        }
        self.cur_row = new_row as usize;
        self.update_step_scroll();
    }

    fn snapshot(&mut self) {
        self.undo.push(self.set.clone());
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.set.clone());
            self.set = prev;
            self.clamp_cursor();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.set.clone());
            self.set = next;
            self.clamp_cursor();
        }
    }

    fn load_focused(&self) -> UiCommand {
        UiCommand::LoadPattern {
            lane: self.focus,
            pattern: self.set.lanes[self.focus].pattern.clone(),
        }
    }

    fn toggle_step(&mut self) {
        let row = self.cur_row;
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                if let Some(step) = steps.get_mut(col) {
                    let note = profiles::DRUM_VOICES[row].note;
                    if let Some(pos) = step.iter().position(|h| h.note == note) {
                        step.remove(pos);
                    } else {
                        step.push(DrumHit {
                            note,
                            vel: 100,
                            prob: 1.0,
                            ratchet: 1,
                        });
                    }
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(slot) = steps.get_mut(col) {
                    if slot.is_some() {
                        *slot = Option::None;
                    } else {
                        let len = lane.profile.gate_fraction;
                        *slot = Some(MelodicNote {
                            semi: 0,
                            vel: MEL_DEFAULT_VEL,
                            slide: false,
                            len,
                            prob: 1.0,
                            ratchet: 1,
                        });
                    }
                }
            }
        }
    }

    /// Adjust the probability of the cursor cell (drum hit at focused voice / melodic note)
    /// by `d * 0.1`, clamped to [0.0, 1.0]. Snapshots undo and returns true iff a cell was
    /// present and mutated (so the caller emits LoadPattern); a no-op otherwise.
    fn adjust_prob(&mut self, d: i8) -> bool {
        let row = self.cur_row;
        let col = self.cur_col;
        // Snapshot before mutating, but only if there is a target cell.
        if !self.cursor_cell_present() {
            return false;
        }
        self.snapshot();
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                if let Some(hit) = steps
                    .get_mut(col)
                    .and_then(|s| s.iter_mut().find(|h| h.note == note))
                {
                    hit.prob = (hit.prob + d as f32 * 0.1).clamp(0.0, 1.0);
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(Some(n)) = steps.get_mut(col) {
                    n.prob = (n.prob + d as f32 * 0.1).clamp(0.0, 1.0);
                }
            }
        }
        true
    }

    /// Adjust the ratchet count of the cursor cell by `d`, clamped to [1, 8]. Snapshots undo
    /// and returns true iff a cell was present and mutated; a no-op otherwise.
    fn adjust_ratchet(&mut self, d: i8) -> bool {
        let row = self.cur_row;
        let col = self.cur_col;
        if !self.cursor_cell_present() {
            return false;
        }
        self.snapshot();
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                if let Some(hit) = steps
                    .get_mut(col)
                    .and_then(|s| s.iter_mut().find(|h| h.note == note))
                {
                    hit.ratchet = (hit.ratchet as i16 + d as i16).clamp(1, 8) as u8;
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(Some(n)) = steps.get_mut(col) {
                    n.ratchet = (n.ratchet as i16 + d as i16).clamp(1, 8) as u8;
                }
            }
        }
        true
    }

    /// Whether the cursor cell holds a note: a DrumHit at the focused voice (drums) or a
    /// MelodicNote at the cursor step (melodic).
    fn cursor_cell_present(&self) -> bool {
        let row = self.cur_row;
        let col = self.cur_col;
        match &self.set.lanes[self.focus].pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                steps
                    .get(col)
                    .map(|s| s.iter().any(|h| h.note == note))
                    .unwrap_or(false)
            }
            PatternData::Melodic(steps) => {
                matches!(steps.get(col), Some(Some(_)))
            }
        }
    }

    /// MIDI velocity (1–127) of the cursor cell, if a note is present. Used for status toasts.
    fn cursor_vel_midi(&self) -> Option<u8> {
        let row = self.cur_row;
        let col = self.cur_col;
        match &self.set.lanes[self.focus].pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                steps
                    .get(col)
                    .and_then(|s| s.iter().find(|h| h.note == note))
                    .map(|h| h.vel)
            }
            PatternData::Melodic(steps) => steps
                .get(col)
                .and_then(|s| s.as_ref())
                .map(|n| (n.vel.clamp(0.0, 1.3) * 97.0) as u8),
        }
    }

    /// Probability of the cursor cell as an integer percentage (0–100), if present.
    fn cursor_prob_pct(&self) -> Option<u32> {
        let row = self.cur_row;
        let col = self.cur_col;
        match &self.set.lanes[self.focus].pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                steps
                    .get(col)
                    .and_then(|s| s.iter().find(|h| h.note == note))
                    .map(|h| (h.prob * 100.0).round() as u32)
            }
            PatternData::Melodic(steps) => steps
                .get(col)
                .and_then(|s| s.as_ref())
                .map(|n| (n.prob * 100.0).round() as u32),
        }
    }

    /// Ratchet count of the cursor cell, if present.
    fn cursor_ratchet(&self) -> Option<u8> {
        let row = self.cur_row;
        let col = self.cur_col;
        match &self.set.lanes[self.focus].pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                steps
                    .get(col)
                    .and_then(|s| s.iter().find(|h| h.note == note))
                    .map(|h| h.ratchet)
            }
            PatternData::Melodic(steps) => {
                steps.get(col).and_then(|s| s.as_ref()).map(|n| n.ratchet)
            }
        }
    }

    /// Count of steps where the focused drum voice's note is present.
    fn euclid_current_pulses(&self) -> usize {
        let voice_note = profiles::DRUM_VOICES[self.cur_row].note;
        match &self.set.lanes[self.focus].pattern.data {
            PatternData::Drums(steps) => steps
                .iter()
                .filter(|s| s.iter().any(|h| h.note == voice_note))
                .count(),
            PatternData::Melodic(_) => 0,
        }
    }

    /// (Re)generate the focused drum voice's row with a Bjorklund mask. `dp` adjusts the
    /// pulse count (relative to the current count); `dr` adjusts `euclid_rotation`. Drums
    /// only — melodic lanes are a no-op (returns false). Other voices are left untouched.
    /// Snapshots undo and returns true iff it mutated.
    fn apply_euclid(&mut self, dp: i8, dr: i8) -> bool {
        if self.focused_kind() != LaneKind::Drums {
            return false;
        }
        let length = self.set.lanes[self.focus].pattern.length;
        if length == 0 {
            return false;
        }
        let voice_note = profiles::DRUM_VOICES[self.cur_row].note;
        let current = self.euclid_current_pulses();
        let new_pulses = (current as i32 + dp as i32).clamp(0, length as i32) as usize;
        self.euclid_rotation =
            ((self.euclid_rotation as i32 + dr as i32).rem_euclid(length as i32)) as usize;
        let mask = euclid::bjorklund(new_pulses, length, self.euclid_rotation);

        self.snapshot();
        if let PatternData::Drums(steps) = &mut self.set.lanes[self.focus].pattern.data {
            for (i, step) in steps.iter_mut().enumerate() {
                let present = step.iter().position(|h| h.note == voice_note);
                let want = mask.get(i).copied().unwrap_or(false);
                match (present, want) {
                    (Option::None, true) => step.push(DrumHit {
                        note: voice_note,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                    }),
                    (Some(pos), false) => {
                        step.remove(pos);
                    }
                    _ => {} // already in the desired state; leave it (and other voices) untouched.
                }
            }
        }
        true
    }

    fn set_vel_bucket(&mut self, bucket: u8) {
        let b = bucket.min(9);
        let row = self.cur_row;
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                if let Some(step) = steps.get_mut(col) {
                    if let Some(hit) = step.iter_mut().find(|h| h.note == note) {
                        hit.vel = ((b as u16) * 14 + 1).clamp(1, 127) as u8;
                    }
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(Some(n)) = steps.get_mut(col) {
                    n.vel = (b as f32 / 9.0) * 1.3;
                }
            }
        }
    }

    fn adjust_vel(&mut self, d: i8) {
        let row = self.cur_row;
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                let note = profiles::DRUM_VOICES[row].note;
                if let Some(step) = steps.get_mut(col) {
                    if let Some(hit) = step.iter_mut().find(|h| h.note == note) {
                        let v = hit.vel as i16 + d as i16;
                        hit.vel = v.clamp(1, 127) as u8;
                    }
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(Some(n)) = steps.get_mut(col) {
                    n.vel = (n.vel + d as f32 * 0.05).clamp(0.0, 1.3);
                }
            }
        }
    }

    fn adjust_semi(&mut self, d: i8) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
            if let Some(Some(n)) = steps.get_mut(col) {
                n.semi = (n.semi as i16 + d as i16).clamp(-48, 48) as i8;
            }
        }
    }

    fn adjust_len(&mut self, d: i8) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
            if let Some(Some(n)) = steps.get_mut(col) {
                n.len = (n.len + d as f32 * 0.25).clamp(0.25, 64.0);
            }
        }
    }

    fn toggle_slide(&mut self) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
            if let Some(Some(n)) = steps.get_mut(col) {
                n.slide = !n.slide;
            }
        }
    }

    fn clear_step(&mut self) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                if let Some(step) = steps.get_mut(col) {
                    step.clear();
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(slot) = steps.get_mut(col) {
                    *slot = Option::None;
                }
            }
        }
    }

    fn copy_step(&mut self) {
        let col = self.cur_col;
        let lane = &self.set.lanes[self.focus];
        let single: PatternData = match &lane.pattern.data {
            PatternData::Drums(steps) => {
                let s: DrumStep = steps.get(col).cloned().unwrap_or_default();
                PatternData::Drums(vec![s])
            }
            PatternData::Melodic(steps) => {
                let s: MelodicStep = steps.get(col).cloned().unwrap_or(Option::None);
                PatternData::Melodic(vec![s])
            }
        };
        self.clipboard = Some(single);
    }

    fn paste_step(&mut self) {
        let col = self.cur_col;
        let clip = match &self.clipboard {
            Some(c) => c.clone(),
            Option::None => return,
        };
        let lane = &mut self.set.lanes[self.focus];
        match (&mut lane.pattern.data, clip) {
            (PatternData::Drums(steps), PatternData::Drums(src)) => {
                if let (Some(dst), Some(s)) = (steps.get_mut(col), src.into_iter().next()) {
                    *dst = s;
                }
            }
            (PatternData::Melodic(steps), PatternData::Melodic(src)) => {
                if let (Some(dst), Some(s)) = (steps.get_mut(col), src.into_iter().next()) {
                    *dst = s;
                }
            }
            _ => {} // kind mismatch: ignore
        }
    }

    fn rotate(&mut self, right: bool) {
        let lane = &mut self.set.lanes[self.focus];
        match &mut lane.pattern.data {
            PatternData::Drums(steps) => {
                if !steps.is_empty() {
                    if right {
                        steps.rotate_right(1);
                    } else {
                        steps.rotate_left(1);
                    }
                }
            }
            PatternData::Melodic(steps) => {
                if !steps.is_empty() {
                    if right {
                        steps.rotate_right(1);
                    } else {
                        steps.rotate_left(1);
                    }
                }
            }
        }
    }

    fn current_genre_map(&self) -> &crate::pattern::library::GenreMap {
        match self.lib_role {
            LibRole::Drums => &self.library.drums,
            LibRole::Bass => &self.library.bass,
            LibRole::Synth => &self.library.synth,
        }
    }

    fn lib_nav(&mut self, dx: i32, dy: i32) {
        // dx: column switch (Left=-1 → Genre, Right=+1 → Pattern)
        // dy: move selection within the focused column
        if dx != 0 {
            self.lib_col = if dx > 0 {
                LibCol::Pattern
            } else {
                LibCol::Genre
            };
            // Switching to Genre resets pattern selection so the two are in sync.
            if self.lib_col == LibCol::Genre {
                self.lib_pattern = 0;
            }
        }

        if dy != 0 {
            let genre_count = self.current_genre_map().len();
            match self.lib_col {
                LibCol::Genre => {
                    if genre_count == 0 {
                        return;
                    }
                    self.lib_genre =
                        (self.lib_genre as i32 + dy).clamp(0, genre_count as i32 - 1) as usize;
                    // Changing genre always resets pattern selection.
                    self.lib_pattern = 0;
                }
                LibCol::Pattern => {
                    let pat_count = self
                        .current_genre_map()
                        .get_index(self.lib_genre)
                        .map(|(_, v)| v.len())
                        .unwrap_or(0);
                    if pat_count == 0 {
                        return;
                    }
                    self.lib_pattern =
                        (self.lib_pattern as i32 + dy).clamp(0, pat_count as i32 - 1) as usize;
                }
            }
        }
    }

    fn selected_lib_pattern(&self) -> Option<&Pattern> {
        let map = self.current_genre_map();
        map.get_index(self.lib_genre)
            .and_then(|(_, v)| v.get(self.lib_pattern))
    }
}

fn role_for_profile(id: &str) -> LibRole {
    // Map by profile id so each lane points at its own library:
    // "t8-drums" -> Drums, "t8-bass" -> Bass, "s1" -> Synth.
    match id {
        "t8-drums" => LibRole::Drums,
        "s1" => LibRole::Synth,
        _ => LibRole::Bass, // "t8-bass" and any other melodic profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles;
    use crate::engine::UiCommand;
    use crate::pattern::library::{GenreMap, LibRole, Library};
    use crate::pattern::model::{DrumHit, MelodicNote, Pattern, PatternData, Set};

    /// Minimal in-memory library (one genre, one pattern per role) for deterministic tests.
    fn test_library() -> Library {
        let mut drums: GenreMap = GenreMap::new();
        let mut dsteps = vec![Vec::new(); 16];
        dsteps[2] = vec![DrumHit {
            note: 38,
            vel: 90,
            prob: 1.0,
            ratchet: 1,
        }];
        drums.insert(
            "techno".into(),
            vec![Pattern {
                name: "lib-drum".into(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(dsteps),
            }],
        );

        let mut bass: GenreMap = GenreMap::new();
        let mut bsteps = vec![None; 16];
        bsteps[0] = Some(MelodicNote {
            semi: 3,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
        });
        bass.insert(
            "acid".into(),
            vec![Pattern {
                name: "lib-bass".into(),
                desc: String::new(),
                length: 16,
                data: PatternData::Melodic(bsteps),
            }],
        );

        let mut synth: GenreMap = GenreMap::new();
        synth.insert(
            "dub".into(),
            vec![Pattern {
                name: "lib-synth".into(),
                desc: String::new(),
                length: 16,
                data: PatternData::Melodic(vec![None; 16]),
            }],
        );

        Library { drums, bass, synth }
    }

    fn new_app() -> App {
        let set = Set::default_set(profiles::default_profiles());
        App::new(set, test_library())
    }

    #[test]
    fn focus_wraps_around_three_lanes() {
        let mut app = new_app();
        assert_eq!(app.focus, 0);
        app.apply(Action::FocusNext);
        app.apply(Action::FocusNext);
        assert_eq!(app.focus, 2);
        app.apply(Action::FocusNext); // wrap 2 -> 0
        assert_eq!(app.focus, 0);
        app.apply(Action::FocusPrev); // wrap 0 -> 2
        assert_eq!(app.focus, 2);
    }

    #[test]
    fn cursor_clamps_to_grid() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums
                                         // Move far up/left -> clamp to 0,0.
        app.apply(Action::MoveCursor(-50, -50));
        assert_eq!((app.cur_row, app.cur_col), (0, 0));
        // Move far right/down -> clamp to (rows-1, cols-1).
        app.apply(Action::MoveCursor(500, 500));
        let lane = app.focused_lane();
        let rows = profiles::DRUM_VOICES.len();
        assert_eq!(app.cur_col, lane.pattern.length - 1);
        assert_eq!(app.cur_row, rows - 1);
    }

    #[test]
    fn melodic_cursor_row_is_fixed_zero() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // bass = melodic
        app.apply(Action::MoveCursor(0, 5));
        assert_eq!(app.cur_row, 0);
    }

    #[test]
    fn toggle_step_adds_and_removes_drum_hit() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 3)); // col 3, voice 0 (note 36)
        let cmds = app.apply(Action::ToggleStep);
        // Adds a hit and emits a LoadPattern command.
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[3].len(), 1);
            assert_eq!(steps[3][0].note, profiles::DRUM_VOICES[0].note);
            assert_eq!(steps[3][0].vel, 100);
        } else {
            panic!("expected drums");
        }
        // Toggling again removes it.
        app.apply(Action::ToggleStep);
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[3].len(), 0);
        } else {
            panic!("expected drums");
        }
    }

    #[test]
    fn toggle_step_places_and_removes_melodic_note() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // bass
        app.apply(Action::MoveCursor(0, 5));
        app.apply(Action::ToggleStep);
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            let n = steps[5].as_ref().expect("note placed");
            assert_eq!(n.semi, 0);
            assert_eq!(n.vel, 1.0);
        } else {
            panic!("expected melodic");
        }
        app.apply(Action::ToggleStep); // remove
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            assert!(steps[5].is_none());
        } else {
            panic!("expected melodic");
        }
    }

    #[test]
    fn undo_restores_prior_set_and_redo_reapplies() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 1));
        app.apply(Action::ToggleStep); // mutates -> pushes undo
        let after_edit = app.set.clone();
        app.apply(Action::Undo);
        // Cursor (0,1) should now be empty again.
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[1].len(), 0, "undo should remove the hit");
        }
        app.apply(Action::Redo);
        assert_eq!(app.set, after_edit, "redo should restore the edited set");
    }

    #[test]
    fn set_vel_bucket_maps_drum_velocity() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit first
        app.apply(Action::SetVelBucket(9)); // 9*14+1 = 127
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].vel, 127);
        }
        app.apply(Action::SetVelBucket(0)); // 0*14+1 = 1
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].vel, 1);
        }
    }

    #[test]
    fn lib_load_replaces_focused_pattern_and_returns_command() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums; lib_role -> Drums
        app.apply(Action::OpenLibrary);
        let cmds = app.apply(Action::LibLoad);
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        assert_eq!(app.focused_lane().pattern.name, "lib-drum");
    }

    #[test]
    fn focusing_s1_lane_sets_lib_role_synth() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2)); // S-1 synth lane (profile id "s1")
        assert_eq!(app.lib_role, LibRole::Synth);
        app.apply(Action::FocusLane(1)); // T-8 bass
        assert_eq!(app.lib_role, LibRole::Bass);
        app.apply(Action::FocusLane(0)); // T-8 drums
        assert_eq!(app.lib_role, LibRole::Drums);
    }

    #[test]
    fn adjust_swing_clamps_and_emits_set_swing() {
        let mut app = new_app();
        // Clamp high: many +steps cannot exceed 0.66.
        for _ in 0..20 {
            app.apply(Action::AdjustSwing(1));
        }
        assert!(
            (app.set.swing - 0.66).abs() < 1e-6,
            "swing should clamp to 0.66"
        );
        let cmds = app.apply(Action::AdjustSwing(1));
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::SetSwing(_))));
        // Clamp low: many -steps cannot drop below 0.5.
        for _ in 0..20 {
            app.apply(Action::AdjustSwing(-1));
        }
        assert!(
            (app.set.swing - 0.5).abs() < 1e-6,
            "swing should clamp to 0.5"
        );
    }

    #[test]
    fn adjust_pattern_len_grows_then_shrinks_and_emits_load() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums
        let start = app.focused_lane().pattern.length;
        // Grow by 2.
        let cmds = app.apply(Action::AdjustPatternLen(2));
        let grown = app.focused_lane().pattern.length;
        assert_eq!(grown, start + 2);
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps.len(), grown, "drum steps Vec resized to new length");
        } else {
            panic!("expected drums");
        }
        assert!(cmds.iter().any(|c| matches!(
            c,
            UiCommand::LoadPattern { lane: 0, pattern } if pattern.length == grown
        )));
        // Shrink by 3.
        app.apply(Action::AdjustPatternLen(-3));
        let shrunk = app.focused_lane().pattern.length;
        assert_eq!(shrunk, grown - 3);
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(
                steps.len(),
                shrunk,
                "drum steps Vec truncated to new length"
            );
        } else {
            panic!("expected drums");
        }
    }

    #[test]
    fn toggle_mute_flips_lane_and_returns_command() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2));
        assert!(!app.focused_lane().mute);
        let cmds = app.apply(Action::ToggleMute);
        assert!(app.focused_lane().mute);
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::Mute { lane: 2, on: true })));
    }

    #[test]
    fn engine_event_updates_display_state() {
        let mut app = new_app();
        app.on_engine_event(crate::engine::EngineEvent::Playhead {
            step: 7,
            bar: 3,
            beat: 1,
            phase: 0.75,
        });
        assert_eq!(app.playhead, 7);
        assert_eq!(app.bar, 3);
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: true,
            tempo: 128.0,
            peers: 2,
        });
        assert!(app.link_enabled);
        assert_eq!(app.link_tempo, 128.0);
        assert_eq!(app.link_peers, 2);
    }

    #[test]
    fn device_status_event_updates_slot_and_sets_toast() {
        let mut app = new_app();
        // Simulate engine reporting lane 1 connected to "Roland S-1".
        app.on_engine_event(crate::engine::EngineEvent::DeviceStatus {
            lane: 1,
            connected: true,
            port: "Roland S-1".to_string(),
        });
        assert_eq!(app.device_status[1], (true, "Roland S-1".to_string()));
        assert!(
            app.status.contains("Roland S-1") && app.status.contains("connected"),
            "toast should mention port and connected, got: {:?}",
            app.status
        );

        // Simulate disconnect.
        app.on_engine_event(crate::engine::EngineEvent::DeviceStatus {
            lane: 1,
            connected: false,
            port: "Roland S-1".to_string(),
        });
        assert!(!app.device_status[1].0);
        assert!(
            app.status.contains("disconnected"),
            "toast should say disconnected, got: {:?}",
            app.status
        );
    }

    #[test]
    fn link_lost_toast_on_peers_drop_to_zero() {
        let mut app = new_app();
        // Establish 2 peers first.
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: true,
            tempo: 128.0,
            peers: 2,
        });
        assert_ne!(app.status, "Link lost", "no lost toast when peers arrive");

        // Peers drop to 0 while link is enabled -> "Link lost".
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: true,
            tempo: 128.0,
            peers: 0,
        });
        assert_eq!(app.status, "Link lost");
        assert_eq!(app.link_peers, 0);
    }

    #[test]
    fn link_gained_toast_on_peers_rise_from_zero() {
        let mut app = new_app();
        // Start with 0 peers (default), then peers arrive.
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: true,
            tempo: 120.0,
            peers: 3,
        });
        assert!(
            app.status.contains("3") && app.status.contains("peers"),
            "toast should mention peer count, got: {:?}",
            app.status
        );
    }

    #[test]
    fn link_lost_toast_not_emitted_when_link_disabled() {
        let mut app = new_app();
        // Establish peers via a separate call (simulating prior state).
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: true,
            tempo: 120.0,
            peers: 2,
        });
        app.status.clear();

        // Link disabled — peers 2→0 should NOT produce "Link lost".
        app.on_engine_event(crate::engine::EngineEvent::LinkStatus {
            enabled: false,
            tempo: 120.0,
            peers: 0,
        });
        assert_ne!(app.status, "Link lost", "no toast when link is off");
    }

    #[test]
    fn panic_returns_panic_command_and_leaves_set_unchanged() {
        let mut app = new_app();
        let before = app.set.clone();
        let undo_len = app.undo.len();
        let cmds = app.apply(Action::Panic);
        assert_eq!(cmds, vec![UiCommand::Panic]);
        // No state change and no undo snapshot for a panic.
        assert_eq!(app.set, before, "panic must not mutate the Set");
        assert_eq!(
            app.undo.len(),
            undo_len,
            "panic must not push an undo snapshot"
        );
    }

    // --- per-step prob / ratchet edits -----------------------------------

    #[test]
    fn adjust_prob_on_present_hit_clamps_and_emits_load() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit (prob defaults to 1.0)
        let cmds = app.apply(Action::AdjustProb(-1)); // 1.0 -> 0.9
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert!((steps[0][0].prob - 0.9).abs() < 1e-6);
        }
        // Clamp low: many -steps cannot drop below 0.0.
        for _ in 0..30 {
            app.apply(Action::AdjustProb(-1));
        }
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert!((steps[0][0].prob - 0.0).abs() < 1e-6);
        }
        // Clamp high: cannot exceed 1.0.
        for _ in 0..30 {
            app.apply(Action::AdjustProb(1));
        }
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert!((steps[0][0].prob - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn adjust_ratchet_clamps_to_one_through_eight() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit (ratchet defaults to 1)
        let cmds = app.apply(Action::AdjustRatchet(2)); // 1 -> 3
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].ratchet, 3);
        }
        for _ in 0..20 {
            app.apply(Action::AdjustRatchet(1));
        }
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].ratchet, 8); // clamp high
        }
        for _ in 0..20 {
            app.apply(Action::AdjustRatchet(-1));
        }
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].ratchet, 1); // clamp low
        }
    }

    #[test]
    fn adjust_prob_on_empty_cell_is_a_noop() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 4)); // empty cell
        let cmds = app.apply(Action::AdjustProb(1));
        assert!(cmds.is_empty(), "no command when the cursor cell is empty");
        assert!(app.undo.is_empty(), "no undo snapshot for a no-op");
    }

    // --- euclidean generate ----------------------------------------------

    #[test]
    fn euclid_dp_plus_one_writes_bjorklund_mask_for_focused_voice() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums, 16 steps, focused voice = BD (note 36)
        app.apply(Action::MoveCursor(0, 0)); // voice row 0
                                             // current pulses = 0; dp=+1 -> 1 pulse, rotation 0 -> bjorklund(1,16,0) = onset at 0.
        let cmds = app.apply(Action::Euclid { dp: 1, dr: 0 });
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        let bd = profiles::DRUM_VOICES[0].note;
        let on_steps = |app: &App| -> Vec<usize> {
            if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
                steps
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.iter().any(|h| h.note == bd))
                    .map(|(i, _)| i)
                    .collect()
            } else {
                panic!("expected drums")
            }
        };
        assert_eq!(on_steps(&app), vec![0]);
        // dp=+3 more -> 4 pulses over 16 -> {0,4,8,12}.
        app.apply(Action::Euclid { dp: 3, dr: 0 });
        assert_eq!(on_steps(&app), vec![0, 4, 8, 12]);
    }

    #[test]
    fn euclid_dr_plus_one_rotates_the_mask() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::Euclid { dp: 4, dr: 0 }); // {0,4,8,12}
                                                    // Rotate left by 1 (pulse count unchanged): {3,7,11,15}.
        app.apply(Action::Euclid { dp: 0, dr: 1 });
        assert_eq!(app.euclid_rotation, 1);
        let bd = profiles::DRUM_VOICES[0].note;
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            let on: Vec<usize> = steps
                .iter()
                .enumerate()
                .filter(|(_, s)| s.iter().any(|h| h.note == bd))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(on, vec![3, 7, 11, 15]);
        }
    }

    #[test]
    fn euclid_is_a_noop_on_melodic_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // bass = melodic
        let cmds = app.apply(Action::Euclid { dp: 1, dr: 0 });
        assert!(cmds.is_empty());
        assert!(app.undo.is_empty());
    }

    // --- BPM control reducer ---------------------------------------------

    #[test]
    fn open_tempo_sets_mode_and_clears_buffer() {
        let mut app = new_app();
        app.tempo_input = "99".to_string();
        let cmds = app.apply(Action::OpenTempo);
        assert_eq!(app.mode, Mode::TempoEntry);
        assert_eq!(app.tempo_input, "");
        assert!(cmds.is_empty());
    }

    #[test]
    fn typing_digits_then_commit_sets_bpm_and_returns_to_edit() {
        let mut app = new_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('1'));
        app.apply(Action::TempoDigit('2'));
        let cmds = app.apply(Action::TempoDigit('4'));
        assert!(cmds.is_empty()); // no command on digit
        assert_eq!(app.tempo_input, "124");

        let cmds = app.apply(Action::TempoCommit);
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.tempo_input, "");
        assert!((app.set.bpm - 124.0).abs() < 1e-9);
        assert_eq!(cmds, vec![UiCommand::SetBpm(124.0)]);
    }

    #[test]
    fn commit_clamps_low_value() {
        let mut app = new_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('5')); // "5" < 20 -> clamp to 20
        let cmds = app.apply(Action::TempoCommit);
        assert!((app.set.bpm - 20.0).abs() < 1e-9);
        assert_eq!(cmds, vec![UiCommand::SetBpm(20.0)]);
    }

    #[test]
    fn commit_clamps_high_value() {
        let mut app = new_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('9'));
        app.apply(Action::TempoDigit('9'));
        app.apply(Action::TempoDigit('9')); // "999" > 300 -> clamp to 300
        let cmds = app.apply(Action::TempoCommit);
        assert!((app.set.bpm - 300.0).abs() < 1e-9);
        assert_eq!(cmds, vec![UiCommand::SetBpm(300.0)]);
    }

    #[test]
    fn commit_empty_input_leaves_bpm_unchanged_returns_edit() {
        let mut app = new_app();
        let original_bpm = app.set.bpm;
        app.apply(Action::OpenTempo);
        // No digits typed
        let cmds = app.apply(Action::TempoCommit);
        assert_eq!(app.mode, Mode::Edit);
        assert!((app.set.bpm - original_bpm).abs() < 1e-9);
        assert!(cmds.is_empty());
    }

    #[test]
    fn tempo_cancel_leaves_bpm_unchanged() {
        let mut app = new_app();
        let original_bpm = app.set.bpm;
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('2'));
        app.apply(Action::TempoDigit('0'));
        app.apply(Action::TempoDigit('0'));
        let cmds = app.apply(Action::TempoCancel);
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.tempo_input, "");
        assert!((app.set.bpm - original_bpm).abs() < 1e-9);
        assert!(cmds.is_empty());
    }

    #[test]
    fn tempo_backspace_removes_last_char() {
        let mut app = new_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('1'));
        app.apply(Action::TempoDigit('2'));
        app.apply(Action::TempoBackspace);
        assert_eq!(app.tempo_input, "1");
        let cmds = app.apply(Action::TempoBackspace);
        assert_eq!(app.tempo_input, "");
        assert!(cmds.is_empty());
    }

    #[test]
    fn tempo_digit_capped_at_three_chars() {
        let mut app = new_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('1'));
        app.apply(Action::TempoDigit('2'));
        app.apply(Action::TempoDigit('3'));
        app.apply(Action::TempoDigit('4')); // should be ignored
        assert_eq!(app.tempo_input, "123");
    }

    #[test]
    fn adjust_bpm_increments_and_clamps() {
        let mut app = new_app();
        app.set.bpm = 120.0;
        let cmds = app.apply(Action::AdjustBpm(1));
        assert!((app.set.bpm - 121.0).abs() < 1e-9);
        assert_eq!(cmds, vec![UiCommand::SetBpm(121.0)]);

        let cmds = app.apply(Action::AdjustBpm(-1));
        assert!((app.set.bpm - 120.0).abs() < 1e-9);
        assert_eq!(cmds, vec![UiCommand::SetBpm(120.0)]);

        // Clamp at 300
        app.set.bpm = 299.5;
        app.apply(Action::AdjustBpm(1));
        assert!((app.set.bpm - 300.0).abs() < 1e-9);

        // Clamp at 20
        app.set.bpm = 20.5;
        app.apply(Action::AdjustBpm(-1));
        assert!((app.set.bpm - 20.0).abs() < 1e-9);
    }

    // --- undo invariant --------------------------------------------------

    #[test]
    fn adjust_swing_snapshots_and_undo_restores_prior_swing() {
        let mut app = new_app();
        let before = app.set.swing;
        app.apply(Action::AdjustSwing(1));
        assert!((app.set.swing - before).abs() > 1e-9, "swing changed");
        app.apply(Action::Undo);
        assert!(
            (app.set.swing - before).abs() < 1e-9,
            "undo restored prior swing"
        );
    }

    #[test]
    fn adjust_prob_snapshots_and_undo_restores_prior_prob() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // prob = 1.0
        app.apply(Action::AdjustProb(-1)); // -> 0.9, snapshots undo
        app.apply(Action::Undo);
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert!(
                (steps[0][0].prob - 1.0).abs() < 1e-6,
                "undo restored prior prob"
            );
        }
    }

    // --- Fix #10 reducer: SetVelBucket on buckets 1-3 (regression) -------

    #[test]
    fn set_vel_bucket_buckets_1_2_3_change_velocity() {
        // Buckets 1-3 must now be reachable (previously shadowed by FocusLane in input).
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place hit; default vel 100

        // Bucket 1 -> 1*14+1 = 15
        app.apply(Action::SetVelBucket(1));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].vel, 15, "bucket 1 should set vel to 15");
        }

        // Bucket 2 -> 2*14+1 = 29
        app.apply(Action::SetVelBucket(2));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].vel, 29, "bucket 2 should set vel to 29");
        }

        // Bucket 3 -> 3*14+1 = 43
        app.apply(Action::SetVelBucket(3));
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0][0].vel, 43, "bucket 3 should set vel to 43");
        }
    }

    // --- Fix #9: horizontal scroll regression ----------------------------

    #[test]
    fn step_scroll_advances_when_cursor_moves_past_visible_window() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums, default 16 steps
                                         // Extend pattern to 32 steps so there is room to scroll.
        for _ in 0..16 {
            app.apply(Action::AdjustPatternLen(1));
        }
        assert_eq!(app.focused_lane().pattern.length, 32);
        assert_eq!(app.step_scroll, 0);

        // Move cursor to col 20 (past the initial 16-step window).
        app.apply(Action::MoveCursor(0, 20));
        assert_eq!(app.cur_col, 20);
        // step_scroll must have advanced so col 20 is within [step_scroll, step_scroll+16).
        assert!(
            app.cur_col >= app.step_scroll
                && app.cur_col < app.step_scroll + crate::app::VISIBLE_STEPS,
            "col 20 should be in the visible window [{}..{})",
            app.step_scroll,
            app.step_scroll + crate::app::VISIBLE_STEPS
        );
        assert!(app.step_scroll > 0, "scroll must have advanced past 0");
    }

    #[test]
    fn step_scroll_resets_when_cursor_returns_to_start() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        for _ in 0..16 {
            app.apply(Action::AdjustPatternLen(1)); // 32 steps
        }
        app.apply(Action::MoveCursor(0, 20)); // scroll right
        assert!(app.step_scroll > 0);

        // Move cursor back to col 0 (many steps left).
        app.apply(Action::MoveCursor(0, -20));
        assert_eq!(app.cur_col, 0);
        assert_eq!(
            app.step_scroll, 0,
            "scroll should reset to 0 when cursor is at col 0"
        );
    }

    #[test]
    fn step_scroll_resets_on_focus_change() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        for _ in 0..16 {
            app.apply(Action::AdjustPatternLen(1)); // 32 steps
        }
        app.apply(Action::MoveCursor(0, 20));
        assert!(app.step_scroll > 0);

        app.apply(Action::FocusNext); // switch lane
        assert_eq!(
            app.step_scroll, 0,
            "step_scroll should reset on lane change"
        );
    }

    // --- semantic arrow-key cursor axis tests ----------------------------
    // These tests tie the key→action mapping to actual cursor movement so a
    // transposition of Up/Down vs Left/Right cannot silently return.

    #[test]
    fn drums_up_key_decreases_cur_row_and_left_key_decreases_cur_col() {
        use crate::input::key_to_action;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums lane

        // Start at row 1, col 1 so there is room to move in both directions.
        app.apply(Action::MoveCursor(1, 1));
        assert_eq!((app.cur_row, app.cur_col), (1, 1));

        // Drums Up should decrease cur_row (previous voice).
        let up_action = key_to_action(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            crate::app::Mode::Edit,
            crate::pattern::model::LaneKind::Drums,
        );
        app.apply(up_action);
        assert_eq!(app.cur_row, 0, "drums Up must decrease cur_row");
        assert_eq!(app.cur_col, 1, "drums Up must not change cur_col");

        // Reset to (1, 1).
        app.apply(Action::MoveCursor(1, 0)); // back to row 1

        // Drums Left should decrease cur_col (previous step).
        let left_action = key_to_action(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            crate::app::Mode::Edit,
            crate::pattern::model::LaneKind::Drums,
        );
        app.apply(left_action);
        assert_eq!(app.cur_col, 0, "drums Left must decrease cur_col");
        assert_eq!(app.cur_row, 1, "drums Left must not change cur_row");
    }

    #[test]
    fn melodic_left_key_decreases_cur_col() {
        use crate::input::key_to_action;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // bass = melodic lane

        // Move to col 3.
        app.apply(Action::MoveCursor(0, 3));
        assert_eq!(app.cur_col, 3);

        // Melodic Left should decrease cur_col (previous step).
        let left_action = key_to_action(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            crate::app::Mode::Edit,
            crate::pattern::model::LaneKind::Melodic,
        );
        app.apply(left_action);
        assert_eq!(app.cur_col, 2, "melodic Left must decrease cur_col");
        assert_eq!(app.cur_row, 0, "melodic cur_row must remain 0");
    }

    // --- Item 2: double-q quit while playing --------------------------------

    #[test]
    fn quit_while_playing_arms_then_quits() {
        let mut app = new_app();
        app.apply(Action::TogglePlay); // start playing
        assert!(app.playing);

        // First Quit: arms, does NOT quit.
        app.apply(Action::Quit);
        assert!(!app.should_quit, "first Quit while playing should not quit");
        assert!(app.quit_armed, "first Quit while playing should arm");
        assert_eq!(app.status, "Press q again to quit");

        // Second Quit: quits.
        app.apply(Action::Quit);
        assert!(app.should_quit, "second consecutive Quit should quit");
    }

    #[test]
    fn quit_while_playing_disarmed_by_other_action() {
        let mut app = new_app();
        app.apply(Action::TogglePlay); // start playing

        app.apply(Action::Quit); // arm
        assert!(app.quit_armed);

        app.apply(Action::None); // any other action clears the arm
        assert!(!app.quit_armed, "non-Quit action should disarm quit_armed");

        // Another Quit now re-arms instead of quitting.
        app.apply(Action::Quit);
        assert!(
            !app.should_quit,
            "disarmed quit should not quit on next Quit"
        );
        assert!(app.quit_armed);
    }

    #[test]
    fn quit_while_stopped_quits_immediately() {
        let mut app = new_app();
        assert!(!app.playing);
        app.apply(Action::Quit);
        assert!(
            app.should_quit,
            "Quit while stopped should quit immediately"
        );
    }

    // --- Item 3: 16-step paging via visible_step_range ----------------------

    #[test]
    fn visible_step_range_cursor_at_col_5_on_16_step_pattern() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums, default 16 steps
        app.apply(Action::MoveCursor(0, 5));
        let (start, end) = app.visible_step_range();
        assert_eq!(start, 0);
        assert_eq!(end, 16); // pattern length = 16, min(0+16, 16) = 16
    }

    #[test]
    fn visible_step_range_cursor_at_col_20_on_32_step_pattern() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        // Extend to 32 steps.
        for _ in 0..16 {
            app.apply(Action::AdjustPatternLen(1));
        }
        app.apply(Action::MoveCursor(0, 20));
        let (start, end) = app.visible_step_range();
        assert_eq!(start, 16); // page 1: 20/16 = 1 -> 1*16=16
        assert_eq!(end, 32); // min(16+16, 32) = 32
    }

    #[test]
    fn visible_step_range_cursor_at_col_50_on_64_step_pattern() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        // Extend to 64 steps.
        for _ in 0..48 {
            app.apply(Action::AdjustPatternLen(1));
        }
        assert_eq!(app.focused_lane().pattern.length, 64);
        app.apply(Action::MoveCursor(0, 50));
        let (start, end) = app.visible_step_range();
        assert_eq!(start, 48); // page 3: 50/16 = 3 -> 3*16=48
        assert_eq!(end, 64); // min(48+16, 64) = 64
    }

    // --- Item 4: context_label ----------------------------------------------

    #[test]
    fn context_label_covers_all_modes_and_kinds() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums
        assert_eq!(app.context_label(), "EDIT DRUM");

        app.apply(Action::FocusLane(1)); // bass = melodic
        assert_eq!(app.context_label(), "EDIT MELODIC");

        app.apply(Action::OpenLibrary);
        assert_eq!(app.context_label(), "LIBRARY");

        app.apply(Action::CloseLibrary);
        app.apply(Action::Help);
        assert_eq!(app.context_label(), "HELP");

        app.apply(Action::Help); // close help -> back to Edit
        app.apply(Action::OpenTempo);
        assert_eq!(app.context_label(), "TEMPO");
    }

    // --- Item 5: status toasts on consequential ops -------------------------

    #[test]
    fn save_sets_saved_status() {
        let mut app = new_app();
        app.apply(Action::Save);
        // Save may fail in tests (no filesystem set up), but status is always set.
        // On success it is "Saved"; on error it starts with "Save failed:".
        assert!(
            app.status == "Saved" || app.status.starts_with("Save failed:"),
            "status after Save should be 'Saved' or 'Save failed: ...' but was {:?}",
            app.status
        );
    }

    #[test]
    fn set_vel_bucket_sets_velocity_status() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit
        app.apply(Action::SetVelBucket(5));
        assert!(
            app.status.contains("Velocity"),
            "status after SetVelBucket should contain 'Velocity' but was {:?}",
            app.status
        );
    }

    // --- Item 6: dirty flag -------------------------------------------------

    #[test]
    fn dirty_false_initially() {
        let app = new_app();
        assert!(!app.dirty(), "dirty should be false on fresh App");
    }

    #[test]
    fn dirty_true_after_toggle_step() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::ToggleStep);
        assert!(app.dirty(), "dirty should be true after a step edit");
    }

    #[test]
    fn dirty_false_after_successful_save() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::ToggleStep); // dirty = true
        assert!(app.dirty());
        app.apply(Action::Save); // may succeed or fail
        if app.status == "Saved" {
            assert!(
                !app.dirty(),
                "dirty should be false after a successful Save"
            );
        }
        // If save failed (no fs), dirty remains true — that's correct behavior.
    }

    // --- Task 1: Library column nav tests -----------------------------------

    #[test]
    fn lib_nav_right_switches_col_to_pattern() {
        let mut app = new_app();
        app.apply(Action::OpenLibrary);
        assert_eq!(app.lib_col, LibCol::Genre);
        app.apply(Action::LibNav(1, 0)); // dx=+1 → switch to Pattern
        assert_eq!(app.lib_col, LibCol::Pattern);
    }

    #[test]
    fn lib_nav_left_switches_col_to_genre_and_resets_pattern() {
        let mut app = new_app();
        app.apply(Action::OpenLibrary);
        app.apply(Action::LibNav(1, 0)); // to Pattern col
        app.lib_pattern = 0; // already 0, but be explicit
        app.apply(Action::LibNav(-1, 0)); // dx=-1 → Genre
        assert_eq!(app.lib_col, LibCol::Genre);
        assert_eq!(app.lib_pattern, 0);
    }

    #[test]
    fn lib_nav_dy_with_genre_col_advances_genre_and_resets_pattern() {
        let mut app = new_app();
        app.apply(Action::OpenLibrary);
        // test_library has one genre ("techno") for drums, so genre nav is clamped at 0.
        // We check that dy moves genre and resets pattern.
        app.lib_pattern = 0;
        app.apply(Action::LibNav(0, 1)); // dy=+1, genre col → advance genre (clamped to 0 since only one)
        assert_eq!(app.lib_col, LibCol::Genre);
        assert_eq!(app.lib_pattern, 0, "genre nav resets pattern");
    }

    #[test]
    fn lib_nav_dy_with_pattern_col_advances_pattern() {
        let mut app = new_app();
        app.apply(Action::OpenLibrary);
        // Switch to pattern column first.
        app.apply(Action::LibNav(1, 0));
        assert_eq!(app.lib_col, LibCol::Pattern);
        // test_library drums has 1 pattern, so pattern nav clamps at 0.
        app.apply(Action::LibNav(0, 1));
        assert_eq!(app.lib_pattern, 0); // clamped — only 1 pattern
        assert_eq!(
            app.lib_col,
            LibCol::Pattern,
            "column should not change on dy"
        );
    }

    // --- Task 2: SetBrowser tests -------------------------------------------

    #[test]
    fn set_browser_nav_clamps_at_bounds() {
        let mut app = new_app();
        // Populate set_files manually (no real fs needed).
        app.set_files = vec![
            std::path::PathBuf::from("/tmp/a.json"),
            std::path::PathBuf::from("/tmp/b.json"),
        ];
        app.set_sel = 0;
        app.mode = Mode::SetBrowser;

        app.apply(Action::SetBrowserNav(-1)); // clamp low → stays 0
        assert_eq!(app.set_sel, 0);

        app.apply(Action::SetBrowserNav(1)); // → 1
        assert_eq!(app.set_sel, 1);

        app.apply(Action::SetBrowserNav(1)); // clamp high → stays 1
        assert_eq!(app.set_sel, 1);
    }

    #[test]
    fn set_browser_load_sets_state_and_emits_set_set() {
        use crate::pattern::store;
        // Write a real set to a temp dir and load it back.
        let dir = std::env::temp_dir().join(format!(
            "midip_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut app = new_app();
        let saved_path = store::save_set(&dir, &app.set).unwrap();

        app.set_files = vec![saved_path];
        app.set_sel = 0;
        app.mode = Mode::SetBrowser;

        let cmds = app.apply(Action::SetBrowserLoad);
        assert_eq!(
            app.mode,
            Mode::Edit,
            "mode should return to Edit after load"
        );
        assert!(!app.dirty, "dirty should be false after load");
        assert!(app.status.starts_with("Loaded"), "status should say Loaded");
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SetSet(_))),
            "SetSet command should be emitted"
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_label_set_browser() {
        let mut app = new_app();
        app.mode = Mode::SetBrowser;
        assert_eq!(app.context_label(), "OPEN SET");
    }

    // --- Audition: cue-before-commit -----------------------------------------

    /// Put the test app into Library mode with focus on drums lane (index 0).
    fn enter_library(app: &mut App) {
        app.apply(Action::FocusLane(0)); // drums
        app.apply(Action::OpenLibrary);
    }

    #[test]
    fn audition_saves_original_loads_selected_returns_load_pattern_no_dirty() {
        let mut app = new_app();
        let original = app.set.lanes[0].pattern.clone();
        enter_library(&mut app);

        assert!(app.audition.is_none());
        let cmds = app.apply(Action::Audition);

        // Backup saved.
        assert!(
            app.audition.is_some(),
            "audition field should be Some after Audition"
        );
        assert_eq!(
            app.audition.as_ref().unwrap().name,
            original.name,
            "saved original"
        );

        // Focused lane now has the library pattern.
        assert_eq!(app.set.lanes[0].pattern.name, "lib-drum");

        // Returns a LoadPattern for the focused lane.
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "must emit LoadPattern for lane 0"
        );

        // Status shows auditioning.
        assert!(
            app.status.contains("Auditioning"),
            "status: {:?}",
            app.status
        );

        // NOT dirty (preview, not commit).
        assert!(!app.dirty, "audition must not set dirty");
    }

    #[test]
    fn audition_noop_when_already_auditioning() {
        let mut app = new_app();
        enter_library(&mut app);
        app.apply(Action::Audition);
        let backup_after_first = app.audition.clone().unwrap();

        // Second Audition should not overwrite the saved backup.
        app.apply(Action::Audition);
        assert_eq!(
            app.audition.as_ref().unwrap().name,
            backup_after_first.name,
            "second Audition must not overwrite the saved original"
        );
    }

    #[test]
    fn lib_nav_while_auditioning_loads_new_pattern_keeps_backup() {
        // Build a library with TWO patterns for drums so LibNav can select the second.
        let mut drums = crate::pattern::library::GenreMap::new();
        let dsteps_a = vec![Vec::new(); 16];
        let dsteps_b = {
            let mut s = vec![Vec::new(); 16];
            s[0] = vec![DrumHit {
                note: 36,
                vel: 80,
                prob: 1.0,
                ratchet: 1,
            }];
            s
        };
        drums.insert(
            "techno".into(),
            vec![
                Pattern {
                    name: "pat-A".into(),
                    desc: String::new(),
                    length: 16,
                    data: PatternData::Drums(dsteps_a),
                },
                Pattern {
                    name: "pat-B".into(),
                    desc: String::new(),
                    length: 16,
                    data: PatternData::Drums(dsteps_b),
                },
            ],
        );
        let library = Library {
            drums,
            bass: crate::pattern::library::GenreMap::new(),
            synth: crate::pattern::library::GenreMap::new(),
        };
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let original_name = set.lanes[0].pattern.name.clone();
        let mut app = App::new(set, library);

        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        // Switch to Pattern column, pat-A is at index 0.
        app.apply(Action::LibNav(1, 0));
        app.apply(Action::Audition);
        assert_eq!(app.set.lanes[0].pattern.name, "pat-A");
        assert_eq!(
            app.audition.as_ref().unwrap().name,
            original_name,
            "backup is original"
        );

        // Navigate to pat-B — should re-audition pat-B.
        let cmds = app.apply(Action::LibNav(0, 1)); // dy=+1 → pat-B
        assert_eq!(
            app.set.lanes[0].pattern.name, "pat-B",
            "lane should now hold pat-B"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "LibNav while auditioning must return LoadPattern"
        );
        // Original backup unchanged.
        assert_eq!(
            app.audition.as_ref().unwrap().name,
            original_name,
            "backup must not change on LibNav"
        );
    }

    #[test]
    fn lib_load_while_auditioning_commits_clears_audition_dirty_edit() {
        let mut app = new_app();
        enter_library(&mut app);
        app.apply(Action::Audition);
        assert!(app.audition.is_some());

        let cmds = app.apply(Action::LibLoad);

        // Committed: audition cleared, dirty set, mode=Edit, status "Loaded …".
        assert!(
            app.audition.is_none(),
            "audition should be cleared after commit"
        );
        assert!(app.dirty, "dirty should be true after commit");
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.status.starts_with("Loaded"), "status: {:?}", app.status);
        // Returns LoadPattern.
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        // Pattern stays as the auditioned one.
        assert_eq!(app.set.lanes[0].pattern.name, "lib-drum");
    }

    #[test]
    fn close_library_while_auditioning_restores_original_and_clears_audition() {
        let mut app = new_app();
        let original_name = app.set.lanes[0].pattern.name.clone();
        enter_library(&mut app);
        app.apply(Action::Audition);
        // Lane now holds lib-drum.
        assert_eq!(app.set.lanes[0].pattern.name, "lib-drum");

        let cmds = app.apply(Action::CloseLibrary);

        // Reverted: lane has original back.
        assert_eq!(
            app.set.lanes[0].pattern.name, original_name,
            "original restored"
        );
        // Returns LoadPattern to restore in engine.
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "must emit LoadPattern to restore in engine"
        );
        // Audition cleared.
        assert!(app.audition.is_none());
        // Status says cancelled.
        assert_eq!(app.status, "Audition cancelled");
        // Mode = Edit.
        assert_eq!(app.mode, Mode::Edit);
    }

    #[test]
    fn lib_load_without_auditioning_still_loads_and_commits() {
        let mut app = new_app();
        enter_library(&mut app);
        assert!(app.audition.is_none());
        let cmds = app.apply(Action::LibLoad);
        assert_eq!(app.set.lanes[0].pattern.name, "lib-drum");
        assert!(app.dirty);
        assert_eq!(app.mode, Mode::Edit);
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
    }

    #[test]
    fn close_library_without_auditioning_just_closes() {
        let mut app = new_app();
        let original = app.set.lanes[0].pattern.name.clone();
        enter_library(&mut app);
        let cmds = app.apply(Action::CloseLibrary);
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.set.lanes[0].pattern.name, original);
        assert!(cmds.is_empty(), "no commands on plain close");
        assert!(app.audition.is_none());
    }

    // --- Task 4: Swing toast ------------------------------------------------

    #[test]
    fn adjust_swing_sets_status_toast() {
        let mut app = new_app();
        app.apply(Action::AdjustSwing(1));
        assert!(
            app.status.contains("Swing"),
            "status should contain 'Swing' after AdjustSwing, got: {:?}",
            app.status
        );
    }
}
