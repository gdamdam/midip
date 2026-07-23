//! midip-gui backend.
//!
//! A second driver over the existing midip engine, structurally mirroring
//! `main.rs`'s loop but without terminal rendering:
//!   GUI command → `Action` → `App::apply` → forward `UiCommand`s → `engine.tx`
//!   drain `engine.rx` → `App::on_engine_event` → forward → emit to the webview.
//!
//! The engine (`spawn_engine`) and all editing/domain logic (`App`) are reused
//! verbatim; this crate only translates, snapshots, and manages the Tauri
//! lifecycle. The GUI thread never performs MIDI scheduling — that stays on the
//! engine thread, exactly as in the TUI.

mod command;
mod dto;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use midip::app::{Action, App};
use midip::devices::profiles::{self, default_profiles, DRUM_VOICES};
use midip::engine::{spawn_engine, EngineEvent, UiCommand};
use midip::link::{AbletonLink, LinkClock};
use midip::pattern::library::Library;
use midip::pattern::model::{LaneKind, Set};
use midip::pattern::refs::PatternRef;
use tauri::{Emitter, Manager};

use command::{command_lane, gui_to_actions, target_cell, GuiCommand};
use dto::{LibraryDto, Snapshot};

/// Editor horizontal page size — matches `App`'s `VISIBLE_STEPS`.
const VISIBLE_STEPS: usize = 16;

/// The authoritative GUI-side state: a headless `App` plus the engine command
/// sender. Held behind a `Mutex` in `GuiState`. Never locked across a blocking
/// channel op or a thread join.
pub struct Core {
    pub app: App,
    cmd_tx: Sender<UiCommand>,
    data_dir: PathBuf,
}

impl Core {
    pub fn new(app: App, cmd_tx: Sender<UiCommand>, data_dir: PathBuf) -> Self {
        Core {
            app,
            cmd_tx,
            data_dir,
        }
    }

    fn forward(&self, cmds: Vec<UiCommand>) {
        for c in cmds {
            // A closed channel means the engine is gone (shutting down); dropping
            // the command is correct — never block the GUI on the engine.
            let _ = self.cmd_tx.send(c);
        }
    }

    /// (rows, cols) of the focused lane's grid, mirroring `App::grid_dims`.
    fn grid_dims(&self) -> (usize, usize) {
        let Some(lane) = self.app.set.lanes.get(self.app.focus) else {
            return (0, 0);
        };
        let cols = lane.pattern.length;
        let rows = match lane.pattern.kind() {
            LaneKind::Drums => DRUM_VOICES.len(),
            LaneKind::Melodic => 1,
        };
        (rows, cols)
    }

    /// Focus `lane` and move the editor cursor to `(row,col)`, clamped to the
    /// lane's grid. `App`'s cursor fields are public, so this positions the exact
    /// same state the TUI's key handlers use — the subsequent edit `Action` then
    /// reuses `App::apply` unchanged. Caller must have bounds-checked `lane`.
    fn place_cursor(&mut self, lane: usize, row: usize, col: usize) {
        let cmds = self.app.apply(Action::FocusLane(lane));
        self.forward(cmds);
        let (rows, cols) = self.grid_dims();
        if cols == 0 {
            return;
        }
        self.app.cur_col = col.min(cols - 1);
        self.app.cur_row = if rows == 0 { 0 } else { row.min(rows - 1) };
        self.app.step_scroll = (self.app.cur_col / VISIBLE_STEPS) * VISIBLE_STEPS;
    }

    /// Apply a GUI command: bounds-check the lane, position the cursor for
    /// cell-targeted edits, then run the translated `Action`s through
    /// `App::apply`, forwarding every resulting engine command.
    pub fn dispatch(&mut self, cmd: GuiCommand) {
        if let Some(lane) = command_lane(&cmd) {
            if lane >= self.app.set.lanes.len() {
                self.app.set_status("no such lane");
                return;
            }
        }
        // Routing commands read `route_editor_lane` (+ the port list for port
        // cycling); prime that state the same way `OpenRouteEditor` would.
        if let Some((lane, needs_ports)) = command::route_prep(&cmd) {
            self.app.route_editor_lane = lane;
            if needs_ports {
                self.app.route_editor_ports = midip::midi::ports::list_output_ports();
            }
        }
        if let Some((lane, row, col)) = target_cell(&cmd) {
            self.place_cursor(lane, row, col);
        }
        for action in gui_to_actions(&cmd) {
            let cmds = self.app.apply(action);
            self.forward(cmds);
        }
    }

