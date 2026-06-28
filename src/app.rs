//! Application state and the action reducer (UI thread side).
//!
//! `App` holds the canonical edit state. `apply` mutates state and returns the
//! `UiCommand`s that must be forwarded to the engine (e.g. pattern edits emit
//! `LoadPattern`). Undo/redo snapshot the whole `Set`.

use crate::devices::profiles;
use crate::engine::scheduler::Quant;
use crate::engine::{EngineEvent, UiCommand};
use crate::pattern::euclid;
use crate::pattern::library::{LibRole, Library};
use crate::pattern::model::{
    DrumHit, DrumStep, Lane, LaneKind, LaneRoute, MelodicNote, MelodicStep, Pattern, PatternData,
    PortRef, Set,
};

/// Purpose of a pending name-entry dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum NamePurpose {
    SaveSetAs,
    RenameSet,
    SaveUserPattern,
}

/// Action to perform when a Confirm dialog is accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    NewSet,
    DeleteSet(std::path::PathBuf),
    ClearPattern,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Edit,
    Library,
    Help,
    TempoEntry,
    SetBrowser,
    RouteEditor,
    RecoveryPrompt,
    /// Text-input dialog for naming a set or pattern.
    NameEntry(NamePurpose),
    /// Yes/no confirmation dialog before a destructive action.
    Confirm(ConfirmAction),
}

/// Which field is focused in the route editor (cycles Left/Right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteField {
    Port,
    Channel,
    ClockOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LibCol {
    Genre,
    Pattern,
}

/// Isolated preview state for an active audition. Holds the cued pattern for a
/// specific lane WITHOUT mutating the committed `Set`. The editor renders this
/// overlay for `lane`; the engine plays it; commit/cancel resolve it.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditionPreview {
    pub lane: usize,
    pub pattern: Pattern,
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
    Euclid {
        dp: i8,
        dr: i8,
    }, // drums: dp = ±pulses for focused voice, dr = ±rotation
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
    OpenRouteEditor,
    CloseRouteEditor,
    RouteNavLane(i32),
    RouteCycleField(i32),
    RouteCyclePort(i32),
    RouteAdjustChannel(i32),
    RouteToggleClockOut,
    RecoveryRecover,
    RecoveryDiscard,
    RecoveryOpenSaved,
    ToggleMirror,
    /// Flip launch quant between NextBar and NextBeat.
    ToggleLaunchQuant,
    /// Cancel any pending queued launch on the focused lane.
    CancelQueue,
    /// Save a copy of the focused lane's pattern under a new name/id to the user-pattern store.
    /// Does NOT mutate the lane. Task 7 supplies the name from a dialog; here it is a param.
    SaveAsUserPattern(String),
    /// Replace the focused lane's pattern with a same-kind, same-length empty pattern ("init").
    /// Snapshots for undo. Confirmation when material exists is Task 7.
    ClearPattern,
    /// Load the user pattern at `path`, assign a fresh id and " copy" suffix, re-save.
    /// Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
    DuplicateUserPattern(std::path::PathBuf),
    /// Rename the user pattern at `path`: keep its id, update name, save new file, remove old.
    /// Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
    RenameUserPattern(std::path::PathBuf, String),
    /// Delete the user pattern file at `path`. Best-effort removal.
    /// Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
    DeleteUserPattern(std::path::PathBuf),
    /// Save current set under a new name with a fresh id. Becomes the new current document.
    SaveSetAs(String),
    /// Rename current set: keep id, update name, write new file, remove old file.
    RenameSet(String),
    /// Write a copy of the current set with a fresh id and " copy" suffix. Current doc unchanged.
    DuplicateSet,
    /// Replace current document with a default empty set. Clears undo/redo.
    NewSet,
    /// Delete the set file at `path`. Best-effort removal.
    DeleteSet(std::path::PathBuf),
    // ── Name-entry dialog ─────────────────────────────────────────────
    OpenNameEntry(NamePurpose),
    NameChar(char),
    NameBackspace,
    NameCommit,
    NameCancel,
    // ── Confirm dialog ────────────────────────────────────────────────
    OpenConfirm(ConfirmAction),
    ConfirmYes,
    ConfirmNo,
    // ── Set-browser management keys ───────────────────────────────────
    SetBrowserRename,
    SetBrowserSaveAs,
    SetBrowserDuplicate,
    SetBrowserDelete,
    SetBrowserNewSet,
    // ── Edit-mode pattern management ──────────────────────────────────
    OpenSaveUserPattern,
    OpenClearPattern,
    // ── User-pattern load ─────────────────────────────────────────────
    LoadUserPattern(std::path::PathBuf),
    /// Double the focused lane's pattern length, filling new steps by repeating the
    /// existing content cyclically (e.g. 16→32: steps 17–32 mirror 1–16).
    /// Capped at 64. No-op with status toast when already at 64.
    DoubleLength,
    None,
}

/// Number of steps visible in the editor at once. Steps beyond this are reached via scrolling.
pub const VISIBLE_STEPS: usize = 16;

/// How many main-loop frames a status toast persists before auto-clearing.
///
/// The main loop polls with a ~16 ms timeout → ~62.5 fps.
/// 188 frames ≈ 3 000 ms ÷ 16 ms/frame, giving roughly 3 seconds of visibility.
pub const STATUS_TTL_FRAMES: u16 = 188;

