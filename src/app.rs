//! Application state and the action reducer (UI thread side).
//!
//! `App` holds the canonical edit state. `apply` mutates state and returns the
//! `UiCommand`s that must be forwarded to the engine (e.g. pattern edits emit
//! `LoadPattern`). Undo/redo snapshot the whole `Set`.

use crate::engine::{EngineEvent, UiCommand};
use crate::pattern::euclid;
use crate::pattern::library::{Library, LibRole};
use crate::pattern::model::{
    DrumHit, DrumStep, Lane, LaneKind, MelodicNote, MelodicStep, Pattern, PatternData, Set,
};
use crate::devices::profiles;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Edit,
    Library,
    Help,
    TempoEntry,
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
    AdjustSwing(i8),       // transport swing param (distinct from AdjustLen = melodic note len)
    AdjustPatternLen(i8),  // resize the focused lane's pattern length
    AdjustProb(i8),        // per-step probability on the cursor cell (±0.1 per unit)
    AdjustRatchet(i8),     // per-step ratchet count on the cursor cell (clamped 1..=8)
    Euclid { dp: i8, dr: i8 }, // drums: dp = ±pulses for focused voice, dr = ±rotation
    Panic,                     // all-notes-off; no undo snapshot, no Set mutation
    OpenLibrary,
    CloseLibrary,
    LibNav(i32, i32),
    LibLoad,
    Save,
    Help,
    Quit,
    None,
}

pub struct App {
    pub set: Set,
    pub focus: usize,
    pub mode: Mode,
    pub cur_row: usize,
    pub cur_col: usize,
    pub euclid_rotation: usize, // current euclid rotation for the focused drum voice

    pub playing: bool,
    pub playhead: usize,
    pub bar: u32,
    pub link_enabled: bool,
    pub link_tempo: f64,
    pub link_peers: u64,
    pub device_status: Vec<(bool, String)>,
    pub library: Library,
    pub lib_role: LibRole,
    pub lib_genre: usize,
    pub lib_pattern: usize,
    pub clipboard: Option<PatternData>,
    pub undo: Vec<Set>,
    pub redo: Vec<Set>,
    pub status: String,
    pub should_quit: bool,
    pub tempo_input: String,
}

/// Default melodic velocity multiplier when placing a note (1.0 -> MIDI 100).
const MEL_DEFAULT_VEL: f32 = 1.0;