    /// Load (or queue, if playing) a vendored library pattern into its role's
    /// lane. Reuses `App::launch_ref`, which resolves the ref against the loaded
    /// library, updates the set, and returns the correct Load/Queue command.
    pub fn load_library_pattern(&mut self, role: String, genre: String, name: String) {
        let pref = PatternRef::Vendored { role, genre, name };
        let cmds = self.app.launch_ref(&pref);
        self.forward(cmds);
    }

    /// Place a melodic note of a specific absolute MIDI `pitch` at `(lane,col)`,
    /// driving the engine's note-input path (which scale-folds on placement, so a
    /// click on a non-scale row snaps to the nearest degree). Inverts
    /// `resolve_melodic_pitch` (root+semi+transpose+12·octave) to derive the
    /// relative semitone, then splits it into the note-input octave + offset.
    pub fn place_note(&mut self, lane: usize, col: usize, pitch: u8) {
        if lane >= self.app.set.lanes.len() {
            return;
        }
        self.place_cursor(lane, 0, col);
        let l = &self.app.set.lanes[lane];
        if l.pattern.kind() != LaneKind::Melodic {
            return;
        }
        let desired =
            pitch as i32 - l.effective_root() as i32 - l.transpose as i32 - 12 * l.octave as i32;
        // Open note-input (resets note_input_octave to 0), then set our own octave
        // + offset so `NoteInputPlace` reconstructs `desired = offset + oct*12`.
        let c0 = self.app.apply(Action::OpenNoteInput);
        self.forward(c0);
        self.app.note_input_octave = desired.div_euclid(12).clamp(-10, 10) as i8;
        let offset = desired.rem_euclid(12) as i8;
        let c1 = self.app.apply(Action::NoteInputPlace(offset));
        self.forward(c1);
        let c2 = self.app.apply(Action::CloseNoteInput);
        self.forward(c2);
    }

    /// Cue a library pattern as an isolated preview on its role's lane WITHOUT
    /// mutating the committed set. Mirrors `Action::Audition` (which is bound to
    /// the TUI's lib-selection cursor) using the public `AuditionPreview`.
    pub fn audition(&mut self, role: String, genre: String, name: String) {
        let pref = PatternRef::Vendored {
            role: role.clone(),
            genre: genre.clone(),
            name: name.clone(),
        };
        let Some(lane) = pref.role_lane_hint() else {
            return;
        };
        if lane >= self.app.set.lanes.len() {
            return;
        }
        let Some(pat) = self.app.library.find(&role, &genre, &name).cloned() else {
            self.app.set_status("pattern not found");
            return;
        };
        // Gate: don't collide with a live (playing, unmuted) lane.
        if self.app.engine_playing && !self.app.set.lanes[lane].mute {
            self.app.set_status("Mute lane to audition (it's live)");
            return;
        }
        self.app.set_status(format!("Auditioning {name}"));
        self.app.audition = Some(midip::app::AuditionPreview {
            lane,
            pattern: pat.clone(),
        });
        self.forward(vec![UiCommand::LoadPattern { lane, pattern: pat }]);
    }

    /// End any active audition, reverting the previewed lane to its committed pattern.
    pub fn stop_audition(&mut self) {
        if let Some(prev) = self.app.audition.take() {
            let pat = self.app.set.lanes[prev.lane].pattern.clone();
            self.forward(vec![UiCommand::LoadPattern {
                lane: prev.lane,
                pattern: pat,
            }]);
            self.app.set_status("Audition stopped");
        }
    }

    /// Toggle a library pattern's favorite flag and persist the favorites file.
    pub fn toggle_favorite(&mut self, role: String, genre: String, name: String) {
        let pref = PatternRef::Vendored { role, genre, name };
        let now = self.app.favorites.toggle(pref);
        let _ = midip::pattern::store::save_favorites(&self.data_dir, &self.app.favorites);
        self.app
            .set_status(if now { "Favorited" } else { "Unfavorited" });
    }

    pub fn on_engine_event(&mut self, ev: EngineEvent) {
        let cmds = self.app.on_engine_event(ev);
        self.forward(cmds);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::build(&self.app)
    }

    pub fn library(&self) -> LibraryDto {
        LibraryDto::build(&self.app.library, &self.app.favorites)
    }
}