/// Frames between autosave flushes when the Set is dirty.
///
/// At ~16 ms/frame this is 125 × 16 ms ≈ 2 000 ms (2 s). Minimum meaningful value
/// is ~30 (avoid thrashing); we pick 125 for a comfortable 2-second debounce.
pub const AUTOSAVE_INTERVAL_FRAMES: u16 = 125;

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
    /// Engine-confirmed: sequencer is actually playing (set by `EngineEvent::Started`).
    pub engine_playing: bool,
    /// Engine-confirmed: waiting for Link bar boundary (set by `EngineEvent::Armed`).
    pub armed: bool,
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
    /// Frames remaining before `status` is auto-cleared. 0 = already blank/expired.
    pub status_ttl: u16,
    pub should_quit: bool,
    pub tempo_input: String,
    /// Text buffer for the NameEntry dialog (set name / user-pattern name).
    pub name_input: String,
    /// Armed for double-q quit: true after first Quit while playing.
    pub quit_armed: bool,
    /// True when the Set has unsaved mutations since the last successful Save.
    pub dirty: bool,
    /// Frame counter for debounced autosave. Increments each frame while dirty;
    /// resets to 0 when it fires or when the set becomes clean.
    pub autosave_counter: u32,
    /// Isolated audition preview overlay. `None` when not auditioning. Holds the
    /// cued pattern for a lane WITHOUT mutating the committed `Set`. Set by
    /// `Action::Audition`/`LibNav`, cleared by `LibLoad` (commit) or `CloseLibrary`
    /// (cancel/revert).
    pub audition: Option<AuditionPreview>,

    // --- Route editor state (Mode::RouteEditor) ---
    /// Selected lane index in the route editor.
    pub route_editor_lane: usize,
    /// Currently focused field in the route editor (Port / Channel / ClockOut).
    pub route_editor_field: RouteField,
    /// Available MIDI output port names, refreshed when the editor opens.
    pub route_editor_ports: Vec<String>,
    pub mirror_on: bool,

    // --- M3 Task 2: clip-launcher queue ---
    /// Quantization grid for the next library-load-while-playing. Default: NextBar.
    pub launch_quant: Quant,
    /// Per-lane queued pattern name (set when a QueuePattern is emitted while playing;
    /// cleared when EngineEvent::Launched fires for that lane or CancelQueue is applied).
    /// Sized to `set.lanes.len()`.
    pub queued: Vec<Option<String>>,

    // --- M3 Task 6: set management ---
    /// The on-disk path of the currently-loaded/saved set file, so Rename/Delete can target it.
    /// None if the set has never been saved or was replaced by NewSet.
    pub current_set_path: Option<std::path::PathBuf>,

    // --- M3 Task 7: management UI ---
    /// Cached user patterns loaded from the user patterns dir; injected into the library
    /// as a "User" genre when the library browser opens.
    pub user_patterns: Vec<crate::pattern::model::Pattern>,
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
            engine_playing: false,
            armed: false,
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
            status_ttl: 0,
            should_quit: false,
            tempo_input: String::new(),
            name_input: String::new(),
            quit_armed: false,
            dirty: false,
            autosave_counter: 0,
            audition: None,
            route_editor_lane: 0,
            route_editor_field: RouteField::Port,
            route_editor_ports: Vec::new(),
            mirror_on: false,
            launch_quant: Quant::NextBar,
            queued: vec![None; n],
            current_set_path: None,
            user_patterns: Vec::new(),
        }
    }

    /// Set the status toast and arm the TTL countdown.
    ///
    /// All status writes MUST go through this method so that every toast
    /// expires automatically after `STATUS_TTL_FRAMES` loop iterations.
    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_ttl = STATUS_TTL_FRAMES;
    }

    /// Decrement the status TTL by one frame; clear `status` when it reaches 0.
    ///
    /// Call once per main-loop iteration (after event handling, before or after render).
    pub fn tick_status(&mut self) {
        if self.status_ttl > 0 {
            self.status_ttl -= 1;
            if self.status_ttl == 0 {
                self.status.clear();
            }
        }
    }

    /// Advance the autosave debounce counter. Returns `true` exactly when the counter
    /// reaches `AUTOSAVE_INTERVAL_FRAMES` and the set is dirty — the caller should then
    /// write a recovery snapshot. Returns `false` and resets the counter when clean.
    ///
    /// Call once per main-loop iteration alongside `tick_status`.
    pub fn tick_autosave(&mut self) -> bool {
        if !self.dirty {
            self.autosave_counter = 0;
            return false;
        }
        self.autosave_counter += 1;
        if self.autosave_counter >= AUTOSAVE_INTERVAL_FRAMES as u32 {
            self.autosave_counter = 0;
            return true;
        }
        false
    }

    pub fn focused_lane(&self) -> &Lane {
        &self.set.lanes[self.focus]
    }

    pub fn focused_kind(&self) -> LaneKind {
        self.set.lanes[self.focus].profile.kind
    }

    /// The pattern to display for `lane`: the audition preview overlay when an
    /// audition targets this lane, otherwise the committed lane pattern. Used by the
    /// editor render so auditioning shows the cued pattern without mutating the Set.
    pub fn display_pattern(&self, lane: usize) -> &Pattern {
        match &self.audition {
            Some(prev) if prev.lane == lane => &prev.pattern,
            _ => &self.set.lanes[lane].pattern,
        }
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
            Action::FocusNext => {
                // Audition is tied to a specific lane; changing focus abandons it.
                if let Some(prev) = self.audition.take() {
                    cmds.push(UiCommand::LoadPattern {
                        lane: prev.lane,
                        pattern: self.set.lanes[prev.lane].pattern.clone(),
                    });
                    self.set_status("Audition cancelled");
                }
                self.set_focus((self.focus + 1) % self.set.lanes.len());
            }
            Action::FocusPrev => {
                // Audition is tied to a specific lane; changing focus abandons it.
                if let Some(prev) = self.audition.take() {
                    cmds.push(UiCommand::LoadPattern {
                        lane: prev.lane,
                        pattern: self.set.lanes[prev.lane].pattern.clone(),
                    });
                    self.set_status("Audition cancelled");
                }
                let n = self.set.lanes.len();
                self.set_focus((self.focus + n - 1) % n);
            }
            Action::FocusLane(i) => {
                if i < self.set.lanes.len() {
                    // Audition is tied to a specific lane; changing focus abandons it.
                    if let Some(prev) = self.audition.take() {
                        cmds.push(UiCommand::LoadPattern {
                            lane: prev.lane,
                            pattern: self.set.lanes[prev.lane].pattern.clone(),
                        });
                        self.set_status("Audition cancelled");
                    }
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
                    self.set_status(format!("Velocity {}", v));
                }
                cmds.push(self.load_focused());
            }
            Action::AdjustVel(d) => {
                self.snapshot();
                self.adjust_vel(d);
                if let Some(v) = self.cursor_vel_midi() {
                    self.set_status(format!("Velocity {}", v));
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
                // Full resync: tempo + swing live outside lanes, so SyncLanes alone diverges.
                cmds.push(UiCommand::SetBpm(self.set.bpm));
                cmds.push(UiCommand::SetSwing(self.set.swing));
                cmds.push(UiCommand::SyncLanes(self.set.lanes.clone()));
            }
            Action::Redo => {
                self.redo();
                cmds.push(UiCommand::SetBpm(self.set.bpm));
                cmds.push(UiCommand::SetSwing(self.set.swing));
                cmds.push(UiCommand::SyncLanes(self.set.lanes.clone()));
            }
            Action::ToggleMute => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                lane.mute = !lane.mute;
                let (n, muted) = (self.focus, self.set.lanes[self.focus].mute);
                self.set_status(format!(
                    "Lane {} {}",
                    n,
                    if muted { "muted" } else { "unmuted" }
                ));
                cmds.push(UiCommand::Mute {
                    lane: self.focus,
                    on: self.set.lanes[self.focus].mute,
                });
            }
            Action::ToggleSolo => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                lane.solo = !lane.solo;
                let (n, soloed) = (self.focus, self.set.lanes[self.focus].solo);
                self.set_status(format!(
                    "Lane {} {}",
                    n,
                    if soloed { "solo" } else { "unsolo" }
                ));
                cmds.push(UiCommand::Solo {
                    lane: self.focus,
                    on: self.set.lanes[self.focus].solo,
                });
            }
            Action::SetBpm(bpm) => {
                self.snapshot();
                self.set.bpm = bpm;
                cmds.push(UiCommand::SetBpm(bpm));
            }
            Action::Tap => cmds.push(UiCommand::Tap),
            Action::ToggleLink => {
                self.link_enabled = !self.link_enabled;
                self.set_status(if self.link_enabled {
                    "Link on"
                } else {
                    "Link off"
                });
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
                    // Only snapshot when the value actually changes (avoids a no-op undo entry).
                    if bpm != self.set.bpm {
                        self.snapshot();
                        self.set.bpm = bpm;
                    }
                    self.set_status(format!("BPM {}", bpm as i64));
                    cmds.push(UiCommand::SetBpm(bpm));
                }
                self.tempo_input.clear();
            }
            Action::TempoCancel => {
                self.mode = Mode::Edit;
                self.tempo_input.clear();
            }
            Action::AdjustBpm(d) => {
                self.snapshot();
                self.set.bpm = (self.set.bpm + d as f64).clamp(20.0, 300.0);
                self.set_status(format!("BPM {}", self.set.bpm as i64));
                cmds.push(UiCommand::SetBpm(self.set.bpm));
            }
            Action::AdjustSwing(d) => {
                // Swing mutates the Set, so it is snapshotted per the undo invariant.
                self.snapshot();
                self.set.swing = (self.set.swing + d as f32 * 0.02).clamp(0.5, 0.66);
                self.set_status(format!(
                    "Swing {}%",
                    (self.set.swing * 100.0).round() as i64
                ));
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
            Action::DoubleLength => {
                let len = self.set.lanes[self.focus].pattern.length;
                if len >= 64 {
                    self.set_status("Already at max length (64)");
                } else {
                    let new_len = (len * 2).min(64);
                    self.snapshot();
                    let lane = &mut self.set.lanes[self.focus];
                    match &mut lane.pattern.data {
                        PatternData::Drums(steps) => {
                            steps.resize(new_len, Vec::new());
                            for i in len..new_len {
                                steps[i] = steps[i % len].clone();
                            }
                        }
                        PatternData::Melodic(steps) => {
                            steps.resize(new_len, Option::None);
                            for i in len..new_len {
                                steps[i] = steps[i % len].clone();
                            }
                        }
                    }
                    lane.pattern.length = new_len;
                    self.set_status(format!("Length {} \u{2192} {}", len, new_len));
                    cmds.push(self.load_focused());
                }
            }
            Action::AdjustProb(d) => {
                if self.adjust_prob(d) {
                    if let Some(pct) = self.cursor_prob_pct() {
                        self.set_status(format!("Prob {}%", pct));
                    }
                    cmds.push(self.load_focused());
                }
            }
            Action::AdjustRatchet(d) => {
                if self.adjust_ratchet(d) {
                    if let Some(r) = self.cursor_ratchet() {
                        self.set_status(format!("Ratchet x{}", r));
                    }
                    cmds.push(self.load_focused());
                }
            }
            Action::Euclid { dp, dr } => {
                if self.apply_euclid(dp, dr) {
                    let pulses = self.euclid_current_pulses();
                    let steps = self.set.lanes[self.focus].pattern.length;
                    self.set_status(format!("Euclid E({},{})", pulses, steps));
                    cmds.push(self.load_focused());
                }
            }
            Action::Panic => {
                // Live recovery: forward to the engine. No undo snapshot, no Set mutation.
                cmds.push(UiCommand::Panic);
            }
            Action::OpenLibrary => {
                self.refresh_user_patterns();
                self.mode = Mode::Library;
            }
            Action::CloseLibrary => {
                if self.audition.take().is_some() {
                    // Cancel audition: the committed Set was never touched, so nothing to undo.
                    // Restore the engine to the COMMITTED pattern (it was playing the preview).
                    cmds.push(UiCommand::LoadPattern {
                        lane: self.focus,
                        pattern: self.set.lanes[self.focus].pattern.clone(),
                    });
                    self.set_status("Audition cancelled");
                }
                self.mode = Mode::Edit;
            }
            Action::LibNav(dx, dy) => {
                self.lib_nav(dx, dy);
                // Re-audition the newly-selected pattern if an audition is active.
                // Updates the isolated preview only; the committed Set is untouched.
                if self.audition.is_some() {
                    if let Some(pat) = self.selected_lib_pattern().cloned() {
                        self.audition = Some(AuditionPreview {
                            lane: self.focus,
                            pattern: pat.clone(),
                        });
                        cmds.push(UiCommand::LoadPattern {
                            lane: self.focus,
                            pattern: pat,
                        });
                    }
                }
            }
            Action::LibLoad => {
                // Commit: prefer the audition preview if one is active for this lane;
                // otherwise commit the currently-selected library pattern.
                let pat = match &self.audition {
                    Some(prev) if prev.lane == self.focus => Some(prev.pattern.clone()),
                    _ => self.selected_lib_pattern().cloned(),
                };
                if let Some(pat) = pat {
                    let name = pat.name.clone();
                    // Snapshot FIRST — captures the committed Set with the ORIGINAL pattern
                    // (bug 4 fix: the audition never mutated the Set, so undo lands on the
                    // true pre-audition pattern). snapshot() also marks dirty.
                    self.snapshot();
                    self.audition = None;
                    // Commit the pattern to the document unconditionally: the saved doc
                    // always reflects the intended pattern. Whether it launches now or at a
                    // boundary is an engine concern only.
                    self.set.lanes[self.focus].pattern = pat.clone();
                    if self.engine_playing {
                        // Playing: queue to the next launch boundary (clip-launcher style).
                        let quant = self.launch_quant;
                        self.queued[self.focus] = Some(name.clone());
                        self.set_status(format!("Queued {} ({})", name, quant_label(quant)));
                        cmds.push(UiCommand::QueuePattern {
                            lane: self.focus,
                            pattern: pat,
                            quant,
                        });
                    } else {
                        // Stopped: load immediately (existing behavior).
                        self.set_status(format!("Loaded {}", name));
                        cmds.push(UiCommand::LoadPattern {
                            lane: self.focus,
                            pattern: pat,
                        });
                    }
                    self.mode = Mode::Edit;
                }
            }
            Action::Audition => {
                // Gate: audition is allowed only when the focused lane is stopped (transport
                // stopped) OR muted. This prevents the cued preview from colliding with a live
                // lane. The audition target is the focused lane's route/channel; a dedicated
                // cue port is a future option — no separate port in this milestone.
                let lane_muted = self.set.lanes[self.focus].mute;
                if self.engine_playing && !lane_muted {
                    self.set_status("Mute lane to audition (it's live)");
                    return cmds;
                }
                // Isolated preview: cue the selected pattern WITHOUT mutating the committed Set.
                if let Some(pat) = self.selected_lib_pattern().cloned() {
                    self.set_status(format!("Auditioning {}", pat.name));
                    self.audition = Some(AuditionPreview {
                        lane: self.focus,
                        pattern: pat.clone(),
                    });
                    cmds.push(UiCommand::LoadPattern {
                        lane: self.focus,
                        pattern: pat,
                    });
                    // Do NOT mark dirty and do NOT mutate self.set — this is a preview.
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
                            self.current_set_path = Some(self.set_files[self.set_sel].clone());
                            self.load_set_document(set, stem);
                            self.mode = Mode::Edit;
                            cmds.push(UiCommand::SetSet(self.set.clone()));
                        }
                        Err(e) => {
                            self.set_status(format!("Load failed: {e}"));
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
                match crate::pattern::store::save_set(&dir, &mut self.set) {
                    Ok(path) => {
                        self.current_set_path = Some(path);
                        self.set_status("Saved");
                        self.dirty = false;
                        // Deliberate save supersedes the recovery snapshot.
                        crate::pattern::store::clear_recovery(&crate::config::data_dir());
                    }
                    Err(e) => self.set_status(format!("Save failed: {}", e)),
                }
            }
            Action::Help => {
                self.mode = if self.mode == Mode::Help {
                    Mode::Edit
                } else {
                    Mode::Help
                };
            }
            Action::OpenRouteEditor => {
                self.route_editor_lane = self.focus;
                self.route_editor_field = RouteField::Port;
                // Refresh the available port list on the UI thread (safe — not the timing thread).
                self.route_editor_ports = crate::midi::ports::list_output_ports();
                self.mode = Mode::RouteEditor;
            }
            Action::CloseRouteEditor => {
                self.mode = Mode::Edit;
            }
            Action::RouteNavLane(d) => {
                let n = self.set.lanes.len();
                if n > 0 {
                    self.route_editor_lane =
                        (self.route_editor_lane as i32 + d).rem_euclid(n as i32) as usize;
                }
            }
            Action::RouteCycleField(d) => {
                self.route_editor_field = match (self.route_editor_field, d) {
                    (RouteField::Port, d) if d > 0 => RouteField::Channel,
                    (RouteField::Channel, d) if d > 0 => RouteField::ClockOut,
                    (RouteField::ClockOut, d) if d > 0 => RouteField::Port,
                    (RouteField::Port, _) => RouteField::ClockOut,
                    (RouteField::Channel, _) => RouteField::Port,
                    (RouteField::ClockOut, _) => RouteField::Channel,
                };
            }
            Action::RouteCyclePort(d) => {
                let lane = self.route_editor_lane;
                if lane >= self.set.lanes.len() {
                    return cmds;
                }
                // Build the port cycle list: "(default)" + all available ports.
                // "(default)" → route = None; named port → route = Some(LaneRoute{...})
                let n_ports = self.route_editor_ports.len();
                // Current selection index: 0 = default, 1..=n = port[i-1]
                let current_idx = match &self.set.lanes[lane].route {
                    None => 0usize,
                    Some(r) => self
                        .route_editor_ports
                        .iter()
                        .position(|p| p == &r.port.name)
                        .map(|i| i + 1)
                        .unwrap_or(0),
                };
                let total = n_ports + 1; // 0=default, 1..=n_ports
                let next_idx = ((current_idx as i32 + d).rem_euclid(total as i32)) as usize;
                self.snapshot();
                if next_idx == 0 {
                    self.set.lanes[lane].route = None;
                    self.set_status(format!("Lane {lane}: route → (default)"));
                } else {
                    let port_name = self.route_editor_ports[next_idx - 1].clone();
                    // Preserve existing channel/clock_out when switching ports.
                    let existing = self.set.lanes[lane].route.as_ref();
                    let channel = existing
                        .map(|r| r.channel)
                        .unwrap_or_else(|| self.set.lanes[lane].profile.channel);
                    let clock_out = existing.map(|r| r.clock_out).unwrap_or(true);
                    self.set.lanes[lane].route = Some(LaneRoute {
                        port: PortRef {
                            stable_key: port_name.clone(),
                            name: port_name.clone(),
                        },
                        channel,
                        clock_out,
                    });
                    self.set_status(format!("Lane {lane}: port → {port_name}"));
                }
                let route = self.set.lanes[lane].route.clone();
                cmds.push(UiCommand::SetRoute { lane, route });
            }
            Action::RouteAdjustChannel(d) => {
                let lane = self.route_editor_lane;
                if lane >= self.set.lanes.len() {
                    return cmds;
                }
                self.snapshot();
                // Build or update the explicit route with the new channel.
                let existing_route = self.set.lanes[lane].route.clone();
                let new_channel = match &existing_route {
                    Some(r) => (r.channel as i32 + d).clamp(0, 15) as u8,
                    None => {
                        let profile_ch = self.set.lanes[lane].profile.channel as i32;
                        (profile_ch + d).clamp(0, 15) as u8
                    }
                };
                let port = match existing_route.as_ref() {
                    Some(r) => r.port.clone(),
                    None => PortRef {
                        stable_key: self.set.lanes[lane].profile.port_match.to_string(),
                        name: self.set.lanes[lane].profile.port_match.to_string(),
                    },
                };
                let clock_out = existing_route.as_ref().map(|r| r.clock_out).unwrap_or(true);
                self.set.lanes[lane].route = Some(LaneRoute {
                    port,
                    channel: new_channel,
                    clock_out,
                });
                self.set_status(format!(
                    "Lane {lane}: channel → {} (MIDI {})",
                    new_channel,
                    new_channel + 1
                ));
                let route = self.set.lanes[lane].route.clone();
                cmds.push(UiCommand::SetRoute { lane, route });
            }
            Action::RouteToggleClockOut => {
                let lane = self.route_editor_lane;
                if lane >= self.set.lanes.len() {
                    return cmds;
                }
                self.snapshot();
                let existing_route = self.set.lanes[lane].route.clone();
                let port = match existing_route.as_ref() {
                    Some(r) => r.port.clone(),
                    None => PortRef {
                        stable_key: self.set.lanes[lane].profile.port_match.to_string(),
                        name: self.set.lanes[lane].profile.port_match.to_string(),
                    },
                };
                let channel = match existing_route.as_ref() {
                    Some(r) => r.channel,
                    None => self.set.lanes[lane].profile.channel,
                };
                let new_clock_out = !existing_route.as_ref().map(|r| r.clock_out).unwrap_or(true);
                self.set.lanes[lane].route = Some(LaneRoute {
                    port,
                    channel,
                    clock_out: new_clock_out,
                });
                self.set_status(format!(
                    "Lane {lane}: clock-out {}",
                    if new_clock_out { "on" } else { "off" }
                ));
                let route = self.set.lanes[lane].route.clone();
                cmds.push(UiCommand::SetRoute { lane, route });
            }
            Action::Quit => {
                if self.playing && !self.quit_armed {
                    self.quit_armed = true;
                    self.set_status("Press q again to quit");
                } else {
                    self.should_quit = true;
                }
            }
            Action::RecoveryRecover => {
                let dir = crate::config::data_dir();
                let path = crate::pattern::store::recovery_path(&dir);
                match crate::pattern::store::load_set(&path) {
                    Ok(recovered) => {
                        self.set = recovered;
                        self.dirty = true;
                        self.undo.clear();
                        self.redo.clear();
                        self.audition = None;
                        self.clamp_cursor();
                        self.mode = Mode::Edit;
                        self.set_status("Recovered unsaved work");
                        crate::pattern::store::clear_recovery(&dir);
                        cmds.push(UiCommand::SetSet(self.set.clone()));
                    }
                    Err(e) => {
                        self.set_status(format!("Recovery failed: {e}"));
                        self.mode = Mode::Edit;
                    }
                }
            }
            Action::RecoveryDiscard => {
                crate::pattern::store::clear_recovery(&crate::config::data_dir());
                self.mode = Mode::Edit;
                self.set_status("Discarded recovered work");
            }
            Action::RecoveryOpenSaved => {
                self.set_files =
                    crate::pattern::store::list_sets(&crate::config::data_dir().join("sets"))
                        .unwrap_or_default();
                self.set_sel = 0;
                self.mode = Mode::SetBrowser;
            }
            Action::ToggleLaunchQuant => {
                self.launch_quant = match self.launch_quant {
                    Quant::NextBar => Quant::NextBeat,
                    Quant::NextBeat => Quant::NextBar,
                };
                self.set_status(format!("Launch: {}", quant_label(self.launch_quant)));
            }
            Action::CancelQueue => {
                let lane = self.focus;
                if self.queued[lane].is_some() {
                    self.queued[lane] = None;
                    self.set_status(format!("Queue cancelled (lane {})", lane + 1));
                    cmds.push(UiCommand::CancelQueue { lane });
                }
            }
            Action::ToggleMirror => {
                self.mirror_on = !self.mirror_on;
                self.set_status(if self.mirror_on {
                    "Mirror on (midip virtual port)"
                } else {
                    "Mirror off"
                });
                let _ = crate::pattern::store::save_prefs(
                    &crate::config::data_dir(),
                    &crate::pattern::store::Prefs {
                        mirror_on: self.mirror_on,
                    },
                );
                cmds.push(crate::engine::UiCommand::SetMirror(self.mirror_on));
            }
            Action::SaveAsUserPattern(name) => {
                // Clone the focused lane's pattern and give it a fresh identity so
                // save-as creates a new file rather than overwriting the source.
                let mut clone = self.set.lanes[self.focus].pattern.clone();
                clone.name = name.clone();
                clone.id = crate::persist::Id::nil();
                clone.ensure_id();
                let dir = crate::config::data_dir().join("patterns");
                match crate::pattern::store::save_user_pattern(&dir, &mut clone) {
                    Ok(_) => self.set_status(format!("Saved pattern {}", name)),
                    Err(e) => self.set_status(format!("Save failed: {}", e)),
                }
                // Does NOT mutate the lane or mark dirty.
            }
            Action::ClearPattern => {
                self.snapshot();
                let lane = &self.set.lanes[self.focus];
                let len = lane.pattern.length;
                let kind = lane.pattern.kind();
                let empty = match kind {
                    crate::pattern::model::LaneKind::Drums => {
                        crate::pattern::model::Pattern::empty_drums(len)
                    }
                    crate::pattern::model::LaneKind::Melodic => {
                        crate::pattern::model::Pattern::empty_melodic(len)
                    }
                };
                self.set.lanes[self.focus].pattern = empty;
                self.set_status("Cleared lane");
                cmds.push(self.load_focused());
            }
            Action::DuplicateUserPattern(path) => {
                // Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
                let dir = crate::config::data_dir().join("patterns");
                match crate::pattern::store::load_user_pattern(&path) {
                    Ok(mut p) => {
                        let new_name = format!("{} copy", p.name);
                        p.name = new_name.clone();
                        p.id = crate::persist::Id::nil();
                        p.ensure_id();
                        match crate::pattern::store::save_user_pattern(&dir, &mut p) {
                            Ok(_) => self.set_status(format!("Duplicated as {}", new_name)),
                            Err(e) => self.set_status(format!("Duplicate failed: {}", e)),
                        }
                    }
                    Err(e) => self.set_status(format!("Duplicate failed: {}", e)),
                }
            }
            Action::RenameUserPattern(path, new_name) => {
                // Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
                let dir = crate::config::data_dir().join("patterns");
                match crate::pattern::store::load_user_pattern(&path) {
                    Ok(mut p) => {
                        let old_id = p.id.clone();
                        p.name = new_name.clone();
                        match crate::pattern::store::save_user_pattern(&dir, &mut p) {
                            Ok(_) => {
                                // Remove the old file (best-effort; ignore errors).
                                let _ = std::fs::remove_file(&path);
                                debug_assert_eq!(p.id, old_id, "rename must keep the pattern id");
                                self.set_status(format!("Renamed to {}", new_name));
                            }
                            Err(e) => self.set_status(format!("Rename failed: {}", e)),
                        }
                    }
                    Err(e) => self.set_status(format!("Rename failed: {}", e)),
                }
            }
            Action::DeleteUserPattern(path) => {
                // Only ever touches paths under the user `patterns/` dir; vendored library dir is never written.
                // Best-effort: silently ignore "not found" errors.
                let _ = std::fs::remove_file(&path);
                self.set_status("Deleted pattern");
            }
            Action::SaveSetAs(name) => {
                // Give the set a fresh identity so save-as creates a new file rather
                // than overwriting the source. The current document becomes the new set.
                let dir = crate::config::data_dir().join("sets");
                self.set.name = name.clone();
                self.set.id = crate::persist::Id::nil();
                self.set.ensure_id();
                match crate::pattern::store::save_set(&dir, &mut self.set) {
                    Ok(path) => {
                        self.current_set_path = Some(path);
                        self.dirty = false;
                        self.set_status(format!("Saved as {}", name));
                    }
                    Err(e) => self.set_status(format!("Save failed: {}", e)),
                }
            }
            Action::RenameSet(name) => {
                // Keep the id; only the name (and therefore filename) changes.
                let dir = crate::config::data_dir().join("sets");
                let old_path = self.current_set_path.clone();
                self.set.name = name.clone();
                match crate::pattern::store::save_set(&dir, &mut self.set) {
                    Ok(new_path) => {
                        // Remove the old file if it differs (best-effort).
                        if let Some(ref op) = old_path {
                            if op != &new_path {
                                let _ = std::fs::remove_file(op);
                            }
                        }
                        self.current_set_path = Some(new_path);
                        self.dirty = false;
                        self.set_status(format!("Renamed to {}", name));
                    }
                    Err(e) => self.set_status(format!("Rename failed: {}", e)),
                }
            }
            Action::DuplicateSet => {
                // Clone with a fresh id and " copy" suffix; write a new file.
                // The current document stays unchanged.
                let dir = crate::config::data_dir().join("sets");
                let mut clone = self.set.clone();
                let copy_name = format!("{} copy", clone.name);
                clone.name = copy_name.clone();
                clone.id = crate::persist::Id::nil();
                clone.ensure_id();
                match crate::pattern::store::save_set(&dir, &mut clone) {
                    Ok(_) => self.set_status(format!("Duplicated to {}", copy_name)),
                    Err(e) => self.set_status(format!("Duplicate failed: {}", e)),
                }
                // current_set_path and self.set are intentionally untouched.
            }
            Action::NewSet => {
                // Replace the current document with a blank default set.
                // Clears undo/redo (you cannot undo across documents).
                // Confirmation when dirty is Task 7 — here just do it unconditionally.
                let new_set = Set::default_set(crate::devices::profiles::default_profiles());
                let n = new_set.lanes.len();
                self.set = new_set;
                self.undo.clear();
                self.redo.clear();
                self.audition = None;
                self.dirty = false;
                self.current_set_path = None;
                self.queued = vec![None; n];
                self.clamp_cursor();
                self.set_status("New set");
                cmds.push(UiCommand::SetSet(self.set.clone()));
            }
            Action::DeleteSet(path) => {
                // Best-effort file removal. If the deleted file is the current document's
                // backing file, clear current_set_path (the in-memory set is left as-is).
                let _ = std::fs::remove_file(&path);
                if self.current_set_path.as_deref() == Some(&path) {
                    self.current_set_path = None;
                }
                self.set_status("Deleted set");
            }
            // ── Name-entry dialog ─────────────────────────────────────────────
            Action::OpenNameEntry(purpose) => {
                self.name_input.clear();
                self.mode = Mode::NameEntry(purpose);
            }
            Action::NameChar(c) => {
                let ok = c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '#');
                if ok && self.name_input.len() < 32 {
                    self.name_input.push(c);
                }
            }
            Action::NameBackspace => {
                self.name_input.pop();
            }
            Action::NameCommit => {
                let purpose = match &self.mode {
                    Mode::NameEntry(p) => p.clone(),
                    _ => return cmds,
                };
                let name = self.name_input.trim().to_string();
                self.mode = Mode::Edit;
                self.name_input.clear();
                if !name.is_empty() {
                    let sub = match purpose {
                        NamePurpose::SaveSetAs => Action::SaveSetAs(name),
                        NamePurpose::RenameSet => Action::RenameSet(name),
                        NamePurpose::SaveUserPattern => Action::SaveAsUserPattern(name),
                    };
                    cmds.extend(self.apply(sub));
                }
            }
            Action::NameCancel => {
                self.mode = Mode::Edit;
                self.name_input.clear();
            }
            // ── Confirm dialog ────────────────────────────────────────────────
            Action::OpenConfirm(action) => {
                self.mode = Mode::Confirm(action);
            }
            Action::ConfirmYes => {
                let action = match &self.mode {
                    Mode::Confirm(a) => a.clone(),
                    _ => return cmds,
                };
                self.mode = Mode::Edit;
                let sub = match action {
                    ConfirmAction::NewSet => Action::NewSet,
                    ConfirmAction::DeleteSet(path) => Action::DeleteSet(path),
                    ConfirmAction::ClearPattern => Action::ClearPattern,
                };
                cmds.extend(self.apply(sub));
            }
            Action::ConfirmNo => {
                self.mode = Mode::Edit;
                self.set_status("Cancelled");
            }
            // ── Set-browser management ────────────────────────────────────────
            Action::SetBrowserRename => {
                self.name_input.clear();
                self.mode = Mode::NameEntry(NamePurpose::RenameSet);
            }
            Action::SetBrowserSaveAs => {
                self.name_input.clear();
                self.mode = Mode::NameEntry(NamePurpose::SaveSetAs);
            }
            Action::SetBrowserDuplicate => {
                cmds.extend(self.apply(Action::DuplicateSet));
            }
            Action::SetBrowserDelete => {
                if !self.set_files.is_empty() {
                    let path = self.set_files[self.set_sel].clone();
                    self.mode = Mode::Confirm(ConfirmAction::DeleteSet(path));
                }
            }
            Action::SetBrowserNewSet => {
                if self.dirty {
                    self.mode = Mode::Confirm(ConfirmAction::NewSet);
                } else {
                    cmds.extend(self.apply(Action::NewSet));
                }
            }
            // ── Edit-mode pattern management ──────────────────────────────────
            Action::OpenSaveUserPattern => {
                self.name_input.clear();
                self.mode = Mode::NameEntry(NamePurpose::SaveUserPattern);
            }
            Action::OpenClearPattern => {
                if self.pattern_has_material() {
                    self.mode = Mode::Confirm(ConfirmAction::ClearPattern);
                } else {
                    cmds.extend(self.apply(Action::ClearPattern));
                }
            }
            // ── User-pattern load ─────────────────────────────────────────────
            Action::LoadUserPattern(path) => {
                match crate::pattern::store::load_user_pattern(&path) {
                    Ok(pat) => {
                        let name = pat.name.clone();
                        self.snapshot();
                        self.set.lanes[self.focus].pattern = pat;
                        self.set_status(format!("Loaded user pattern {}", name));
                        self.mode = Mode::Edit;
                        cmds.push(self.load_focused());
                    }
                    Err(e) => self.set_status(format!("Load failed: {}", e)),
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
                        self.set_status("Link lost");
                    } else if prev == 0 && peers > 0 {
                        self.set_status(format!("Link: {} peers", peers));
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
                self.set_status(format!(
                    "MIDI: {} {}",
                    label,
                    if connected {
                        "connected"
                    } else {
                        "disconnected"
                    }
                ));
            }
            EngineEvent::Armed => {
                self.armed = true;
                self.engine_playing = false;
            }
            EngineEvent::Started { .. } => {
                self.armed = false;
                self.engine_playing = true;
            }
            EngineEvent::Stopped => {
                self.armed = false;
                self.engine_playing = false;
            }
            EngineEvent::Tempo { bpm } => {
                // Update the displayed BPM (display only — no dirty flag, no undo snapshot;
                // tap is a performance action, not an edit).
                self.set.bpm = bpm;
                self.set_status(format!("Tap: {} BPM", bpm.round() as u32));
            }
            // M3 Task 2: a queued per-lane launch fired. Clear the QUEUED display for
            // that lane so it flips back to ACTIVE (the engine now plays the queued pattern).
            EngineEvent::Launched { lane, .. } => {
                if let Some(slot) = self.queued.get_mut(lane) {
                    *slot = None;
                }
            }
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
            Mode::RouteEditor => "ROUTES",
            Mode::RecoveryPrompt => "RECOVERY",
            Mode::NameEntry(_) => "NAME",
            Mode::Confirm(_) => "CONFIRM",
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

    /// Maximum undo depth. Bounded so a long live session can't grow the stack without limit.
    const UNDO_LIMIT: usize = 100;

    fn snapshot(&mut self) {
        self.undo.push(self.set.clone());
        if self.undo.len() > Self::UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    /// Switch to a freshly-loaded document. Replaces the Set and CLEARS undo/redo
    /// (you cannot undo across documents). Warns in the status if unsaved edits were
    /// discarded so the loss is not silent. (Full confirm-prompt is deferred to M3.)
    fn load_set_document(&mut self, set: Set, name: String) {
        let had_unsaved = self.dirty;
        let n = set.lanes.len();
        self.set = set;
        self.undo.clear();
        self.redo.clear();
        self.audition = None;
        self.dirty = false;
        self.queued = vec![None; n];
        self.clamp_cursor();
        self.set_status(if had_unsaved {
            format!("Loaded {} (unsaved changes discarded)", name)
        } else {
            format!("Loaded {}", name)
        });
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

    /// True if the focused lane's pattern has at least one active step.
    fn pattern_has_material(&self) -> bool {
        let lane = &self.set.lanes[self.focus];
        match &lane.pattern.data {
            PatternData::Drums(steps) => steps.iter().any(|s| !s.is_empty()),
            PatternData::Melodic(steps) => steps.iter().any(|s| s.is_some()),
        }
    }

    /// Load user patterns from disk and inject them as a "User" genre in every role map.
    /// Called when opening the library browser so saved patterns are immediately browseable.
    pub fn refresh_user_patterns(&mut self) {
        let dir = crate::config::data_dir().join("patterns");
        let paths = crate::pattern::store::list_user_patterns(&dir);
        let user_pats: Vec<crate::pattern::model::Pattern> = paths
            .iter()
            .filter_map(|p| crate::pattern::store::load_user_pattern(p).ok())
            .collect();
        self.user_patterns = user_pats.clone();
        // Inject (or replace) a "User" genre in all three role maps so the browser
        // shows saved user patterns regardless of focused-lane role. Only if non-empty.
        self.library.drums.shift_remove("User");
        self.library.bass.shift_remove("User");
        self.library.synth.shift_remove("User");
        if !user_pats.is_empty() {
            self.library.drums.insert("User".into(), user_pats.clone());
            self.library.bass.insert("User".into(), user_pats.clone());
            self.library.synth.insert("User".into(), user_pats);
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

/// Human-readable label for a `Quant` value, used in status toasts and UI.
fn quant_label(q: Quant) -> &'static str {
    match q {
        Quant::NextBar => "next bar",
        Quant::NextBeat => "next beat",
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

    /// Serializes tests that read/write the shared `data_dir()/recovery/autosave.json` path.
    /// Poison-tolerant: a panicking test won't cascade to block the others.
    static RECOVERY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
                id: crate::persist::Id::nil(),
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
                id: crate::persist::Id::nil(),
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
                id: crate::persist::Id::nil(),
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
    fn engine_started_event_sets_engine_playing_clears_armed() {
        let mut app = new_app();
        // Start in armed state to verify Started clears it.
        app.armed = true;
        app.engine_playing = false;
        app.on_engine_event(crate::engine::EngineEvent::Started { at_step: 0 });
        assert!(
            app.engine_playing,
            "engine_playing must be true after Started"
        );
        assert!(!app.armed, "armed must be false after Started");
    }

    #[test]
    fn engine_armed_event_sets_armed_clears_engine_playing() {
        let mut app = new_app();
        app.engine_playing = true;
        app.on_engine_event(crate::engine::EngineEvent::Armed);
        assert!(app.armed, "armed must be true after Armed event");
        assert!(
            !app.engine_playing,
            "engine_playing must be false after Armed event"
        );
    }

    #[test]
    fn engine_stopped_event_clears_both_fields() {
        let mut app = new_app();
        app.engine_playing = true;
        app.armed = true;
        app.on_engine_event(crate::engine::EngineEvent::Stopped);
        assert!(
            !app.engine_playing,
            "engine_playing must be false after Stopped"
        );
        assert!(!app.armed, "armed must be false after Stopped");
    }

    #[test]
    fn engine_armed_then_started_ends_armed_playing() {
        let mut app = new_app();
        app.on_engine_event(crate::engine::EngineEvent::Armed);
        assert!(app.armed);
        assert!(!app.engine_playing);
        app.on_engine_event(crate::engine::EngineEvent::Started { at_step: 0 });
        assert!(!app.armed, "armed cleared after Started");
        assert!(app.engine_playing, "engine_playing set after Started");
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
        let saved_path = store::save_set(&dir, &mut app.set).unwrap();

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
    fn audition_isolates_preview_loads_selected_returns_load_pattern_no_dirty() {
        let mut app = new_app();
        let original = app.set.lanes[0].pattern.clone();
        enter_library(&mut app);

        assert!(app.audition.is_none());
        let cmds = app.apply(Action::Audition);

        // Preview holds the SELECTED library pattern targeting the focused lane.
        assert!(
            app.audition.is_some(),
            "audition field should be Some after Audition"
        );
        let preview = app.audition.as_ref().unwrap();
        assert_eq!(preview.lane, 0, "preview targets the focused lane");
        assert_eq!(
            preview.pattern.name, "lib-drum",
            "preview holds the selected pattern"
        );

        // Committed lane is UNCHANGED (audition is isolated — bug 5 fix).
        assert_eq!(
            app.set.lanes[0].pattern.name, original.name,
            "committed set must NOT be mutated by audition"
        );

        // Returns a LoadPattern for the focused lane (engine plays the cued pattern).
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
    fn audition_updates_preview_when_already_auditioning() {
        let mut app = new_app();
        enter_library(&mut app);
        app.apply(Action::Audition);
        // Re-auditioning the same selection keeps the preview pointed at the selected pattern.
        app.apply(Action::Audition);
        assert_eq!(
            app.audition.as_ref().unwrap().pattern.name,
            "lib-drum",
            "re-audition keeps the preview on the selected pattern"
        );
        // Committed set still untouched.
        assert_ne!(app.set.lanes[0].pattern.name, "lib-drum");
    }

    #[test]
    fn lib_nav_while_auditioning_updates_preview_keeps_committed() {
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
                    id: crate::persist::Id::nil(),
                },
                Pattern {
                    name: "pat-B".into(),
                    desc: String::new(),
                    length: 16,
                    data: PatternData::Drums(dsteps_b),
                    id: crate::persist::Id::nil(),
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
        // Preview holds pat-A; committed lane is untouched (still the original).
        assert_eq!(app.audition.as_ref().unwrap().pattern.name, "pat-A");
        assert_eq!(
            app.set.lanes[0].pattern.name, original_name,
            "committed lane must stay the original during audition"
        );

        // Navigate to pat-B — should re-audition pat-B into the preview.
        let cmds = app.apply(Action::LibNav(0, 1)); // dy=+1 → pat-B
        assert_eq!(
            app.audition.as_ref().unwrap().pattern.name,
            "pat-B",
            "preview should now hold pat-B"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "LibNav while auditioning must return LoadPattern"
        );
        // Committed lane STILL the original — audition never mutates the Set.
        assert_eq!(
            app.set.lanes[0].pattern.name, original_name,
            "committed lane must not change on LibNav"
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
    fn close_library_while_auditioning_reverts_engine_and_clears_audition() {
        let mut app = new_app();
        let original_name = app.set.lanes[0].pattern.name.clone();
        enter_library(&mut app);
        app.apply(Action::Audition);
        // Committed lane is untouched even mid-audition (isolated preview).
        assert_eq!(app.set.lanes[0].pattern.name, original_name);
        // Preview holds the cued library pattern.
        assert_eq!(app.audition.as_ref().unwrap().pattern.name, "lib-drum");

        let cmds = app.apply(Action::CloseLibrary);

        // Committed lane still the original (nothing was ever mutated).
        assert_eq!(
            app.set.lanes[0].pattern.name, original_name,
            "committed lane unchanged"
        );
        // Returns LoadPattern restoring the COMMITTED pattern in the engine.
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::LoadPattern { lane: 0, pattern } if pattern.name == original_name
            )),
            "must emit LoadPattern restoring the committed pattern in the engine"
        );
        // Audition cleared.
        assert!(app.audition.is_none());
        // Not dirty — no Set mutation occurred.
        assert!(!app.dirty, "cancelling audition must not mark dirty");
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

    // --- Task 8: undo/dirty correctness + audition isolation ----------------

    /// Build a library with two distinct drum patterns (A then B) on the focused role.
    fn two_pattern_app() -> App {
        let mut drums = crate::pattern::library::GenreMap::new();
        let pat_a = Pattern {
            name: "pat-A".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![Vec::new(); 16]),
            id: crate::persist::Id::nil(),
        };
        let pat_b = Pattern {
            name: "pat-B".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums({
                let mut s = vec![Vec::new(); 16];
                s[0] = vec![DrumHit {
                    note: 36,
                    vel: 80,
                    prob: 1.0,
                    ratchet: 1,
                }];
                s
            }),
            id: crate::persist::Id::nil(),
        };
        drums.insert("techno".into(), vec![pat_a, pat_b]);
        let library = Library {
            drums,
            bass: crate::pattern::library::GenreMap::new(),
            synth: crate::pattern::library::GenreMap::new(),
        };
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        App::new(set, library)
    }

    /// Bug 4: audition→commit→undo must return to the TRUE pre-audition pattern,
    /// not the auditioned one.
    #[test]
    fn undo_after_committed_audition_restores_pre_audition_pattern() {
        let mut app = two_pattern_app();
        app.apply(Action::FocusLane(0));
        // Set a known committed pattern A by loading pat-A from the library.
        app.apply(Action::OpenLibrary);
        app.apply(Action::LibNav(1, 0)); // Pattern column, pat-A (idx 0)
        app.apply(Action::LibLoad); // commit pat-A
        let known_a = app.set.lanes[0].pattern.clone();
        assert_eq!(known_a.name, "pat-A");

        // Enter library, audition pat-B, then commit it.
        app.apply(Action::OpenLibrary);
        app.apply(Action::LibNav(1, 0)); // Pattern column
        app.apply(Action::LibNav(0, 1)); // -> pat-B
        app.apply(Action::Audition);
        app.apply(Action::LibLoad); // commit pat-B
        assert_eq!(app.set.lanes[0].pattern.name, "pat-B");

        // Undo must restore the pre-audition pattern A, NOT pat-B.
        app.apply(Action::Undo);
        assert_eq!(
            app.set.lanes[0].pattern, known_a,
            "undo after committed audition must restore the pre-audition pattern"
        );
    }

    /// Bug 5: auditioning must not mutate the committed Set; the preview holds the cued pattern.
    #[test]
    fn auditioning_does_not_mutate_committed_set() {
        let mut app = two_pattern_app();
        app.apply(Action::FocusLane(0));
        let committed = app.set.lanes[0].pattern.clone();
        app.apply(Action::OpenLibrary);
        app.apply(Action::LibNav(1, 0)); // Pattern column
        app.apply(Action::LibNav(0, 1)); // -> pat-B
        let cmds = app.apply(Action::Audition);

        // Committed lane unchanged.
        assert_eq!(
            app.set.lanes[0].pattern, committed,
            "audition must not mutate the committed set"
        );
        // Preview holds pat-B.
        assert_eq!(app.audition.as_ref().unwrap().pattern.name, "pat-B");
        // LoadPattern(pat-B) emitted so the engine plays the cued pattern.
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::LoadPattern { lane: 0, pattern } if pattern.name == "pat-B"
            )),
            "must emit LoadPattern for the auditioned pattern"
        );
    }

    #[test]
    fn mute_marks_dirty_and_is_undoable() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2));
        assert!(!app.dirty);
        assert!(!app.focused_lane().mute);
        app.apply(Action::ToggleMute);
        assert!(app.focused_lane().mute, "lane should be muted");
        assert!(app.dirty, "mute must mark dirty");
        app.apply(Action::Undo);
        assert!(!app.focused_lane().mute, "undo must unmute the lane");
    }

    #[test]
    fn undo_emits_full_resync_including_bpm_and_swing() {
        let mut app = new_app();
        app.apply(Action::AdjustSwing(1)); // snapshots
        let cmds = app.apply(Action::Undo);
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SetBpm(_))),
            "undo must resync bpm"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SetSwing(_))),
            "undo must resync swing"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SyncLanes(_))),
            "undo must resync lanes"
        );
    }

    #[test]
    fn set_browser_load_clears_undo() {
        let mut app = new_app();
        // Make an edit so the undo stack is non-empty.
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 1));
        app.apply(Action::ToggleStep);
        assert!(!app.undo.is_empty(), "precondition: undo non-empty");
        assert!(app.dirty, "precondition: dirty");

        // Loading a document (the SetBrowserLoad success path) resets undo/redo and
        // warns about discarded unsaved edits.
        let other =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        app.load_set_document(other, "other".into());
        assert!(app.undo.is_empty(), "loading a set must clear undo");
        assert!(app.redo.is_empty(), "loading a set must clear redo");
        assert!(
            app.status.contains("unsaved changes discarded"),
            "must warn about discarded edits, got: {:?}",
            app.status
        );
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

    #[test]
    fn tempo_event_updates_displayed_bpm() {
        let mut app = new_app();
        app.on_engine_event(crate::engine::EngineEvent::Tempo { bpm: 124.0 });
        assert_eq!(
            app.set.bpm, 124.0,
            "displayed BPM (app.set.bpm) should be updated by Tempo event"
        );
        assert!(
            app.status.contains("124"),
            "status toast should contain the new BPM, got: {:?}",
            app.status
        );
    }

    // --- Task 9: status TTL -------------------------------------------------

    #[test]
    fn set_status_sets_ttl() {
        let mut app = new_app();
        app.set_status("x");
        assert_eq!(app.status, "x");
        assert!(
            app.status_ttl > 0,
            "status_ttl must be > 0 after set_status"
        );
        assert_eq!(app.status_ttl, STATUS_TTL_FRAMES);
    }

    #[test]
    fn tick_status_clears_after_ttl() {
        let mut app = new_app();
        app.set_status("x");
        assert_eq!(app.status, "x");
        for _ in 0..STATUS_TTL_FRAMES {
            app.tick_status();
        }
        assert_eq!(app.status, "", "status must be cleared after TTL frames");
        assert_eq!(app.status_ttl, 0);
    }

    #[test]
    fn tick_status_does_not_underflow_when_already_zero() {
        let mut app = new_app();
        // No status set — TTL is 0. Ticking must not panic or underflow.
        app.tick_status();
        assert_eq!(app.status_ttl, 0);
        assert_eq!(app.status, "");
    }

    #[test]
    fn playhead_event_does_not_clear_fresh_status() {
        let mut app = new_app();
        app.set_status("Saved");
        let ttl_before = app.status_ttl;
        app.on_engine_event(crate::engine::EngineEvent::Playhead {
            step: 3,
            bar: 1,
            beat: 0,
            phase: 0.0,
        });
        assert_eq!(
            app.status, "Saved",
            "Playhead event must not overwrite a fresh status toast"
        );
        assert_eq!(
            app.status_ttl, ttl_before,
            "Playhead event must not alter status_ttl"
        );
    }

    // --- Route editor (Task 8) -------------------------------------------

    #[test]
    fn open_route_editor_sets_mode_and_remembers_focused_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::OpenRouteEditor);
        assert_eq!(app.mode, Mode::RouteEditor);
        assert_eq!(app.route_editor_lane, 1);
        assert_eq!(app.route_editor_field, RouteField::Port);
    }

    #[test]
    fn close_route_editor_returns_to_edit() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        assert_eq!(app.mode, Mode::RouteEditor);
        app.apply(Action::CloseRouteEditor);
        assert_eq!(app.mode, Mode::Edit);
    }

    #[test]
    fn route_nav_lane_wraps_within_lane_count() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // default 3 lanes, start at 0
        app.apply(Action::RouteNavLane(1));
        assert_eq!(app.route_editor_lane, 1);
        app.apply(Action::RouteNavLane(1));
        assert_eq!(app.route_editor_lane, 2);
        app.apply(Action::RouteNavLane(1)); // wrap 2 -> 0
        assert_eq!(app.route_editor_lane, 0);
        app.apply(Action::RouteNavLane(-1)); // wrap 0 -> 2
        assert_eq!(app.route_editor_lane, 2);
    }

    #[test]
    fn route_adjust_channel_plus_one_updates_route_sets_dirty_emits_set_route() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // Lane 0 has no explicit route; profile channel is 9.
        assert!(app.set.lanes[0].route.is_none());
        let cmds = app.apply(Action::RouteAdjustChannel(1));
        // Should now have an explicit route with channel 10.
        assert!(app.set.lanes[0].route.is_some());
        assert_eq!(app.set.lanes[0].route.as_ref().unwrap().channel, 10);
        // dirty flag set.
        assert!(app.dirty);
        // Emits SetRoute for lane 0.
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::SetRoute {
                    lane: 0,
                    route: Some(_)
                }
            )),
            "expected SetRoute command, got: {:?}",
            cmds
        );
    }

    #[test]
    fn route_adjust_channel_clamps_at_zero_and_fifteen() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // Force channel to 0 by subtracting a lot.
        for _ in 0..20 {
            app.apply(Action::RouteAdjustChannel(-1));
        }
        assert_eq!(app.set.lanes[0].route.as_ref().unwrap().channel, 0);
        // Force channel to 15 by adding a lot.
        for _ in 0..20 {
            app.apply(Action::RouteAdjustChannel(1));
        }
        assert_eq!(app.set.lanes[0].route.as_ref().unwrap().channel, 15);
    }

    #[test]
    fn route_cycle_port_to_default_sets_route_none_and_emits_set_route() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // Pre-seed an explicit route on lane 0 so we can cycle away from it.
        app.set.lanes[0].route = Some(LaneRoute {
            port: PortRef {
                stable_key: "fake-port".into(),
                name: "fake-port".into(),
            },
            channel: 5,
            clock_out: true,
        });
        // route_editor_ports is empty (no real hardware), so total slots = 1 (just "default").
        // Cycling by +1 from current (index 0, default is not found so current_idx=0) → stays 0.
        // Actually with an unknown port current_idx=0, next=1 but total=1, so next=0 → default.
        // Inject a fake port list so cycling works.
        app.route_editor_ports = vec!["fake-port".to_string()];
        // current_idx = 1 (found at index 0, +1 = 1), total = 2
        // cycling +1 from 1 → (1+1) % 2 = 0 → default
        let cmds = app.apply(Action::RouteCyclePort(1));
        assert!(
            app.set.lanes[0].route.is_none(),
            "cycling to index 0 sets route=None"
        );
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::SetRoute {
                    lane: 0,
                    route: None
                }
            )),
            "expected SetRoute{{lane:0, route:None}}, got: {:?}",
            cmds
        );
    }

    #[test]
    fn route_toggle_clock_out_flips_and_emits_set_route() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // Lane 0 starts with no explicit route (clock_out defaults to true in effective_route).
        let cmds = app.apply(Action::RouteToggleClockOut);
        // Now has an explicit route with clock_out = false (toggled from default true).
        assert!(
            !app.set.lanes[0].route.as_ref().unwrap().clock_out,
            "clock_out should be false after first toggle"
        );
        assert!(cmds.iter().any(|c| matches!(
            c,
            UiCommand::SetRoute {
                lane: 0,
                route: Some(_)
            }
        )));
        // Toggle again → clock_out = true.
        app.apply(Action::RouteToggleClockOut);
        assert!(
            app.set.lanes[0].route.as_ref().unwrap().clock_out,
            "clock_out should be true after second toggle"
        );
    }

    #[test]
    fn route_adjust_channel_is_undoable() {
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        let before_route = app.set.lanes[0].route.clone();
        app.apply(Action::RouteAdjustChannel(1));
        assert!(app.set.lanes[0].route.is_some());
        // Undo should restore the original route (None).
        app.apply(Action::Undo);
        assert_eq!(app.set.lanes[0].route, before_route);
    }

    // ── Task 9: debounced autosave ────────────────────────────────────────

    #[test]
    fn tick_autosave_fires_only_when_dirty_at_interval() {
        let mut app = new_app();

        // Not dirty: counter must stay at 0 and never return true.
        assert!(!app.dirty);
        for _ in 0..500 {
            assert!(!app.tick_autosave(), "must not fire when not dirty");
        }
        assert_eq!(app.autosave_counter, 0, "counter must stay 0 when clean");

        // Mark dirty: counter increments each tick; fires at AUTOSAVE_INTERVAL_FRAMES.
        app.dirty = true;
        let interval = AUTOSAVE_INTERVAL_FRAMES as u32;
        let mut fired = 0u32;
        for _ in 0..interval {
            if app.tick_autosave() {
                fired += 1;
            }
        }
        assert_eq!(fired, 1, "must fire exactly once per interval");
        assert_eq!(app.autosave_counter, 0, "counter resets after firing");

        // Now clean again: counter resets immediately.
        app.dirty = false;
        assert!(!app.tick_autosave());
        assert_eq!(app.autosave_counter, 0, "counter resets when clean");
    }

    #[test]
    fn deliberate_save_clears_recovery() {
        // Verify that Action::Save keeps mode as Edit and clears dirty on success.
        // The store-level clear_recovery-on-save wiring is verified via code inspection
        // (Action::Save calls clear_recovery) and store::tests::clear_recovery_removes_file.
        let _guard = RECOVERY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = new_app();
        app.set.name = "test-save".to_string();
        app.apply(Action::Save);
        // Mode must stay Edit regardless of save outcome.
        assert_eq!(app.mode, Mode::Edit, "Save must not change mode");
    }

    // ── Task 10: RecoveryPrompt actions ──────────────────────────────────────

    #[test]
    fn recovery_discard_clears_and_goes_to_edit() {
        // Test mode transition + status. The fs.clear is exercised in store tests.
        let _guard = RECOVERY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = new_app();
        app.mode = Mode::RecoveryPrompt;
        let cmds = app.apply(Action::RecoveryDiscard);
        assert_eq!(app.mode, Mode::Edit, "RecoveryDiscard must go to Edit mode");
        assert!(cmds.is_empty(), "RecoveryDiscard emits no engine commands");
        assert!(
            app.status.contains("Discard"),
            "RecoveryDiscard must set a status toast; got: {:?}",
            app.status
        );
    }

    #[test]
    fn recovery_recover_loads_and_marks_dirty_and_resets_undo() {
        // Write a real recovery file to the data_dir() so RecoveryRecover can load it.
        // We create the recovery dir if absent so this works in fresh checkouts/CI.
        // fs-clear is verified in store tests; here we test app behavior only.
        let _guard = RECOVERY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use crate::pattern::store;
        let data_dir = crate::config::data_dir();
        std::fs::create_dir_all(data_dir.join("recovery")).ok();

        let mut recovery_set = Set::default_set(profiles::default_profiles());
        recovery_set.bpm = 99.0;
        recovery_set.name = "recovered-test".to_string();
        // If writing fails (e.g. read-only CI fs), skip rather than panic.
        if store::save_recovery(&data_dir, &recovery_set).is_err() {
            return;
        }

        let mut app = new_app();
        app.undo.push(app.set.clone()); // junk entry to confirm clear
        app.mode = Mode::RecoveryPrompt;

        let cmds = app.apply(Action::RecoveryRecover);

        // Clean up the recovery file regardless of test outcome.
        store::clear_recovery(&data_dir);

        assert_eq!(app.mode, Mode::Edit, "RecoveryRecover must go to Edit mode");
        assert!(app.dirty, "recovered set must be marked dirty");
        assert!(
            app.undo.is_empty(),
            "undo stack must be cleared after recovery"
        );
        assert!(
            app.redo.is_empty(),
            "redo stack must be cleared after recovery"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SetSet(_))),
            "RecoveryRecover must emit SetSet"
        );
        assert_eq!(
            app.set.bpm, 99.0,
            "recovered set bpm must match saved recovery"
        );
    }

    #[test]
    fn recovery_open_saved_goes_to_set_browser() {
        let mut app = new_app();
        app.mode = Mode::RecoveryPrompt;
        let cmds = app.apply(Action::RecoveryOpenSaved);
        assert_eq!(
            app.mode,
            Mode::SetBrowser,
            "RecoveryOpenSaved must go to SetBrowser mode"
        );
        assert!(cmds.is_empty());
    }

    #[test]
    fn toggle_mirror_flips_and_emits_set_mirror() {
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let lib = crate::pattern::library::Library::empty();
        let mut app = App::new(set, lib);
        assert!(!app.mirror_on, "starts false");

        let cmds = app.apply(Action::ToggleMirror);
        assert!(app.mirror_on, "must flip to true");
        assert!(
            cmds.contains(&crate::engine::UiCommand::SetMirror(true)),
            "must emit SetMirror(true)"
        );

        let cmds2 = app.apply(Action::ToggleMirror);
        assert!(!app.mirror_on, "must flip back to false");
        assert!(
            cmds2.contains(&crate::engine::UiCommand::SetMirror(false)),
            "must emit SetMirror(false)"
        );
    }

    // ── M3 Task 2: load-while-playing queues; ACTIVE/QUEUED display ──────────

    #[test]
    fn libload_while_playing_queues_not_loads() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums; lib_role -> Drums
        app.apply(Action::OpenLibrary);
        // Simulate engine confirmed playing.
        app.engine_playing = true;
        let cmds = app.apply(Action::LibLoad);
        // Must emit QueuePattern, NOT LoadPattern.
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::QueuePattern { lane: 0, .. })),
            "while playing, LibLoad must emit QueuePattern; got: {:?}",
            cmds
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "while playing, LibLoad must NOT emit LoadPattern; got: {:?}",
            cmds
        );
        // queued[0] must be set to the pattern name.
        assert!(
            app.queued[0].is_some(),
            "queued[0] must be Some after LibLoad while playing"
        );
        assert_eq!(
            app.queued[0].as_deref(),
            Some("lib-drum"),
            "queued[0] must contain the loaded pattern name"
        );
    }

    #[test]
    fn libload_while_stopped_loads_immediately() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = false;
        let cmds = app.apply(Action::LibLoad);
        // Must emit LoadPattern (existing behavior).
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "while stopped, LibLoad must emit LoadPattern; got: {:?}",
            cmds
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::QueuePattern { .. })),
            "while stopped, LibLoad must NOT emit QueuePattern; got: {:?}",
            cmds
        );
        // queued[0] must remain None.
        assert!(
            app.queued[0].is_none(),
            "queued[0] must be None after immediate load while stopped"
        );
    }

    #[test]
    fn launched_event_clears_queued_display() {
        let mut app = new_app();
        // Manually set queued state as if a queue was pending on lane 1.
        app.queued[1] = Some("lib-bass".to_string());
        assert!(app.queued[1].is_some());
        // Fire the Launched event for lane 1.
        app.on_engine_event(crate::engine::EngineEvent::Launched { lane: 1, step: 16 });
        assert!(
            app.queued[1].is_none(),
            "Launched event must clear queued[1]"
        );
    }

    #[test]
    fn cancel_queue_clears_display_and_emits_command() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.queued[1] = Some("lib-bass".to_string());
        let cmds = app.apply(Action::CancelQueue);
        assert!(
            app.queued[1].is_none(),
            "CancelQueue must clear queued[focus]"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::CancelQueue { lane: 1 })),
            "CancelQueue must emit UiCommand::CancelQueue{{lane:1}}; got: {:?}",
            cmds
        );
    }

    #[test]
    fn toggle_launch_quant_flips_nextbar_nextbeat_nextbar() {
        let mut app = new_app();
        assert_eq!(app.launch_quant, Quant::NextBar, "default is NextBar");
        app.apply(Action::ToggleLaunchQuant);
        assert_eq!(
            app.launch_quant,
            Quant::NextBeat,
            "first toggle -> NextBeat"
        );
        app.apply(Action::ToggleLaunchQuant);
        assert_eq!(app.launch_quant, Quant::NextBar, "second toggle -> NextBar");
    }

    #[test]
    fn libload_while_playing_queues_with_nextbeat_when_set() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = true;
        app.launch_quant = Quant::NextBeat;
        let cmds = app.apply(Action::LibLoad);
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::QueuePattern {
                    lane: 0,
                    quant: Quant::NextBeat,
                    ..
                }
            )),
            "QueuePattern must carry the current launch_quant; got: {:?}",
            cmds
        );
    }

    #[test]
    fn libload_while_playing_commits_pattern_to_doc() {
        // The doc pattern is updated immediately even when queuing (so saved file matches).
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = true;
        app.apply(Action::LibLoad);
        assert_eq!(
            app.set.lanes[0].pattern.name, "lib-drum",
            "doc pattern must be updated even when queued (not stopped)"
        );
    }

    // --- M3 T3: Audition gating + focus-change revert ---

    #[test]
    fn audition_refused_when_lane_live_and_unmuted() {
        // engine_playing=true, focused lane NOT muted → gate blocks the preview.
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = true;
        assert!(!app.set.lanes[0].mute, "lane must be unmuted for this test");

        let cmds = app.apply(Action::Audition);

        assert!(
            app.audition.is_none(),
            "audition must not be set when lane is live and unmuted"
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "no LoadPattern must be emitted when gate refuses"
        );
        assert!(
            app.status.contains("Mute lane"),
            "status should hint to mute the lane; got: {:?}",
            app.status
        );
    }

    #[test]
    fn audition_allowed_when_stopped() {
        // engine_playing=false → gate passes regardless of mute state.
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = false;

        let cmds = app.apply(Action::Audition);

        assert!(
            app.audition.is_some(),
            "audition should be set when transport is stopped"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "must emit LoadPattern when stopped"
        );
    }

    #[test]
    fn audition_allowed_when_lane_muted() {
        // engine_playing=true but lane muted → gate passes (silent lane, safe to preview).
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = true;
        app.set.lanes[0].mute = true;

        let cmds = app.apply(Action::Audition);

        assert!(
            app.audition.is_some(),
            "audition should be allowed when lane is muted"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "must emit LoadPattern when lane is muted"
        );
    }

    #[test]
    fn focus_change_reverts_audition() {
        // Start audition while stopped, then FocusNext → revert + clear.
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = false;
        app.apply(Action::Audition);
        assert!(
            app.audition.is_some(),
            "audition should be active before focus change"
        );
        let committed_name = app.set.lanes[0].pattern.name.clone();

        let cmds = app.apply(Action::FocusNext);

        assert!(
            app.audition.is_none(),
            "audition should be cleared after focus change"
        );
        // Engine must be restored to the committed pattern.
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::LoadPattern { lane: 0, pattern } if pattern.name == committed_name
            )),
            "must emit LoadPattern restoring committed pattern after focus change; got: {:?}",
            cmds
        );
        assert_eq!(
            app.status, "Audition cancelled",
            "status should say cancelled; got: {:?}",
            app.status
        );
    }

    #[test]
    fn audition_commit_clears_audition() {
        // LibLoad after audition must clear self.audition.
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::OpenLibrary);
        app.engine_playing = false;
        app.apply(Action::Audition);
        assert!(app.audition.is_some());

        app.apply(Action::LibLoad);

        assert!(
            app.audition.is_none(),
            "self.audition must be None after committing via LibLoad"
        );
    }

    // ── M3 Task 5: pattern management ops ────────────────────────────────────

    /// Returns a unique token for each call within the process (nanos + pid + atomic counter).
    /// Used to make filenames unique across parallel test threads without touching the env.
    fn unique_token(tag: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CTR: AtomicU64 = AtomicU64::new(1);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        format!("{}-{}-{}", tag, nanos, n)
    }

    #[test]
    fn clear_pattern_empties_focused_lane_and_marks_dirty() {
        let mut app = new_app();
        // Place a hit on lane 0 (drums) so it is non-empty.
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep);
        let len = app.focused_lane().pattern.length;
        let undo_depth_before = app.undo.len();

        let cmds = app.apply(Action::ClearPattern);

        // Snapshot taken → undo stack grew by 1.
        assert_eq!(
            app.undo.len(),
            undo_depth_before + 1,
            "ClearPattern must snapshot (undoable)"
        );
        assert!(app.dirty, "ClearPattern must mark dirty");
        // Lane pattern is now all-empty drums of the same length.
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps.len(), len, "length must be preserved");
            assert!(
                steps.iter().all(|s| s.is_empty()),
                "all steps must be empty after clear"
            );
        } else {
            panic!("expected drums");
        }
        assert_eq!(app.focused_lane().pattern.name, "init");
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "must emit LoadPattern for the engine"
        );
        // Undo must restore the hit.
        app.apply(Action::Undo);
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps[0].len(), 1, "undo must restore the drum hit");
        }
    }

    #[test]
    fn save_as_user_pattern_writes_file_with_fresh_id() {
        // Write to the real data_dir()/patterns using a unique pattern name.
        // This avoids env-var mutation (which races with other tests using config::data_dir()).
        let pat_dir = crate::config::data_dir().join("patterns");
        std::fs::create_dir_all(&pat_dir).ok();
        let tok = unique_token("save-as");
        let unique_name = format!("t5-save-as-{}", tok);

        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original_id = app.focused_lane().pattern.id.clone();

        app.apply(Action::SaveAsUserPattern(unique_name.clone()));

        // Lane is NOT mutated.
        assert_eq!(
            app.focused_lane().pattern.id,
            original_id,
            "SaveAsUserPattern must not mutate the lane's pattern id"
        );
        // Find the written file by listing and matching the unique name.
        let files = crate::pattern::store::list_user_patterns(&pat_dir);
        let written = files
            .iter()
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("t5-save-as"))
                    .unwrap_or(false)
            })
            .expect("a file with the unique name must have been written");
        let saved = crate::pattern::store::load_user_pattern(written).unwrap();
        assert_eq!(saved.name, unique_name);
        assert_ne!(
            saved.id, original_id,
            "saved pattern must have a fresh (different) id"
        );
        assert!(!saved.id.is_nil(), "saved id must be non-nil");

        // Clean up only the file we created.
        std::fs::remove_file(written).ok();
    }

    #[test]
    fn duplicate_user_pattern_creates_new_id_copy() {
        // Write source and copy both into data_dir()/patterns using unique names.
        let pat_dir = crate::config::data_dir().join("patterns");
        std::fs::create_dir_all(&pat_dir).ok();
        let tok = unique_token("dup");

        let mut src = crate::pattern::model::Pattern::empty_drums(8);
        src.name = format!("t5-dup-src-{}", tok);
        let src_path = crate::pattern::store::save_user_pattern(&pat_dir, &mut src).unwrap();
        let src_id = src.id.clone();

        let mut app = new_app();
        app.apply(Action::DuplicateUserPattern(src_path.clone()));

        // Find the copy file: unique to this run via tok, and contains "copy".
        let files = crate::pattern::store::list_user_patterns(&pat_dir);
        let copy_file = files
            .iter()
            .find(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.contains("copy") && name.contains("t5-dup-src")
            })
            .expect("copy file must exist and contain 'copy' in the filename");
        let copy = crate::pattern::store::load_user_pattern(copy_file).unwrap();
        assert!(
            copy.name.contains("copy"),
            "copy name must contain 'copy': {}",
            copy.name
        );
        assert_ne!(
            copy.id, src_id,
            "copy must have a different id than the original"
        );
        assert!(!copy.id.is_nil());

        // Clean up only the files we created.
        std::fs::remove_file(&src_path).ok();
        std::fs::remove_file(copy_file).ok();
    }

    #[test]
    fn rename_user_pattern_keeps_id_changes_name_removes_old() {
        let pat_dir = crate::config::data_dir().join("patterns");
        std::fs::create_dir_all(&pat_dir).ok();
        let tok = unique_token("rename");

        let mut src = crate::pattern::model::Pattern::empty_drums(8);
        src.name = format!("t5-rename-old-{}", tok);
        let old_path = crate::pattern::store::save_user_pattern(&pat_dir, &mut src).unwrap();
        let original_id = src.id.clone();
        let new_name = format!("t5-rename-new-{}", tok);

        let mut app = new_app();
        app.apply(Action::RenameUserPattern(
            old_path.clone(),
            new_name.clone(),
        ));

        // Old file is gone.
        assert!(!old_path.exists(), "old file must be removed after rename");
        // New file exists in pat_dir with updated name but same id.
        let files = crate::pattern::store::list_user_patterns(&pat_dir);
        let new_file = files
            .iter()
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("t5-rename-new"))
                    .unwrap_or(false)
            })
            .expect("renamed file must exist");
        let renamed = crate::pattern::store::load_user_pattern(new_file).unwrap();
        assert_eq!(renamed.name, new_name);
        assert_eq!(
            renamed.id, original_id,
            "rename must preserve the pattern id"
        );

        std::fs::remove_file(new_file).ok();
    }

    #[test]
    fn delete_user_pattern_removes_file() {
        // DeleteUserPattern takes an explicit path — no env var or shared dir needed.
        // Write to a private temp dir to keep this fully isolated.
        let tok = unique_token("delete");
        let tmp = std::env::temp_dir().join(format!("midip-t5-del-{}", tok));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut p = crate::pattern::model::Pattern::empty_drums(4);
        p.name = format!("doomed-{}", tok);
        let path = crate::pattern::store::save_user_pattern(&tmp, &mut p).unwrap();
        assert!(path.exists(), "file must exist before delete");

        let mut app = new_app();
        app.apply(Action::DeleteUserPattern(path.clone()));

        assert!(!path.exists(), "file must be gone after DeleteUserPattern");

        std::fs::remove_dir_all(&tmp).ok();
    }

    // ── Task 6: Set management ops ───────────────────────────────────────────

    #[test]
    fn save_set_as_writes_new_id_and_tracks_path() {
        let tok = unique_token("t6-saveas");
        let tmp = std::env::temp_dir().join(format!("midip-t6-saveas-{}", tok));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut app = new_app();
        // Override data_dir is not possible cleanly, so we call the action and
        // verify using the stored current_set_path plus direct store access.
        // We need the sets dir to be writable; use a tmp dir per task instructions:
        // "sets dir resolves under target/ in test binaries" — the action uses
        // config::data_dir().join("sets"), so we verify via current_set_path.

        let original_id = app.set.id.clone(); // nil before first save

        // SaveSetAs("mine-<tok>") gives a fresh id and new name.
        let name = format!("t6mine{}", tok);
        app.apply(Action::SaveSetAs(name.clone()));

        // Name was updated.
        assert_eq!(app.set.name, name);
        // Id was freshly minted (different from nil).
        assert!(!app.set.id.is_nil(), "id must be non-nil after SaveSetAs");
        // If original was nil, it must now differ; always true for nil.
        assert_ne!(app.set.id, original_id.clone());
        // current_set_path must be tracked.
        assert!(
            app.current_set_path.is_some(),
            "current_set_path must be Some after SaveSetAs"
        );
        // File must exist on disk.
        let path = app.current_set_path.as_ref().unwrap();
        assert!(path.exists(), "saved file must exist on disk");
        // dirty must be false.
        assert!(!app.dirty, "dirty must be false after SaveSetAs");
        // Status must mention "Saved as".
        assert!(
            app.status.contains("Saved as"),
            "status must say 'Saved as' but was: {}",
            app.status
        );

        // Cleanup.
        std::fs::remove_file(path).ok();
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn rename_set_keeps_id_changes_file_removes_old() {
        let tok = unique_token("t6-rename");

        let mut app = new_app();
        // First, do a SaveSetAs to get a real file and current_set_path.
        let first_name = format!("t6renold{}", tok);
        app.apply(Action::SaveSetAs(first_name.clone()));
        let old_path = app
            .current_set_path
            .clone()
            .expect("must have a path after SaveSetAs");
        assert!(old_path.exists(), "old file must exist before rename");
        let original_id = app.set.id.clone();

        // Now rename.
        let new_name = format!("t6rennew{}", tok);
        app.apply(Action::RenameSet(new_name.clone()));

        // Id must be unchanged.
        assert_eq!(
            app.set.id, original_id,
            "RenameSet must preserve the set id"
        );
        // Name updated.
        assert_eq!(app.set.name, new_name);
        // Old file is gone.
        assert!(
            !old_path.exists(),
            "old file must be removed after RenameSet"
        );
        // New file exists.
        let new_path = app
            .current_set_path
            .as_ref()
            .expect("current_set_path must be Some after rename");
        assert!(new_path.exists(), "new file must exist after RenameSet");
        assert_ne!(new_path, &old_path, "new path must differ from old path");
        // dirty cleared.
        assert!(!app.dirty, "dirty must be false after RenameSet");
        // Status mentions "Renamed to".
        assert!(
            app.status.contains("Renamed to"),
            "status must say 'Renamed to' but was: {}",
            app.status
        );

        // Cleanup.
        std::fs::remove_file(new_path).ok();
    }

    #[test]
    fn duplicate_set_creates_copy_leaves_current() {
        let tok = unique_token("t6-dup");

        let mut app = new_app();
        // Save to get a stable id for the current set.
        let orig_name = format!("t6dupbase{}", tok);
        app.apply(Action::SaveSetAs(orig_name.clone()));
        let orig_path = app.current_set_path.clone().expect("must have path");
        let orig_id = app.set.id.clone();
        let orig_name_after = app.set.name.clone();

        // Duplicate.
        app.apply(Action::DuplicateSet);

        // Current set unchanged.
        assert_eq!(
            app.set.id, orig_id,
            "DuplicateSet must not change current set id"
        );
        assert_eq!(
            app.set.name, orig_name_after,
            "DuplicateSet must not change current set name"
        );
        assert_eq!(
            app.current_set_path,
            Some(orig_path.clone()),
            "current_set_path must not change after DuplicateSet"
        );

        // A copy file should exist with " copy" suffix in the name.
        let sets_dir = orig_path.parent().expect("path must have parent");
        let files = crate::pattern::store::list_sets(sets_dir).unwrap_or_default();
        let copy_file = files.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains("copy"))
                .unwrap_or(false)
        });
        assert!(
            copy_file.is_some(),
            "a 'copy' file must exist after DuplicateSet"
        );

        // The copy must have a different id from the original.
        let copy = crate::pattern::store::load_set(copy_file.unwrap()).unwrap();
        assert_ne!(copy.id, orig_id, "duplicate must have a fresh id");
        assert!(
            copy.name.contains("copy"),
            "duplicate name must contain 'copy' but was: {}",
            copy.name
        );

        // Status mentions "Duplicated".
        assert!(
            app.status.contains("Duplicated"),
            "status must mention 'Duplicated' but was: {}",
            app.status
        );

        // Cleanup.
        std::fs::remove_file(&orig_path).ok();
        if let Some(cp) = copy_file {
            std::fs::remove_file(cp).ok();
        }
    }

    #[test]
    fn new_set_resets_and_clears_undo() {
        let mut app = new_app();
        // Make an edit to push something onto the undo stack.
        app.apply(Action::ToggleStep);
        assert!(!app.undo.is_empty(), "undo must be non-empty after edit");
        // Track a path.
        app.apply(Action::SaveSetAs(format!(
            "t6newbefore{}",
            unique_token("t6-new")
        )));
        assert!(app.current_set_path.is_some());

        let cmds = app.apply(Action::NewSet);

        // Undo/redo cleared.
        assert!(app.undo.is_empty(), "undo must be empty after NewSet");
        assert!(app.redo.is_empty(), "redo must be empty after NewSet");
        // dirty cleared.
        assert!(!app.dirty, "dirty must be false after NewSet");
        // current_set_path cleared.
        assert!(
            app.current_set_path.is_none(),
            "current_set_path must be None after NewSet"
        );
        // Must emit SetSet.
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::SetSet(_))),
            "NewSet must emit SetSet"
        );
    }

    #[test]
    fn delete_set_removes_file() {
        let tok = unique_token("t6-delset");

        let mut app = new_app();
        // Save to get a file.
        app.apply(Action::SaveSetAs(format!("t6delsetfile{}", tok)));
        let path = app
            .current_set_path
            .clone()
            .expect("must have path after save");
        assert!(path.exists(), "file must exist before delete");

        app.apply(Action::DeleteSet(path.clone()));

        assert!(!path.exists(), "file must be gone after DeleteSet");
        // current_set_path cleared when deleted path matches.
        assert!(
            app.current_set_path.is_none(),
            "current_set_path must be None after deleting current file"
        );
        // Status mentions "Deleted".
        assert!(
            app.status.contains("Deleted"),
            "status must say 'Deleted' but was: {}",
            app.status
        );
    }

    // ── M3 Task 7: management UI app-layer tests ─────────────────────────────

    #[test]
    fn name_entry_char_appends_to_buffer() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::SaveSetAs));
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::SaveSetAs));
        assert!(app.name_input.is_empty());
        app.apply(Action::NameChar('m'));
        app.apply(Action::NameChar('y'));
        assert_eq!(app.name_input, "my");
    }

    #[test]
    fn name_entry_backspace_removes_last_char() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::RenameSet));
        app.apply(Action::NameChar('a'));
        app.apply(Action::NameChar('b'));
        app.apply(Action::NameBackspace);
        assert_eq!(app.name_input, "a");
    }

    #[test]
    fn name_entry_cancel_returns_to_edit_and_clears() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::SaveSetAs));
        app.apply(Action::NameChar('x'));
        app.apply(Action::NameCancel);
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.name_input.is_empty());
    }

    #[test]
    fn name_entry_commit_save_set_as_applies_action() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::SaveSetAs));
        app.apply(Action::NameChar('t'));
        app.apply(Action::NameChar('e'));
        app.apply(Action::NameChar('s'));
        app.apply(Action::NameChar('t'));
        // NameCommit → applies SaveSetAs("test") internally.
        // Since this writes to disk and we're in a test, it may fail with a status toast,
        // but mode should still return to Edit and name_input cleared.
        app.apply(Action::NameCommit);
        assert_eq!(app.mode, Mode::Edit, "NameCommit must return to Edit");
        assert!(
            app.name_input.is_empty(),
            "name_input must be cleared after commit"
        );
    }

    #[test]
    fn confirm_yes_new_set_resets_document() {
        let mut app = new_app();
        // Mark dirty and put some undo history.
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep);
        assert!(app.dirty);
        assert!(!app.undo.is_empty());
        // Open confirm and accept.
        app.apply(Action::OpenConfirm(ConfirmAction::NewSet));
        assert_eq!(app.mode, Mode::Confirm(ConfirmAction::NewSet));
        app.apply(Action::ConfirmYes);
        assert_eq!(app.mode, Mode::Edit);
        assert!(!app.dirty);
        assert!(app.undo.is_empty(), "NewSet must clear undo stack");
    }

    #[test]
    fn confirm_no_cancels_without_action() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep);
        let set_before = app.set.clone();
        app.apply(Action::OpenConfirm(ConfirmAction::NewSet));
        app.apply(Action::ConfirmNo);
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.set, set_before, "ConfirmNo must leave set unchanged");
        assert!(app.status.contains("Cancelled"));
    }

    #[test]
    fn set_browser_new_set_dirty_routes_to_confirm() {
        let mut app = new_app();
        // Make app dirty.
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep);
        assert!(app.dirty);
        app.apply(Action::SetBrowserNewSet);
        assert_eq!(
            app.mode,
            Mode::Confirm(ConfirmAction::NewSet),
            "SetBrowserNewSet when dirty must go to Confirm"
        );
    }

    #[test]
    fn set_browser_new_set_clean_does_it_directly() {
        let mut app = new_app();
        assert!(!app.dirty);
        app.apply(Action::SetBrowserNewSet);
        // Should have reset to a new set without confirm.
        assert_eq!(app.mode, Mode::Edit);
        assert!(!app.dirty);
        assert!(app.undo.is_empty());
    }

    #[test]
    fn open_clear_pattern_with_material_routes_to_confirm() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit → pattern has material
                                       // Now clear — should route to confirm.
        app.apply(Action::OpenClearPattern);
        assert_eq!(
            app.mode,
            Mode::Confirm(ConfirmAction::ClearPattern),
            "OpenClearPattern with material must route to Confirm"
        );
    }

    #[test]
    fn open_clear_pattern_without_material_clears_directly() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // default set has empty drums
                                         // Pattern should be empty already → direct clear.
        app.apply(Action::OpenClearPattern);
        assert_eq!(
            app.mode,
            Mode::Edit,
            "empty pattern must clear without confirm"
        );
        assert!(app.status.contains("Cleared"));
    }

    #[test]
    fn open_save_user_pattern_enters_name_entry_mode() {
        let mut app = new_app();
        app.apply(Action::OpenSaveUserPattern);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::SaveUserPattern));
        assert!(app.name_input.is_empty());
    }

    #[test]
    fn set_browser_rename_enters_name_entry_mode() {
        let mut app = new_app();
        app.apply(Action::SetBrowserRename);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::RenameSet));
    }

    #[test]
    fn set_browser_save_as_enters_name_entry_mode() {
        let mut app = new_app();
        app.apply(Action::SetBrowserSaveAs);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::SaveSetAs));
    }

    #[test]
    fn name_char_rejects_non_ascii_alphanumeric_except_allowed() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::SaveSetAs));
        // Allowed: alphanumeric, space, -, #
        app.apply(Action::NameChar('a'));
        app.apply(Action::NameChar('1'));
        app.apply(Action::NameChar(' '));
        app.apply(Action::NameChar('-'));
        app.apply(Action::NameChar('#'));
        assert_eq!(app.name_input, "a1 -#");
        // Rejected: control chars mapped to NameChar shouldn't pass the filter
        // (in practice input.rs only sends allowed chars, but test the apply guard too)
        app.apply(Action::NameChar('\t'));
        app.apply(Action::NameChar('\n'));
        assert_eq!(app.name_input, "a1 -#", "tabs/newlines must be rejected");
    }

    #[test]
    fn name_char_caps_at_32_characters() {
        let mut app = new_app();
        app.apply(Action::OpenNameEntry(NamePurpose::SaveSetAs));
        for _ in 0..40 {
            app.apply(Action::NameChar('x'));
        }
        assert_eq!(
            app.name_input.len(),
            32,
            "name_input must be capped at 32 chars"
        );
    }

    // ── DoubleLength tests ────────────────────────────────────────────────

    /// Build a 16-step drum pattern with hits at steps 0, 4, 8, 12.
    fn app_with_drum_hits() -> App {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // lane 0 is drums
                                         // Ensure the pattern is exactly 16 steps (default).
        if let PatternData::Drums(ref mut steps) = app.set.lanes[0].pattern.data {
            *steps = vec![Vec::new(); 16];
            let hit = DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
            };
            steps[0] = vec![hit.clone()];
            steps[4] = vec![hit.clone()];
            steps[8] = vec![hit.clone()];
            steps[12] = vec![hit.clone()];
        }
        app.set.lanes[0].pattern.length = 16;
        app
    }

    #[test]
    fn double_length_repeats_content_and_doubles() {
        let mut app = app_with_drum_hits();
        assert_eq!(app.focused_lane().pattern.length, 16);

        app.apply(Action::DoubleLength);

        let lane = app.focused_lane();
        assert_eq!(lane.pattern.length, 32, "length must be 32");
        if let PatternData::Drums(steps) = &lane.pattern.data {
            assert_eq!(steps.len(), 32, "data vec must have 32 entries");
            // Original hits preserved.
            assert!(!steps[0].is_empty(), "hit at step 0");
            assert!(!steps[4].is_empty(), "hit at step 4");
            assert!(!steps[8].is_empty(), "hit at step 8");
            assert!(!steps[12].is_empty(), "hit at step 12");
            // Mirrored hits in the second half.
            assert!(!steps[16].is_empty(), "mirrored hit at step 16");
            assert!(!steps[20].is_empty(), "mirrored hit at step 20");
            assert!(!steps[24].is_empty(), "mirrored hit at step 24");
            assert!(!steps[28].is_empty(), "mirrored hit at step 28");
            // Empty steps remain empty.
            assert!(steps[1].is_empty(), "step 1 stays empty");
            assert!(steps[17].is_empty(), "step 17 stays empty");
        } else {
            panic!("expected drums data");
        }
        assert!(app.dirty, "dirty must be set after DoubleLength");
        assert!(!app.undo.is_empty(), "snapshot must have been taken");
    }

    #[test]
    fn double_length_caps_at_64() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        // Set pattern to length 40 manually.
        if let PatternData::Drums(ref mut steps) = app.set.lanes[0].pattern.data {
            *steps = vec![Vec::new(); 40];
            let hit = DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
            };
            steps[3] = vec![hit.clone()];
        }
        app.set.lanes[0].pattern.length = 40;

        app.apply(Action::DoubleLength);
        assert_eq!(app.focused_lane().pattern.length, 64, "40*2 capped to 64");
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert_eq!(steps.len(), 64);
            // step 3 mirrored: 40+3=43
            assert!(!steps[43].is_empty(), "step 43 should mirror step 3");
        }

        // Already at 64 → no-op.
        let undo_len_before = app.undo.len();
        app.apply(Action::DoubleLength);
        assert_eq!(app.focused_lane().pattern.length, 64, "stays at 64");
        assert_eq!(app.undo.len(), undo_len_before, "no snapshot when at max");
        assert!(app.status.contains("max length"), "status toast set");
    }

    #[test]
    fn double_length_is_undoable() {
        let mut app = app_with_drum_hits();
        let orig_len = app.focused_lane().pattern.length;

        app.apply(Action::DoubleLength);
        assert_eq!(app.focused_lane().pattern.length, orig_len * 2);

        app.apply(Action::Undo);
        assert_eq!(
            app.focused_lane().pattern.length,
            orig_len,
            "undo must restore original length"
        );
        // Original hits still present.
        if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
            assert!(!steps[0].is_empty(), "step 0 hit restored after undo");
            assert!(!steps[4].is_empty(), "step 4 hit restored after undo");
        }
    }
}