impl App {
    pub fn new(set: Set, library: Library) -> App {
        let n = set.lanes.len();
        let role = role_for_profile(set.lanes.get(0).map(|l| l.profile.id).unwrap_or("t8-drums"));
        App {
            set,
            focus: 0,
            mode: Mode::Edit,
            cur_row: 0,
            cur_col: 0,
            euclid_rotation: 0,
            playing: false,
            playhead: 0,
            bar: 0,
            link_enabled: false,
            link_tempo: 120.0,
            link_peers: 0,
            device_status: vec![(false, String::new()); n],
            library,
            lib_role: role,
            lib_genre: 0,
            lib_pattern: 0,
            clipboard: Option::None,
            undo: Vec::new(),
            redo: Vec::new(),
            status: String::new(),
            should_quit: false,
            tempo_input: String::new(),
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
        let mut cmds = Vec::new();
        match action {
            Action::TogglePlay => {
                self.playing = !self.playing;
                cmds.push(if self.playing { UiCommand::Play } else { UiCommand::Stop });
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
                cmds.push(self.load_focused());
            }
            Action::AdjustVel(d) => {
                self.snapshot();
                self.adjust_vel(d);
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
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::ToggleMute => {
                let lane = &mut self.set.lanes[self.focus];
                lane.mute = !lane.mute;
                cmds.push(UiCommand::Mute { lane: self.focus, on: lane.mute });
            }
            Action::ToggleSolo => {
                let lane = &mut self.set.lanes[self.focus];
                lane.solo = !lane.solo;
                cmds.push(UiCommand::Solo { lane: self.focus, on: lane.solo });
            }
            Action::SetBpm(bpm) => {
                self.set.bpm = bpm;
                cmds.push(UiCommand::SetBpm(bpm));
            }
            Action::Tap => cmds.push(UiCommand::Tap),
            Action::ToggleLink => {
                self.link_enabled = !self.link_enabled;
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
                cmds.push(UiCommand::SetBpm(self.set.bpm));
            }
            Action::AdjustSwing(d) => {
                // Swing mutates the Set, so it is snapshotted per the undo invariant.
                self.snapshot();
                self.set.swing = (self.set.swing + d as f32 * 0.02).clamp(0.5, 0.66);
                cmds.push(UiCommand::SetSwing(self.set.swing));
            }
            Action::AdjustPatternLen(d) => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                let new_len =
                    (lane.pattern.length as i32 + d as i32).clamp(1, 64) as usize;
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
                    cmds.push(self.load_focused());
                }
            }
            Action::AdjustRatchet(d) => {
                if self.adjust_ratchet(d) {
                    cmds.push(self.load_focused());
                }
            }
            Action::Euclid { dp, dr } => {
                if self.apply_euclid(dp, dr) {
                    cmds.push(self.load_focused());
                }
            }
            Action::Panic => {
                // Live recovery: forward to the engine. No undo snapshot, no Set mutation.
                cmds.push(UiCommand::Panic);
            }
            Action::OpenLibrary => self.mode = Mode::Library,
            Action::CloseLibrary => self.mode = Mode::Edit,
            Action::LibNav(dg, dp) => self.lib_nav(dg, dp),
            Action::LibLoad => {
                if let Some(pat) = self.selected_lib_pattern().cloned() {
                    self.snapshot();
                    self.set.lanes[self.focus].pattern = pat;
                    cmds.push(self.load_focused());
                }
            }
            Action::Save => {
                // Cross-task dependency: `config::data_dir()` is defined in Task 21.
                let dir = crate::config::data_dir().join("sets");
                match crate::pattern::store::save_set(&dir, &self.set) {
                    Ok(path) => self.status = format!("saved {}", path.display()),
                    Err(e) => self.status = format!("save failed: {}", e),
                }
            }
            Action::Help => {
                self.mode = if self.mode == Mode::Help { Mode::Edit } else { Mode::Help };
            }
            Action::Quit => self.should_quit = true,
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
            EngineEvent::LinkStatus { enabled, tempo, peers } => {
                self.link_enabled = enabled;
                self.link_tempo = tempo;
                self.link_peers = peers;
            }
            EngineEvent::DeviceStatus { lane, connected, port } => {
                if let Some(slot) = self.device_status.get_mut(lane) {
                    *slot = (connected, port);
                }
            }
        }
    }

    // --- internal helpers ---

    fn set_focus(&mut self, i: usize) {
        self.focus = i;
        self.lib_role = role_for_profile(self.set.lanes[i].profile.id);
        self.lib_genre = 0;
        self.lib_pattern = 0;
        self.euclid_rotation = 0; // focus changed -> reset euclid rotation
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
    }