pub struct GuiState {
    pub core: Mutex<Core>,
    /// Taken out exactly once, at shutdown, to join the engine thread.
    engine_join: Mutex<Option<JoinHandle<()>>>,
    /// A top-level clone of the engine command sender, used at shutdown so we can
    /// send `Quit` WITHOUT locking `core` (the lock must never be held across a join).
    cmd_tx: Sender<UiCommand>,
}

/// Everything produced by booting the engine + app together.
struct Boot {
    core: Core,
    evt_rx: Receiver<EngineEvent>,
    join: JoinHandle<()>,
    cmd_tx: Sender<UiCommand>,
}

/// Spawn the engine and build the `App` over the same `Set`. `link` is injected
/// so production uses `AbletonLink` while tests use `FakeLink` (no hardware).
fn boot(link: Box<dyn LinkClock>, set: Set, library: Library, data_dir: PathBuf) -> Boot {
    let handle = spawn_engine(set.clone(), link);
    let app = App::new(set, library);
    let cmd_tx = handle.tx.clone();
    let core = Core::new(app, handle.tx, data_dir);
    Boot {
        core,
        evt_rx: handle.rx,
        join: handle.join,
        cmd_tx,
    }
}

// --- Tauri commands ------------------------------------------------------

#[tauri::command]
fn gui_snapshot(state: tauri::State<GuiState>) -> Snapshot {
    state.core.lock().unwrap().snapshot()
}

#[tauri::command]
fn gui_dispatch(cmd: GuiCommand, state: tauri::State<GuiState>) -> Snapshot {
    let mut core = state.core.lock().unwrap();
    core.dispatch(cmd);
    core.snapshot()
}

#[tauri::command]
fn gui_library(state: tauri::State<GuiState>) -> LibraryDto {
    state.core.lock().unwrap().library()
}

#[tauri::command]
fn gui_audition(
    role: String,
    genre: String,
    name: String,
    state: tauri::State<GuiState>,
) -> Snapshot {
    let mut core = state.core.lock().unwrap();
    core.audition(role, genre, name);
    core.snapshot()
}

#[tauri::command]
fn gui_place_note(lane: usize, col: usize, pitch: u8, state: tauri::State<GuiState>) -> Snapshot {
    let mut core = state.core.lock().unwrap();
    core.place_note(lane, col, pitch);
    core.snapshot()
}

#[tauri::command]
fn gui_stop_audition(state: tauri::State<GuiState>) -> Snapshot {
    let mut core = state.core.lock().unwrap();
    core.stop_audition();
    core.snapshot()
}

#[tauri::command]
fn gui_toggle_favorite(
    role: String,
    genre: String,
    name: String,
    state: tauri::State<GuiState>,
) -> LibraryDto {
    let mut core = state.core.lock().unwrap();
    core.toggle_favorite(role, genre, name);
    core.library()
}

#[tauri::command]
fn gui_load_pattern(
    role: String,
    genre: String,
    name: String,
    state: tauri::State<GuiState>,
) -> Snapshot {
    let mut core = state.core.lock().unwrap();
    core.load_library_pattern(role, genre, name);
    core.snapshot()
}

#[tauri::command]
fn gui_set_list(state: tauri::State<GuiState>) -> Vec<SetEntry> {
    let core = state.core.lock().unwrap();
    let dir = core.data_dir.join("sets");
    midip::pattern::store::list_sets(&dir)
        .unwrap_or_default()
        .into_iter()
        .map(|p| SetEntry {
            name: p
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: p.to_string_lossy().to_string(),
        })
        .collect()
}

#[derive(serde::Serialize)]
pub struct SetEntry {
    pub name: String,
    pub path: String,
}

#[tauri::command]
fn gui_output_ports() -> Vec<String> {
    midip::midi::ports::list_output_ports()
}

// --- Event pump ----------------------------------------------------------

/// Drain engine events on a dedicated thread. Per event: acquire the `core`
/// lock BRIEFLY (never across `recv`), fold the event into `App`, then release
/// the lock before emitting to the webview. `Playhead` (the only high-rate
/// event) emits a lightweight `transport` payload; everything else emits a full
/// structural `snapshot`.
fn pump(handle: tauri::AppHandle, rx: Receiver<EngineEvent>) {
    while let Ok(ev) = rx.recv() {
        let lightweight = matches!(ev, EngineEvent::Playhead { .. });
        let payload = {
            let state = handle.state::<GuiState>();
            let mut core = state.core.lock().unwrap();
            core.on_engine_event(ev);
            if lightweight {
                Payload::Transport(core.snapshot().transport)
            } else {
                Payload::Snapshot(Box::new(core.snapshot()))
            }
        }; // lock released before emitting
        match payload {
            Payload::Transport(t) => {
                let _ = handle.emit("transport", t);
            }
            Payload::Snapshot(s) => {
                let _ = handle.emit("snapshot", s);
            }
        }
    }
}

enum Payload {
    Transport(dto::TransportDto),
    // Boxed: `Snapshot` is far larger than `TransportDto`, so keep the enum small.
    Snapshot(Box<Snapshot>),
}

/// Send `Quit` and join the engine thread. Mirrors `main.rs`'s `EngineQuitGuard`
/// so the engine performs its all-notes-off/panic flush before the process
/// exits — even if the window is closed mid-playback. Never holds the `core`
/// lock while joining.
fn shutdown(handle: &tauri::AppHandle) {
    let state = handle.state::<GuiState>();
    let _ = state.cmd_tx.send(UiCommand::Quit);
    let join = state.engine_join.lock().unwrap().take();
    if let Some(join) = join {
        let _ = join.join();
    }
}