    fn snapshot(&mut self) {
        self.undo.push(self.set.clone());
        self.redo.clear();
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
                        step.push(DrumHit { note, vel: 100, prob: 1.0, ratchet: 1 });
                    }
                }
            }
            PatternData::Melodic(steps) => {
                if let Some(slot) = steps.get_mut(col) {
                    if slot.is_some() {
                        *slot = Option::None;
                    } else {
                        let len = lane.profile.gate_fraction;
                        *slot = Some(MelodicNote { semi: 0, vel: MEL_DEFAULT_VEL, slide: false, len, prob: 1.0, ratchet: 1 });
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
                if let Some(hit) = steps.get_mut(col).and_then(|s| s.iter_mut().find(|h| h.note == note)) {
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
                if let Some(hit) = steps.get_mut(col).and_then(|s| s.iter_mut().find(|h| h.note == note)) {
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
                steps.get(col).map(|s| s.iter().any(|h| h.note == note)).unwrap_or(false)
            }
            PatternData::Melodic(steps) => {
                matches!(steps.get(col), Some(Some(_)))
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
                    (Option::None, true) => step.push(DrumHit { note: voice_note, vel: 100, prob: 1.0, ratchet: 1 }),
                    (Some(pos), false) => { step.remove(pos); }
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

    fn lib_nav(&mut self, dg: i32, dp: i32) {
        let genre_count = self.current_genre_map().len();
        if genre_count == 0 {
            return;
        }
        let new_genre = (self.lib_genre as i32 + dg).clamp(0, genre_count as i32 - 1) as usize;
        self.lib_genre = new_genre;
        let pat_count = self.current_genre_map().get_index(new_genre).map(|(_, v)| v.len()).unwrap_or(0);
        if pat_count == 0 {
            self.lib_pattern = 0;
        } else {
            self.lib_pattern = (self.lib_pattern as i32 + dp).clamp(0, pat_count as i32 - 1) as usize;
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
    use crate::pattern::library::{GenreMap, Library, LibRole};
    use crate::pattern::model::{
        DrumHit, MelodicNote, Pattern, PatternData, Set,
    };

    /// Minimal in-memory library (one genre, one pattern per role) for deterministic tests.
    fn test_library() -> Library {
        let mut drums: GenreMap = GenreMap::new();
        let mut dsteps = vec![Vec::new(); 16];
        dsteps[2] = vec![DrumHit { note: 38, vel: 90, prob: 1.0, ratchet: 1 }];
        drums.insert(
            "techno".into(),
            vec![Pattern { name: "lib-drum".into(), desc: String::new(), length: 16, data: PatternData::Drums(dsteps) }],
        );

        let mut bass: GenreMap = GenreMap::new();
        let mut bsteps = vec![None; 16];
        bsteps[0] = Some(MelodicNote { semi: 3, vel: 1.0, slide: false, len: 0.5, prob: 1.0, ratchet: 1 });
        bass.insert(
            "acid".into(),
            vec![Pattern { name: "lib-bass".into(), desc: String::new(), length: 16, data: PatternData::Melodic(bsteps) }],
        );

        let mut synth: GenreMap = GenreMap::new();
        synth.insert(
            "dub".into(),
            vec![Pattern { name: "lib-synth".into(), desc: String::new(), length: 16, data: PatternData::Melodic(vec![None; 16]) }],
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
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
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
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
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
        assert!((app.set.swing - 0.66).abs() < 1e-6, "swing should clamp to 0.66");
        let cmds = app.apply(Action::AdjustSwing(1));
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::SetSwing(_))));
        // Clamp low: many -steps cannot drop below 0.5.
        for _ in 0..20 {
            app.apply(Action::AdjustSwing(-1));
        }
        assert!((app.set.swing - 0.5).abs() < 1e-6, "swing should clamp to 0.5");
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
            assert_eq!(steps.len(), shrunk, "drum steps Vec truncated to new length");
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
    fn panic_returns_panic_command_and_leaves_set_unchanged() {
        let mut app = new_app();
        let before = app.set.clone();
        let undo_len = app.undo.len();
        let cmds = app.apply(Action::Panic);
        assert_eq!(cmds, vec![UiCommand::Panic]);
        // No state change and no undo snapshot for a panic.
        assert_eq!(app.set, before, "panic must not mutate the Set");
        assert_eq!(app.undo.len(), undo_len, "panic must not push an undo snapshot");
    }

    // --- per-step prob / ratchet edits -----------------------------------

    #[test]
    fn adjust_prob_on_present_hit_clamps_and_emits_load() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.apply(Action::MoveCursor(0, 0));
        app.apply(Action::ToggleStep); // place a hit (prob defaults to 1.0)
        let cmds = app.apply(Action::AdjustProb(-1)); // 1.0 -> 0.9
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
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
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
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
        assert!(cmds.iter().any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
        let bd = profiles::DRUM_VOICES[0].note;
        let on_steps = |app: &App| -> Vec<usize> {
            if let PatternData::Drums(steps) = &app.focused_lane().pattern.data {
                steps.iter().enumerate()
                    .filter(|(_, s)| s.iter().any(|h| h.note == bd))
                    .map(|(i, _)| i).collect()
            } else { panic!("expected drums") }
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
            let on: Vec<usize> = steps.iter().enumerate()
                .filter(|(_, s)| s.iter().any(|h| h.note == bd))
                .map(|(i, _)| i).collect();
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
        assert!((app.set.swing - before).abs() < 1e-9, "undo restored prior swing");
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
            assert!((steps[0][0].prob - 1.0).abs() < 1e-6, "undo restored prior prob");
        }
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
}