/// Desktop entry point.
pub fn run() {
    let data_dir = midip::config::data_dir();
    profiles::init_user_catalog(&data_dir);

    let (library, lib_status) = match Library::load(&midip::config::patterns_dir()) {
        Ok(lib) => (lib, String::from("library loaded")),
        Err(e) => (
            Library::empty(),
            format!("library load failed: {e} (running with empty library)"),
        ),
    };

    let set = Set::default_set(default_profiles());
    let link: Box<dyn LinkClock> = Box::new(AbletonLink::new(set.bpm));
    let Boot {
        mut core,
        evt_rx,
        join,
        cmd_tx,
    } = boot(link, set, library, data_dir);
    core.app.set_status(lib_status);

    tauri::Builder::default()
        .manage(GuiState {
            core: Mutex::new(core),
            engine_join: Mutex::new(Some(join)),
            cmd_tx,
        })
        .invoke_handler(tauri::generate_handler![
            gui_snapshot,
            gui_dispatch,
            gui_library,
            gui_load_pattern,
            gui_audition,
            gui_place_note,
            gui_stop_audition,
            gui_toggle_favorite,
            gui_set_list,
            gui_output_ports,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let rx = evt_rx.clone();
            std::thread::spawn(move || pump(handle, rx));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error building tauri application")
        .run(move |handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                shutdown(handle);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use midip::link::FakeLink;
    use midip::midi::ports::RecordingSink;
    use midip::pattern::model::{Pattern, PatternData};

    /// A `Core` with a dangling command channel (no live engine) — enough to test
    /// dispatch, cursor clamping and snapshot generation without a thread.
    fn core_no_engine() -> Core {
        let (tx, _rx) = unbounded();
        let app = App::new(Set::default_set(default_profiles()), Library::empty());
        Core::new(app, tx, std::env::temp_dir())
    }

    #[test]
    fn snapshot_reflects_default_set() {
        let core = core_no_engine();
        let snap = core.snapshot();
        assert_eq!(snap.lanes.len(), 3, "default set has drums/bass/synth");
        assert_eq!(snap.focused_lane, 0);
        assert_eq!(snap.lanes[0].kind, "drums");
        assert!(snap.transport.set_bpm > 0.0);
        // Focused (drum) pattern exposes the standard kit rows.
        assert_eq!(snap.focused_pattern.voices.len(), DRUM_VOICES.len());
    }

    #[test]
    fn toggle_step_roundtrips_through_app() {
        let mut core = core_no_engine();
        // Toggle BD (row 0) at step 0 on the drum lane.
        core.dispatch(GuiCommand::ToggleStep {
            lane: 0,
            row: 0,
            col: 0,
        });
        let snap = core.snapshot();
        assert!(
            !snap.focused_pattern.drum_steps[0].is_empty(),
            "step 0 should now carry a hit"
        );
        // Toggle again clears it.
        core.dispatch(GuiCommand::ToggleStep {
            lane: 0,
            row: 0,
            col: 0,
        });
        let snap = core.snapshot();
        assert!(snap.focused_pattern.drum_steps[0].is_empty());
    }

    #[test]
    fn invalid_lane_index_is_dropped_not_panicked() {
        let mut core = core_no_engine();
        core.dispatch(GuiCommand::ToggleMute(999));
        core.dispatch(GuiCommand::ToggleStep {
            lane: 999,
            row: 0,
            col: 0,
        });
        core.dispatch(GuiCommand::FocusLane(999));
        // Focus unchanged; no panic.
        assert_eq!(core.snapshot().focused_lane, 0);
    }

    #[test]
    fn cursor_clamps_to_grid_for_out_of_range_cell() {
        let mut core = core_no_engine();
        // Row/col far past the 16-step, 10-voice drum grid.
        core.dispatch(GuiCommand::ToggleStep {
            lane: 0,
            row: 500,
            col: 500,
        });
        let snap = core.snapshot();
        assert!(snap.selection.row < DRUM_VOICES.len());
        assert!(snap.selection.col < snap.focused_pattern.length);
    }

    #[test]
    fn snapshot_handles_pattern_lengths_1_through_64() {
        for len in 1..=64usize {
            let mut app = App::new(Set::default_set(default_profiles()), Library::empty());
            // Resize the focused drum lane's pattern to `len` and rebuild.
            let mut steps = vec![Vec::new(); len];
            if len > 0 {
                steps[len - 1] = vec![midip::pattern::model::DrumHit {
                    note: 36,
                    vel: 100,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: midip::pattern::model::TrigCond::Always,
                }];
            }
            app.set.lanes[0].pattern = Pattern {
                name: "t".into(),
                desc: String::new(),
                length: len,
                data: PatternData::Drums(steps),
                id: midip::persist::Id::nil(),
                cc: Vec::new(),
            };
            let (tx, _rx) = unbounded();
            let core = Core::new(app, tx, std::env::temp_dir());
            let snap = core.snapshot();
            assert_eq!(snap.focused_pattern.length, len);
            assert_eq!(snap.focused_pattern.drum_steps.len(), len);
            // Cursor at the last cell must be representable.
            assert!(snap.focused_pattern.cc.len() == len);
        }
    }

    #[test]
    fn place_note_lands_on_requested_pitch_on_chromatic_scale() {
        let mut app = App::new(Set::default_set(default_profiles()), Library::empty());
        // Lane 1 is the melodic bass lane; Chromatic means no scale folding, so the
        // placed pitch must equal the request exactly.
        app.set.lanes[1].scale = midip::music::scale::Scale::Chromatic;
        app.focus = 1;
        let (tx, _rx) = unbounded();
        let mut core = Core::new(app, tx, std::env::temp_dir());

        for target in [40u8, 50, 55, 62, 69] {
            core.place_note(1, 0, target);
            let snap = core.snapshot();
            let notes = &snap.focused_pattern.melodic_steps[0];
            assert_eq!(notes.len(), 1, "one note at step 0");
            assert_eq!(notes[0].pitch, target, "placed pitch matches request");
        }
    }

    #[test]
    fn commands_work_without_midi_hardware() {
        // Boot a REAL engine thread with a FakeLink (no ports, no hardware) and
        // drive it through the GUI dispatch path, then shut it down cleanly.
        let set = Set::default_set(default_profiles());
        let handle = spawn_engine(set.clone(), Box::new(FakeLink::new()));
        let cmd_tx = handle.tx.clone();
        let mut core = Core::new(
            App::new(set, Library::empty()),
            handle.tx,
            std::env::temp_dir(),
        );

        core.dispatch(GuiCommand::TogglePlay);
        core.dispatch(GuiCommand::SetBpm(140.0));
        core.dispatch(GuiCommand::ToggleStep {
            lane: 0,
            row: 0,
            col: 4,
        });
        core.dispatch(GuiCommand::Panic);
        core.dispatch(GuiCommand::TogglePlay);

        // Clean shutdown: Quit then join must return (engine flushed notes off).
        let _ = cmd_tx.send(UiCommand::Quit);
        handle
            .join
            .join()
            .expect("engine thread joins cleanly after Quit");
    }

    #[test]
    fn recording_sink_is_reachable_for_headless_checks() {
        // Sanity: the headless test seam the engine uses is available to us too.
        let mut sink = RecordingSink::default();
        let _ = &mut sink;
    }
}
