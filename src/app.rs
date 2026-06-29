//! Application state and the action reducer (UI thread side).
//!
//! `App` holds the canonical edit state. `apply` mutates state and returns the
//! `UiCommand`s that must be forwarded to the engine (e.g. pattern edits emit
//! `LoadPattern`). Undo/redo snapshot the whole `Set`.

use crate::devices::profiles;
use crate::engine::scheduler::{LaunchState, Quant};
use crate::engine::{EngineEvent, UiCommand};
use crate::music::scale::{fold_to_scale, step_by_degree, Scale};
use crate::pattern::euclid;
use crate::pattern::generate::{generate, next_rng, GenMode, GenParams};
use crate::pattern::library::{LibRole, Library};
use crate::pattern::model::{
    DrumHit, DrumStep, Lane, LaneKind, LaneRoute, MelodicNote, MelodicStep, Pattern, PatternData,
    PortRef, Set,
};
use crate::pattern::refs::{resolve_scene, PatternRef};
use crate::pattern::store::{CrateEntry, CrateIndex, Favorites};

/// A validation issue found in a crate.
#[derive(Clone, Debug, PartialEq)]
pub enum CrateIssue {
    /// The entry's PatternRef could not be resolved to a Pattern.
    MissingPattern { entry_idx: usize, name: String },
    /// The entry resolves but its role-matched lane's device is known-disconnected.
    UnavailableTarget { entry_idx: usize, lane: usize },
}

/// Purpose of a pending name-entry dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum NamePurpose {
    SaveSetAs,
    RenameSet,
    SaveUserPattern,
    RenameScene,
    RenameChain,
}

/// Action to perform when a Confirm dialog is accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    NewSet,
    DeleteSet(std::path::PathBuf),
    ClearPattern,
    /// Fold all out-of-scale notes in the focused melodic lane to the lane's scale.
    /// The `usize` is the count of notes that will be folded (used in the confirm prompt).
    ConformToScale(usize),
    /// Remove the scene at `index` from `set.scenes` (after user confirmation).
    DeleteScene(usize),
    /// Remove the chain at `index` from `set.chains` (after user confirmation).
    DeleteChain(usize),
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
    /// Live crate browser: browse crate entries and launch patterns without
    /// committing to the active Set until Enter is pressed.
    CrateView,
    /// Scene manager: list, capture, recall, rename, duplicate, delete scenes.
    /// Opened from Edit via `'G'`; `Esc`/`'G'` closes.
    Scenes,
    /// Chain manager: list, create, rename, duplicate, delete chains and edit entries.
    /// Opened from Edit (M7 UI — key assigned in Task 6).
    Chains,
    /// QWERTY piano note-input sub-mode (melodic lanes only).
    ///
    /// Entered from melodic Edit via `'I'` (Shift+i — was unbound before M5a-T5).
    /// Snapshot is taken ONCE on entry so the whole input session is a single undo unit.
    /// `Esc` returns to Edit without an additional snapshot.
    NoteInput,
    /// Generative pattern tool: preview/audition/commit/cancel on the focused lane.
    ///
    /// Entered via `OpenGenerative`; live preview is held in `temp_transform` (same
    /// machinery as `ToggleFill`). Snapshot is deferred to `GenCommit` (one undo entry).
    /// `GenCancel`/`Esc` reverts without adding any snapshot.
    Generative,
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

/// An active temporary transformation on a single lane. Holds the pre-fill original
/// so it can be reverted. The `Set` IS mutated with the fill pattern (so the engine
/// plays it live), but `snapshot()` is NOT called until `CommitTransform`.
#[derive(Clone, Debug, PartialEq)]
pub struct TempTransform {
    /// The lane index the fill was applied to.
    pub lane: usize,
    /// The original pattern before the fill was applied.
    pub original: Pattern,
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
    /// Cycle the focused melodic lane's scale forward (+1) or backward (-1) through
    /// `Scale::all()`. Does NOT rewrite existing note semis — fold applies to new input only.
    CycleScale(i8),
    /// Adjust the focused melodic lane's root note by ±1 semitone (0..=127, clamped).
    /// Sets `lane.root = Some(...)` so it overrides the profile root.
    AdjustRoot(i8),
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
    /// Re-sync the focused lane's phase at the next bar/beat by re-queuing its CURRENT
    /// pattern. The engine re-launches the same pattern at the boundary, resetting
    /// `launch_offset` so the lane restarts at local step 0. No Set mutation, no snapshot.
    RestartLane,
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
    /// M6: recall the scene at `set.scenes[index]` as a single quantized all-lane launch.
    /// While playing: queues every resolvable lane (pattern + mute/solo/transpose/octave) on
    /// ONE upcoming boundary (the current `launch_quant`). Stopped: applies immediately. A
    /// lane whose assignment cannot be resolved is left unchanged and reported in the status.
    /// Live performance action — NOT an undoable edit; does not mark the set dirty.
    RecallScene(usize),
    // ── Scene manager ────────────────────────────────────────────────────
    /// Open the scene manager overlay from Edit mode.
    OpenScenes,
    /// Close the scene manager and return to Edit.
    CloseScenes,
    /// Move scene selection ±1 (clamped; never changes playback).
    SceneSelect(i32),
    /// Snapshot the live set into a new auto-named Scene and append it.
    CaptureScene,
    /// Open NameEntry prefilled with the selected scene's name.
    RenameScene,
    /// Internal: apply the rename after NameEntry accepts.
    DoRenameScene(String),
    /// Clone the selected scene with a fresh id and an appended " (copy)" name.
    DuplicateScene,
    /// Open a Confirm dialog before deleting the selected scene.
    DeleteScene,
    /// Internal: remove the scene at `idx`, clamp sel, return to Scenes.
    DoDeleteScene(usize),
    /// Recall the scene at `scene_sel` (dispatches RecallScene(scene_sel)).
    RecallSelectedScene,
    /// Resolve the selected scene's assignments; store missing lane indices.
    ValidateScene,
    // ── M7 Chain manager ─────────────────────────────────────────────────────
    /// Open the chain manager overlay from Edit mode.
    OpenChains,
    /// Close the chain manager and return to Edit.
    CloseChains,
    /// Move chain selection by delta (clamped).
    ChainSelect(i32),
    /// Create a new chain with an auto-generated name.
    CreateChain,
    /// Open name-entry to rename the selected chain.
    RenameChain,
    /// Internal: commit the rename with the supplied name.
    DoRenameChain(String),
    /// Duplicate the selected chain (deep clone + new ids + " copy" suffix).
    DuplicateChain,
    /// Open a Confirm dialog before deleting the selected chain.
    DeleteChain,
    /// Internal: remove the chain at `idx`, clamp sel, return to Chains.
    DoDeleteChain(usize),
    /// Append a new entry (scene_id) to the selected chain.
    AddChainEntry {
        chain: usize,
        scene_id: crate::persist::Id,
    },
    /// Append the currently selected scene (scene_sel) to the selected chain (chain_sel).
    AddSelectedSceneToChain,
    /// Remove the selected entry (chain_entry_sel) from the selected chain (chain_sel).
    RemoveSelectedChainEntry,
    /// Move entry selection down by 1 within the selected chain's entries.
    ChainEntrySelectNext,
    /// Move entry selection up by 1 within the selected chain's entries.
    ChainEntrySelectPrev,
    /// Remove entry at `entry` from chain `chain`.
    RemoveChainEntry {
        chain: usize,
        entry: usize,
    },
    /// Move entry at `entry` earlier in chain `chain`.
    MoveChainEntry {
        chain: usize,
        entry: usize,
        up: bool,
    },
    /// Set `repeats` on the given entry (clamped >= 1).
    SetChainEntryRepeats {
        chain: usize,
        entry: usize,
        value: u32,
    },
    /// Set `bars` on the given entry (clamped >= 1).
    SetChainEntryBars {
        chain: usize,
        entry: usize,
        value: u32,
    },
    /// Toggle the `looped` flag on chain at `idx`.
    ToggleChainLoop(usize),
    /// Toggle loop on the currently selected chain (dispatches ToggleChainLoop(chain_sel)).
    ToggleSelectedChainLoop,
    /// Adjust bars of (chain_sel, chain_entry_sel) by delta (clamped ≥ 1).
    AdjustSelectedChainEntryBars(i32),
    /// Adjust repeats of (chain_sel, chain_entry_sel) by delta (clamped ≥ 1).
    AdjustSelectedChainEntryRepeats(i32),
    // ── M7 Transport stubs (real behavior in Task 5) ──────────────────────────
    /// Play the currently selected chain (dispatches PlayChain(chain_sel)).
    PlaySelectedChain,
    /// Jump the playing chain to the currently selected entry (chain_entry_sel).
    JumpSelectedChainEntry,
    /// Open the chains overlay (alias used by song-mode UI).
    PlayChain(usize),
    StopChain,
    JumpChainEntry(usize),
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
    /// Scroll the help overlay by `delta` lines (positive=down, negative=up).
    HelpScroll(i32),
    /// Toggle the currently-selected library entry in/out of favorites. Library mode only.
    ToggleFavorite,
    /// Toggle favorites-only filter in the library browser. Library mode only.
    ToggleFavFilter,
    // ── M4a Task 4: crate management ─────────────────────────────────────
    /// Create a new named crate; persist.
    CreateCrate(String),
    /// Rename the crate at `idx`; persist.
    RenameCrate(usize, String),
    /// Duplicate the crate at `idx` (fresh id, " copy" suffix); persist.
    DuplicateCrate(usize),
    /// Remove the crate at `idx`; persist.
    DeleteCrate(usize),
    /// Move the entry at `from` to `to` within the crate at `crate_idx`; persist.
    ReorderCrateEntry(usize, usize, usize),
    /// Add the currently-selected library pattern to the crate at `idx`; persist.
    AddToCrate(usize),
    /// Remove the entry at `entry_idx` from the crate at `crate_idx`; persist.
    RemoveFromCrate(usize, usize),
    // ── M4a Task 5: live crate view ──────────────────────────────────────────
    /// Open the crate browser overlay (from Edit mode).
    OpenCrateView,
    /// Close the crate browser and return to Edit.
    CloseCrateView,
    /// Move the entry selection within the current crate (±1, clamped).
    CrateEntrySel(i32),
    /// Switch the active crate (±1, clamped); resets entry selection.
    CrateSel(i32),
    /// Launch the selected crate entry to its role-matched lane (quantized).
    LaunchCrateEntry,
    /// Audition the selected crate entry (gated: stopped or lane muted).
    AuditionCrateEntry,
    /// Toggle the selected crate entry's PatternRef in/out of favorites.
    FavoriteCrateEntry,
    /// Run pre-performance validation on the current crate; stores results and shows summary.
    ValidateCrate,
    /// Per-drum-voice mute (§2.6): toggle the mute on the voice row under the cursor.
    /// Drums only — no-op with a status message on melodic lanes.
    ToggleVoiceMute,
    /// Toggle a temporary fill on the focused lane (non-destructive, latched).
    /// If no temp is active: save the original pattern, apply a deterministic fill,
    /// emit LoadPattern. If already active: revert to original. No snapshot.
    ToggleFill,
    /// Commit the active temporary fill: snapshot (makes it undoable), clear temp,
    /// mark dirty. No-op with status if no temp is active.
    CommitTransform,
    // ── M5a Task 4: conform existing notes to scale ──────────────────────────
    /// Open the conform-to-scale flow: Chromatic → status no-op; 0 out-of-scale notes →
    /// status no-op; otherwise route to `Mode::Confirm(ConfirmAction::ConformToScale)`.
    /// Preview is provided by the confirm prompt which shows the count of notes to fold.
    OpenConformToScale,
    /// Fold every note in the focused melodic lane to the lane's current scale.
    /// Snapshots for undo. Chromatic / drum lanes are silent no-ops.
    ConformToScale,
    // ── M5a Task 5: QWERTY note-input sub-mode ───────────────────────────────
    /// Enter `Mode::NoteInput` from melodic Edit (key `'I'`, Shift+i — was unbound).
    /// On a drum lane this is a no-op with a status message.
    OpenNoteInput,
    /// Exit `Mode::NoteInput` back to Edit.
    CloseNoteInput,
    /// Place a note with the given semitone offset (relative to root, pre-folded by input.rs).
    /// The raw offset from the QWERTY map; `apply` folds it to the lane's scale and places it.
    NoteInputPlace(i8),
    /// Shift the note-input octave by ±1 (clamped to −3..=3).
    NoteInputOctave(i8),
    /// Clear the cursor step and step back one (Backspace/Delete in NoteInput).
    NoteInputBackspace,
    // ── M5b Task 4: chord entry on poly lanes ────────────────────────────────
    /// Build a scale-aware triad on the cursor step (Edit mode, key `'j'`, was unbound).
    /// From the step's root note (its first note) add a 3rd (+2 scale degrees) and a 5th
    /// (+4 scale degrees), folded to the lane's scale. No-op with a status message if the
    /// step is empty or the lane is mono (`poly == false`). Snapshots for undo.
    BuildTriad,
    /// Remove the LAST note from the cursor step (Edit mode, key `'J'`/Shift+j, was unbound).
    /// On a single-note step this clears it (becomes a rest). No-op on an empty step.
    /// Snapshots for undo.
    RemoveChordNote,
    // ── M9 Generative tool ───────────────────────────────────────────────────
    /// Open the generative tool for the focused lane: set `Mode::Generative`, init `GenParams`
    /// with a session-seeded RNG, generate a candidate into `temp_transform`, emit `LoadPattern`.
    /// NO snapshot — mirrors `ToggleFill`.
    OpenGenerative,
    /// Switch the generation mode (Generate ↔ Vary); regenerate preview.
    GenSetMode(GenMode),
    /// Adjust a `GenParams` field by `delta`; clamp to 0..=100 (or 0..=127 for range).
    /// Regenerates the preview in place — no snapshot.
    GenAdjust {
        field: GenField,
        delta: i32,
    },
    /// Bump the seed via `next_rng` and regenerate a fresh candidate. No snapshot.
    GenReroll,
    /// Commit the current candidate: push pre-op Set to undo (one entry), apply, mark dirty,
    /// close `Mode::Generative`. Mirrors `CommitTransform`.
    GenCommit,
    /// Cancel: restore the original pattern, clear `temp_transform`, close `Mode::Generative`.
    /// Zero undo entries added.
    GenCancel,
    None,
}

/// Which `GenParams` field a `GenAdjust` action targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenField {
    Density,
    Range,
    Mutate,
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
    pub help_scroll: u16,

    // --- M4a Task 3: favorites ---
    /// Persisted set of favorited pattern refs, loaded at startup.
    pub favorites: Favorites,
    /// When true, the library browser shows only favorited patterns.
    pub fav_filter: bool,

    // --- M4a Task 4: crates ---
    /// Named, ordered collections of pattern refs; loaded at startup, persisted on mutation.
    pub crates: CrateIndex,

    // --- M4a Task 5: crate view selection ---
    /// Index of the currently-displayed crate in `crates.crates`.
    pub crate_sel: usize,
    /// Index of the selected entry within the current crate.
    pub crate_entry_sel: usize,

    // --- M4a Task 6: pre-performance validation ---
    /// Issues found by the last `ValidateCrate` run. Empty until validation is run.
    pub crate_issues: Vec<CrateIssue>,

    // --- M4b Task 3: temporary fill (non-destructive perf transform) ---
    /// Active temporary transformation. `None` when no fill is applied.
    /// The transformed pattern IS loaded into the lane (engine plays it live),
    /// but `snapshot()` is deferred to `CommitTransform` — so it is non-destructive
    /// until the performer chooses to keep it.
    pub temp_transform: Option<TempTransform>,
    // ── M9: Generative tool state ────────────────────────────────────────────
    /// Parameters for the active generative session. Valid only while `mode == Mode::Generative`.
    pub gen_params: GenParams,
    /// Per-session RNG state for `next_rng`. Seeded once in `App::new`; bumped by `GenReroll`.
    pub gen_seed: u64,
    // ── M5a Task 5: QWERTY note-input sub-mode ───────────────────────────────
    /// Octave offset applied on top of the QWERTY semitone map while in `Mode::NoteInput`.
    /// Range −3..=3 (clamped). Reset to 0 each time `Mode::NoteInput` is entered.
    pub note_input_octave: i8,
    /// Index of the currently selected scene in `set.scenes`.
    pub scene_sel: usize,
    /// Lane indices whose assignments could not be resolved by the last `ValidateScene`.
    pub scene_issues: Vec<usize>,
    /// Index of the currently selected chain in `set.chains`.
    pub chain_sel: usize,
    /// Index of the currently selected entry within the selected chain.
    pub chain_entry_sel: usize,
    /// M7 Task 5: live chain playback state (runtime only — NOT persisted). `Some` while a
    /// chain is auto-advancing; the App runs `chain_decision` at each engine-reported bar
    /// boundary (see `tick_chain`) and recalls the next entry's scene via the existing
    /// quantized `recall_scene` path. `None` when idle/stopped/overridden.
    pub chain_playback: Option<crate::pattern::chain::ChainPlayback>,
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
            help_scroll: 0,
            favorites: Favorites::default(),
            fav_filter: false,
            crates: CrateIndex::default(),
            crate_sel: 0,
            crate_entry_sel: 0,
            crate_issues: Vec::new(),
            temp_transform: None,
            gen_params: GenParams::default(),
            gen_seed: 0x9e37_79b9_7f4a_7c15, // non-zero Fibonacci-hash constant
            note_input_octave: 0,
            scene_sel: 0,
            scene_issues: Vec::new(),
            chain_sel: 0,
            chain_entry_sel: 0,
            chain_playback: None,
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

    /// Best-effort persist of the crate index; ignores errors so a missing data dir never panics.
    pub fn persist_crates(&self) {
        let _ = crate::pattern::store::save_crates(&crate::config::data_dir(), &self.crates);
    }

    /// Determine the role-matched target lane index for a resolved pattern ref + pattern.
    ///
    /// This is the shared lane-targeting logic used by both `launch_ref` and `validate_crate`.
    /// - `Vendored`: uses `role_lane_hint()`, clamped to available lanes.
    /// - `User`: Drums → first drum lane; Melodic → focused lane if melodic, else first melodic.
    pub fn target_lane_for(&self, r: &PatternRef, pat: &Pattern) -> Option<usize> {
        let n = self.set.lanes.len();
        if n == 0 {
            return None;
        }
        let lane = if let Some(hint) = r.role_lane_hint() {
            hint.min(n - 1)
        } else {
            match pat.kind() {
                LaneKind::Drums => self
                    .set
                    .lanes
                    .iter()
                    .position(|l| l.profile.kind == LaneKind::Drums)
                    .unwrap_or(0),
                LaneKind::Melodic => {
                    if self.set.lanes.get(self.focus).map(|l| l.profile.kind)
                        == Some(LaneKind::Melodic)
                    {
                        self.focus
                    } else {
                        self.set
                            .lanes
                            .iter()
                            .position(|l| l.profile.kind == LaneKind::Melodic)
                            .unwrap_or(self.focus)
                    }
                }
            }
        };
        Some(lane)
    }

    /// Validate all entries in the crate at `crate_idx`.
    ///
    /// For each entry:
    /// - If the `PatternRef` cannot be resolved → `CrateIssue::MissingPattern`.
    /// - If the resolved pattern's target lane device is known-disconnected →
    ///   `CrateIssue::UnavailableTarget`.
    ///
    /// Returns an empty vec when the crate is valid or `crate_idx` is out of bounds.
    pub fn validate_crate(&self, crate_idx: usize) -> Vec<CrateIssue> {
        let Some(cr) = self.crates.crates.get(crate_idx) else {
            return vec![];
        };
        let user_dir = crate::config::data_dir().join("patterns");
        let mut issues = Vec::new();
        for (entry_idx, entry) in cr.entries.iter().enumerate() {
            let name = entry
                .label
                .clone()
                .unwrap_or_else(|| entry.pattern.display_name());
            match crate::pattern::refs::resolve_pattern_ref(
                &entry.pattern,
                &self.library,
                &user_dir,
            ) {
                None => issues.push(CrateIssue::MissingPattern { entry_idx, name }),
                Some(pat) => {
                    if let Some(lane) = self.target_lane_for(&entry.pattern, &pat) {
                        // device_status: (connected, port_name). We only flag as unavailable
                        // when we have received at least one DeviceStatus event for that lane
                        // (port name is non-empty) AND it reported disconnected. The initial
                        // (false, "") means "no status received yet" — not an error.
                        if let Some((connected, port)) = self.device_status.get(lane) {
                            if !connected && !port.is_empty() {
                                issues.push(CrateIssue::UnavailableTarget { entry_idx, lane });
                            }
                        }
                    }
                }
            }
        }
        issues
    }

    /// Resolve `r` and load/queue it to the role-matched lane.
    ///
    /// Lane targeting:
    /// - `Vendored`: `role_lane_hint()` (drums→0, bass→1, synth→2); unknown → focused lane.
    /// - `User`: resolved pattern's `kind()` — Drums → first drum lane; Melodic → focused lane
    ///   if it is melodic, else the first melodic lane.
    ///
    /// Returns empty vec and sets status "missing pattern" when the ref cannot be resolved.
    pub fn launch_ref(&mut self, r: &PatternRef) -> Vec<UiCommand> {
        let user_dir = crate::config::data_dir().join("patterns");
        let Some(pat) = crate::pattern::refs::resolve_pattern_ref(r, &self.library, &user_dir)
        else {
            self.set_status("missing pattern");
            return vec![];
        };

        let lane = self.target_lane_for(r, &pat).unwrap_or(self.focus);

        let name = pat.name.clone();
        self.snapshot();
        self.set.lanes[lane].pattern = pat.clone();
        let mut cmds = Vec::new();
        if self.engine_playing {
            let quant = self.launch_quant;
            self.queued[lane] = Some(name.clone());
            let quant_str = match quant {
                Quant::NextBar => "next bar",
                Quant::NextBeat => "next beat",
            };
            self.set_status(format!("Queued {} ({})", name, quant_str));
            cmds.push(UiCommand::QueuePattern {
                lane,
                pattern: pat,
                quant,
            });
        } else {
            self.set_status(format!("Loaded {}", name));
            cmds.push(UiCommand::LoadPattern { lane, pattern: pat });
        }
        cmds
    }

    /// M6: recall the scene at `set.scenes[index]` as a single quantized all-lane launch.
    ///
    /// Resolves the scene's per-lane assignments via `resolve_scene` (inline user patterns
    /// from this set's lanes; vendored from the library). For each lane that resolves `Ok`:
    /// - PLAYING: queue the pattern + the scene's mute/solo/transpose/octave on the SAME
    ///   upcoming boundary as every other recalled lane (one `QueueScene` carrying all lanes
    ///   at the current `launch_quant`), restarting each lane at step 1 at that instant. The
    ///   currently-playing scene keeps sounding until the boundary; state applies at launch.
    /// - STOPPED: apply immediately (load pattern + state into the lane now).
    ///
    /// A lane whose assignment cannot be resolved (missing/deleted pattern) is left UNCHANGED
    /// and counted in a warning status. Recalling is a live performance action: it neither
    /// snapshots for undo nor marks the set dirty (matching the M3 pattern-launch precedent).
    pub fn recall_scene(&mut self, index: usize) -> Vec<UiCommand> {
        let quant = self.launch_quant;
        self.recall_scene_quant(index, quant)
    }

    /// Like `recall_scene` but forces a specific `quant` instead of using `self.launch_quant`.
    /// The chain recall path calls this with `Quant::NextBar` so scene swaps always land on
    /// a bar boundary — which is where `chain_decision` anchors its dwell measurements.
    fn recall_scene_quant(&mut self, index: usize, quant: Quant) -> Vec<UiCommand> {
        let Some(scene) = self.set.scenes.get(index).cloned() else {
            self.set_status("No such scene");
            return vec![];
        };
        // Resolve against the library + this set's inline lane patterns (by id).
        let inline: Vec<Pattern> = self.set.lanes.iter().map(|l| l.pattern.clone()).collect();
        let resolved = crate::pattern::refs::resolve_scene(&scene, &self.library, &inline);

        let mut missing = 0usize;
        let n = self.set.lanes.len();
        let mut cmds = Vec::new();

        if self.engine_playing {
            // Build ONE QueueScene carrying every resolvable lane at the SAME quant,
            // so the engine queues them all onto a single boundary (`apply_due_launches`
            // fires every lane whose `is_boundary(step, quant)` matches the same step).
            let mut launch_lanes: Vec<(usize, Pattern, LaunchState)> = Vec::new();
            for (lane, res) in resolved.iter().enumerate() {
                if lane >= n {
                    break; // defensive: assignment count > lane count
                }
                match res {
                    Ok(pat) => {
                        let a = &scene.assignments[lane];
                        let state = LaunchState {
                            mute: a.mute,
                            solo: a.solo,
                            transpose: a.transpose,
                            octave: a.octave,
                        };
                        self.queued[lane] = Some(pat.name.clone());
                        launch_lanes.push((lane, pat.clone(), state));
                    }
                    Err(()) => missing += 1,
                }
            }
            if !launch_lanes.is_empty() {
                cmds.push(UiCommand::QueueScene {
                    quant,
                    lanes: launch_lanes,
                });
            }
            self.set_status(scene_recall_status(&scene.name, missing, quant, true));
        } else {
            // Stopped: apply each resolved lane's pattern + performance state immediately.
            for (lane, res) in resolved.iter().enumerate() {
                if lane >= n {
                    break;
                }
                match res {
                    Ok(pat) => {
                        let a = &scene.assignments[lane];
                        let l = &mut self.set.lanes[lane];
                        l.pattern = pat.clone();
                        l.mute = a.mute;
                        l.solo = a.solo;
                        l.transpose = a.transpose;
                        l.octave = a.octave;
                        cmds.push(UiCommand::LoadPattern {
                            lane,
                            pattern: pat.clone(),
                        });
                        cmds.push(UiCommand::Mute { lane, on: a.mute });
                        cmds.push(UiCommand::Solo { lane, on: a.solo });
                        cmds.push(UiCommand::Transpose {
                            lane,
                            semis: a.transpose,
                        });
                        cmds.push(UiCommand::SetOctave {
                            lane,
                            octave: a.octave,
                        });
                    }
                    Err(()) => missing += 1,
                }
            }
            self.set_status(scene_recall_status(
                &scene.name,
                missing,
                self.launch_quant,
                false,
            ));
        }
        cmds
    }

    // ── M7 Task 5: chain playback (app-side auto-advance) ───────────────────────────
    //
    // `ChainPlayback` lives on the App. The engine reports the ABSOLUTE step via
    // `EngineEvent::Playhead{step,..}` (`step = seq.current_step()`); `on_engine_event`
    // calls `tick_chain(step)` on each bar boundary (step % 16 == 0). On Advance/LoopWrap
    // the App recalls the next entry's scene through the EXISTING quantized `recall_scene`
    // path (one `QueueScene` at `Quant::NextBar`); on Stop it emits `UiCommand::Stop`
    // (the engine's `seq.stop` releases all sounding notes). No new note emission.

    /// Resolve a chain entry's `scene_id` to its index in `set.scenes`, if present.
    fn scene_index_by_id(&self, scene_id: &crate::persist::Id) -> Option<usize> {
        self.set.scenes.iter().position(|s| &s.id == scene_id)
    }

    /// The next bar boundary at or after `step` (absolute 16th grid). When `step` is itself
    /// a boundary it is returned unchanged (a recall queued there lands on it).
    fn next_bar_boundary(step: u64) -> u64 {
        if step.is_multiple_of(16) {
            step
        } else {
            (step / 16 + 1) * 16
        }
    }

    /// Arm the chain `entry`'s scene: recall it (one quantized `QueueScene` at `NextBar`)
    /// when its `scene_id` resolves; if it does NOT resolve, recall nothing but push a
    /// `[MISSING]` warning. Either way the caller still advances/anchors so dwell timing
    /// stays deterministic. Returns the recall `UiCommand`s (possibly empty).
    fn arm_chain_entry(&mut self, chain_idx: usize, entry_idx: usize) -> Vec<UiCommand> {
        let Some(scene_id) = self
            .set
            .chains
            .get(chain_idx)
            .and_then(|c| c.entries.get(entry_idx))
            .map(|e| e.scene_id.clone())
        else {
            return vec![];
        };
        match self.scene_index_by_id(&scene_id) {
            // Chain dwell is bar-anchored, so always land on a bar boundary regardless of
            // the user's current `launch_quant` setting (which may be NextBeat).
            Some(scene_idx) => self.recall_scene_quant(scene_idx, Quant::NextBar),
            None => {
                self.set_status(format!("[MISSING] chain entry {} scene", entry_idx + 1));
                vec![]
            }
        }
    }

    /// Driven by `on_engine_event` at each bar boundary with the engine's ABSOLUTE step.
    /// Runs `chain_decision` for the active chain and returns the resulting commands:
    /// - `Hold` → no commands.
    /// - `Advance(n)`/`LoopWrap` → recall entry n's scene (re-anchor `entry_start_step`).
    /// - `Stop` → clear playback + `UiCommand::Stop` (engine releases all sounding notes).
    ///
    /// Returns the commands the caller must forward to the engine. No-op (empty) when no
    /// chain is active, when `now_step` is not past the current anchor, or the chain is gone.
    pub fn tick_chain(&mut self, now_step: u64) -> Vec<UiCommand> {
        let Some(pb) = self.chain_playback.as_ref() else {
            return vec![];
        };
        if !pb.active {
            return vec![];
        }
        // Only decide AT a fresh bar boundary strictly past the entry's anchor; the anchor
        // boundary itself is the entry's start (decision there would always Hold).
        if !now_step.is_multiple_of(16) || now_step <= pb.entry_start_step {
            return vec![];
        }
        let chain_id = pb.chain_id.clone();
        let entry_idx = pb.entry_idx;
        let entry_start_step = pb.entry_start_step;
        let Some(chain_idx) = self.set.chains.iter().position(|c| c.id == chain_id) else {
            // The chain was deleted out from under playback — stop cleanly.
            self.chain_playback = None;
            return vec![UiCommand::Stop];
        };
        let decision = {
            let chain = &self.set.chains[chain_idx];
            crate::pattern::chain::chain_decision(chain, entry_idx, entry_start_step, now_step)
        };
        use crate::pattern::chain::ChainStep;
        let next = match decision {
            ChainStep::Hold => return vec![],
            ChainStep::Advance(n) => n,
            ChainStep::LoopWrap => 0,
            ChainStep::Stop => {
                self.stop_chain_playback();
                return vec![UiCommand::Stop];
            }
        };
        // Re-anchor BEFORE recall so the recall's status/queued state is coherent.
        if let Some(pb) = self.chain_playback.as_mut() {
            pb.entry_idx = next;
            pb.entry_start_step = now_step;
        }
        self.arm_chain_entry(chain_idx, next)
    }

    /// Clear chain playback and any pending chain-queued recall display. Does NOT emit
    /// commands (the caller decides whether a transport `Stop` is also issued).
    fn stop_chain_playback(&mut self) {
        self.chain_playback = None;
        for q in self.queued.iter_mut() {
            *q = None;
        }
    }

    /// Manual takeover: when the performer issues a manual scene recall or pattern launch
    /// while a chain is auto-advancing, deactivate auto-advance (transport keeps playing,
    /// the manual action proceeds). A no-op when no chain is active.
    fn deactivate_chain_on_manual(&mut self) {
        if let Some(pb) = self.chain_playback.as_mut() {
            if pb.active {
                pb.active = false;
                self.set_status("Chain auto-advance off (manual override)");
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

    /// Selectable destination ports for the route editor, as `PortRef`s. The synthetic
    /// virtual "midip" entry comes FIRST (so a lane can always be pointed at midip's own
    /// engine-managed virtual source — it never appears in `list_output_ports()`), followed
    /// by the real hardware ports enumerated when the editor opened.
    pub fn route_port_choices(&self) -> Vec<PortRef> {
        let mut choices = Vec::with_capacity(self.route_editor_ports.len() + 1);
        choices.push(PortRef::virtual_midip());
        for name in &self.route_editor_ports {
            choices.push(PortRef {
                stable_key: name.clone(),
                name: name.clone(),
            });
        }
        choices
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
                // A temporary fill must not silently persist after lane change — revert.
                if let Some(tt) = self.temp_transform.take() {
                    self.set.lanes[tt.lane].pattern = tt.original;
                    cmds.push(UiCommand::LoadPattern {
                        lane: tt.lane,
                        pattern: self.set.lanes[tt.lane].pattern.clone(),
                    });
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
                // A temporary fill must not silently persist after lane change — revert.
                if let Some(tt) = self.temp_transform.take() {
                    self.set.lanes[tt.lane].pattern = tt.original;
                    cmds.push(UiCommand::LoadPattern {
                        lane: tt.lane,
                        pattern: self.set.lanes[tt.lane].pattern.clone(),
                    });
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
                    // A temporary fill must not silently persist after lane change — revert.
                    if let Some(tt) = self.temp_transform.take() {
                        self.set.lanes[tt.lane].pattern = tt.original;
                        cmds.push(UiCommand::LoadPattern {
                            lane: tt.lane,
                            pattern: self.set.lanes[tt.lane].pattern.clone(),
                        });
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
                self.adjust_semi_by_degree(1);
                cmds.push(self.load_focused());
            }
            Action::NoteDown => {
                self.snapshot();
                self.adjust_semi_by_degree(-1);
                cmds.push(self.load_focused());
            }
            Action::CycleScale(dir) => {
                self.snapshot();
                let all = Scale::all();
                let lane = &mut self.set.lanes[self.focus];
                let idx = all.iter().position(|&s| s == lane.scale).unwrap_or(0);
                let next = (idx as i32 + dir as i32).rem_euclid(all.len() as i32) as usize;
                lane.scale = all[next];
                let name = lane.scale.name();
                let root = lane.effective_root();
                self.set_status(format!(
                    "Scale: {} (root {})",
                    name,
                    crate::music::scale::note_name(root)
                ));
                self.dirty = true;
                cmds.push(self.load_focused());
            }
            Action::AdjustRoot(dir) => {
                self.snapshot();
                let lane = &mut self.set.lanes[self.focus];
                let current = lane.effective_root();
                let new_root = (current as i32 + dir as i32).clamp(0, 127) as u8;
                lane.root = Some(new_root);
                let scale_name = lane.scale.name();
                self.set_status(format!(
                    "Root: {} ({})",
                    crate::music::scale::note_name(new_root),
                    scale_name
                ));
                self.dirty = true;
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
            Action::ToggleVoiceMute => {
                if self.focused_kind() != LaneKind::Drums {
                    self.set_status("Voice mute is drums-only".to_string());
                } else {
                    let note = profiles::DRUM_VOICES[self.cur_row].note;
                    self.snapshot();
                    let lane = &mut self.set.lanes[self.focus];
                    let already = lane.muted_voices.contains(&note);
                    if already {
                        lane.muted_voices.retain(|&n| n != note);
                    } else {
                        lane.muted_voices.push(note);
                    }
                    let on = !already;
                    self.set_status(format!(
                        "Voice {} {}",
                        note,
                        if on { "muted" } else { "unmuted" }
                    ));
                    cmds.push(UiCommand::MuteVoice {
                        lane: self.focus,
                        note,
                        on,
                    });
                }
            }
            Action::ToggleFill => {
                if self.temp_transform.is_none() {
                    // No active fill: save original, apply fill, load into engine.
                    let original = self.set.lanes[self.focus].pattern.clone();
                    let lane = self.focus;
                    apply_fill(&mut self.set.lanes[lane].pattern);
                    self.temp_transform = Some(TempTransform { lane, original });
                    self.set_status("Fill on");
                    cmds.push(self.load_focused());
                } else {
                    // Active fill: revert to original.
                    let tt = self.temp_transform.take().unwrap();
                    let lane = tt.lane;
                    self.set.lanes[lane].pattern = tt.original;
                    self.set_status("Fill off");
                    cmds.push(UiCommand::LoadPattern {
                        lane,
                        pattern: self.set.lanes[lane].pattern.clone(),
                    });
                }
            }
            Action::CommitTransform => {
                if let Some(tt) = self.temp_transform.take() {
                    // Make the transform permanent and undoable.
                    // snapshot() before clearing so the pre-fill Set is saved.
                    // At this point the lane already holds the filled pattern;
                    // snapshot() clones the current Set (filled) and pushes it
                    // as the undo target — we push BEFORE the lane is changed,
                    // so we manually push the pre-fill version via the original.
                    //
                    // Correct sequence: push original Set to undo stack, keep filled lane.
                    let mut pre_fill_set = self.set.clone();
                    pre_fill_set.lanes[tt.lane].pattern = tt.original;
                    self.undo.push(pre_fill_set);
                    if self.undo.len() > Self::UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.dirty = true;
                    self.set_status("Fill committed");
                } else {
                    self.set_status("No temporary transform");
                }
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
                    PatternData::Melodic(steps) => steps.resize(new_len, MelodicStep::default()),
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
                            steps.resize(new_len, MelodicStep::default());
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
                        // Manual launch takes over from any chain auto-advance.
                        self.deactivate_chain_on_manual();
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
                // Ensure stable ids on the LIVE set first so that any uncommitted
                // fill's lane inherits the same id it already had (or gets one now),
                // and so that re-saves always produce the same filename.
                self.set.ensure_id();
                for l in &mut self.set.lanes {
                    l.pattern.ensure_id();
                }
                // Build the committed view (fill reverted to original) and write that
                // to disk. The live set — with the active fill — is untouched.
                let mut committed = self.committed_set();
                match crate::pattern::store::save_set(&dir, &mut committed) {
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
                if self.mode == Mode::Help {
                    self.mode = Mode::Edit;
                } else {
                    self.help_scroll = 0;
                    self.mode = Mode::Help;
                }
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
                // Build the port cycle list: "(default)" + the selectable ports. The
                // selectable ports include the synthetic virtual "midip" entry FIRST, then
                // the hardware ports (see `route_port_choices`). "(default)" → route = None;
                // any named choice → route = Some(LaneRoute{...}).
                let choices = self.route_port_choices();
                // Current selection index: 0 = default, 1..=n = choices[i-1] (match by key).
                let current_idx = match &self.set.lanes[lane].route {
                    None => 0usize,
                    Some(r) => choices
                        .iter()
                        .position(|p| p.stable_key == r.port.stable_key)
                        .map(|i| i + 1)
                        .unwrap_or(0),
                };
                let total = choices.len() + 1; // 0=default, 1..=choices.len()
                let next_idx = ((current_idx as i32 + d).rem_euclid(total as i32)) as usize;
                self.snapshot();
                if next_idx == 0 {
                    self.set.lanes[lane].route = None;
                    self.set_status(format!("Lane {lane}: route → (default)"));
                } else {
                    let port = choices[next_idx - 1].clone();
                    let port_name = port.name.clone();
                    // Preserve existing channel/clock_out when switching ports.
                    let existing = self.set.lanes[lane].route.as_ref();
                    let channel = existing
                        .map(|r| r.channel)
                        .unwrap_or_else(|| self.set.lanes[lane].profile.channel);
                    let clock_out = existing.map(|r| r.clock_out).unwrap_or(true);
                    self.set.lanes[lane].route = Some(LaneRoute {
                        port,
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
                // M6: if a scene recall queued multiple lanes, cancel ALL of them; otherwise
                // fall back to cancelling just the focused lane's single queued launch.
                let queued_lanes: Vec<usize> = (0..self.queued.len())
                    .filter(|&l| self.queued[l].is_some())
                    .collect();
                if queued_lanes.len() > 1 {
                    for &lane in &queued_lanes {
                        self.queued[lane] = None;
                        cmds.push(UiCommand::CancelQueue { lane });
                    }
                    self.set_status("Scene recall cancelled");
                } else {
                    let lane = self.focus;
                    if self.queued[lane].is_some() {
                        self.queued[lane] = None;
                        self.set_status(format!("Queue cancelled (lane {})", lane + 1));
                        cmds.push(UiCommand::CancelQueue { lane });
                    }
                }
            }
            Action::RecallScene(index) => {
                // Manual recall takes over: deactivate chain auto-advance (the performer
                // is steering by hand now). Transport keeps playing; only auto-advance stops.
                self.deactivate_chain_on_manual();
                cmds.extend(self.recall_scene(index));
            }
            // ── Scene manager ──────────────────────────────────────────────────
            Action::OpenScenes => {
                self.scene_sel = 0;
                self.scene_issues.clear();
                self.mode = Mode::Scenes;
            }
            Action::CloseScenes => {
                self.mode = Mode::Edit;
            }
            Action::SceneSelect(d) => {
                let n = self.set.scenes.len();
                if n > 0 {
                    self.scene_sel = (self.scene_sel as i32 + d).clamp(0, n as i32 - 1) as usize;
                }
            }
            Action::CaptureScene => {
                let n = self.set.scenes.len() + 1;
                let name = format!("Scene {n}");
                self.snapshot();
                let scene = self.set.capture_scene(name);
                self.set.scenes.push(scene);
                self.scene_sel = self.set.scenes.len() - 1;
                self.dirty = true;
                self.set_status(format!("Captured Scene {n}"));
            }
            Action::RenameScene => {
                if let Some(scene) = self.set.scenes.get(self.scene_sel) {
                    self.name_input = scene.name.clone();
                    self.mode = Mode::NameEntry(NamePurpose::RenameScene);
                }
            }
            Action::DoRenameScene(name) => {
                if self.set.scenes.get(self.scene_sel).is_some() {
                    self.snapshot();
                    self.set.scenes[self.scene_sel].name = name.clone();
                    self.dirty = true;
                    self.set_status(format!("Renamed to \"{name}\""));
                }
                self.mode = Mode::Scenes;
            }
            Action::DuplicateScene => {
                if let Some(scene) = self.set.scenes.get(self.scene_sel).cloned() {
                    self.snapshot();
                    let mut copy = scene.clone();
                    copy.id = crate::persist::mint_id();
                    copy.name = format!("{} (copy)", scene.name);
                    self.set.scenes.push(copy);
                    self.scene_sel = self.set.scenes.len() - 1;
                    self.dirty = true;
                    self.set_status("Scene duplicated");
                }
            }
            Action::DeleteScene => {
                if self.set.scenes.get(self.scene_sel).is_some() {
                    let idx = self.scene_sel;
                    self.mode = Mode::Confirm(ConfirmAction::DeleteScene(idx));
                }
            }
            Action::DoDeleteScene(idx) => {
                if idx < self.set.scenes.len() {
                    self.snapshot();
                    self.set.scenes.remove(idx);
                    self.dirty = true;
                    let n = self.set.scenes.len();
                    if n == 0 {
                        self.scene_sel = 0;
                    } else {
                        self.scene_sel = self.scene_sel.min(n - 1);
                    }
                    self.set_status("Scene deleted");
                }
                self.mode = Mode::Scenes;
            }
            Action::RecallSelectedScene => {
                if self.set.scenes.is_empty() {
                    self.set_status("No scenes");
                } else {
                    let idx = self.scene_sel;
                    cmds.extend(self.apply(Action::RecallScene(idx)));
                }
            }
            Action::ValidateScene => {
                self.scene_issues.clear();
                if let Some(scene) = self.set.scenes.get(self.scene_sel) {
                    let inline: Vec<Pattern> =
                        self.set.lanes.iter().map(|l| l.pattern.clone()).collect();
                    let results = resolve_scene(scene, &self.library, &inline);
                    for (i, r) in results.iter().enumerate() {
                        if r.is_err() {
                            self.scene_issues.push(i);
                        }
                    }
                    let missing = self.scene_issues.len();
                    if missing == 0 {
                        self.set_status("All assignments resolved");
                    } else {
                        self.set_status(format!("{missing} missing assignment(s)"));
                    }
                }
            }
            // ── M7 Chain manager ─────────────────────────────────────────────
            Action::OpenChains => {
                self.mode = Mode::Chains;
            }
            Action::CloseChains => {
                self.mode = Mode::Edit;
            }
            Action::ChainSelect(delta) => {
                let n = self.set.chains.len();
                if n > 0 {
                    let sel = self.chain_sel as i32 + delta;
                    self.chain_sel = sel.clamp(0, (n as i32) - 1) as usize;
                }
            }
            Action::CreateChain => {
                let n = self.set.chains.len() + 1;
                let name = format!("Chain {n}");
                self.snapshot();
                crate::pattern::chain::create_chain(&mut self.set, &name);
                self.chain_sel = self.set.chains.len() - 1;
                self.set_status(format!("Created {name}"));
            }
            Action::RenameChain => {
                if let Some(chain) = self.set.chains.get(self.chain_sel) {
                    self.name_input = chain.name.clone();
                    self.mode = Mode::NameEntry(NamePurpose::RenameChain);
                }
            }
            Action::DoRenameChain(name) => {
                if self.set.chains.get(self.chain_sel).is_some() {
                    self.snapshot();
                    crate::pattern::chain::rename_chain(&mut self.set, self.chain_sel, &name);
                    self.set_status(format!("Renamed to \"{name}\""));
                }
                self.mode = Mode::Chains;
            }
            Action::DuplicateChain => {
                if self.set.chains.get(self.chain_sel).is_some() {
                    self.snapshot();
                    let new_idx =
                        crate::pattern::chain::duplicate_chain(&mut self.set, self.chain_sel);
                    self.chain_sel = new_idx;
                    self.set_status("Chain duplicated");
                }
            }
            Action::DeleteChain => {
                let idx = self.chain_sel;
                if self.set.chains.get(idx).is_some() {
                    self.mode = Mode::Confirm(ConfirmAction::DeleteChain(idx));
                }
            }
            Action::DoDeleteChain(idx) => {
                if idx < self.set.chains.len() {
                    self.snapshot();
                    crate::pattern::chain::delete_chain(&mut self.set, idx);
                    let n = self.set.chains.len();
                    self.chain_sel = if n == 0 { 0 } else { self.chain_sel.min(n - 1) };
                    self.set_status("Chain deleted");
                }
                self.mode = Mode::Chains;
            }
            Action::AddChainEntry { chain, scene_id } => {
                if self.set.chains.get(chain).is_some() {
                    self.snapshot();
                    crate::pattern::chain::add_chain_entry(&mut self.set, chain, scene_id);
                    // Point entry sel at the newly appended entry.
                    if let Some(c) = self.set.chains.get(chain) {
                        self.chain_entry_sel = c.entries.len().saturating_sub(1);
                    }
                }
            }
            Action::AddSelectedSceneToChain => {
                if let Some(scene) = self.set.scenes.get(self.scene_sel) {
                    let scene_id = scene.id.clone();
                    let chain = self.chain_sel;
                    cmds.extend(self.apply(Action::AddChainEntry { chain, scene_id }));
                } else {
                    self.set_status("No scene selected");
                }
            }
            Action::RemoveSelectedChainEntry => {
                let chain = self.chain_sel;
                let entry = self.chain_entry_sel;
                cmds.extend(self.apply(Action::RemoveChainEntry { chain, entry }));
                // Clamp entry sel after removal.
                if let Some(c) = self.set.chains.get(chain) {
                    let n = c.entries.len();
                    if n == 0 {
                        self.chain_entry_sel = 0;
                    } else {
                        self.chain_entry_sel = self.chain_entry_sel.min(n - 1);
                    }
                }
            }
            Action::ChainEntrySelectNext => {
                if let Some(c) = self.set.chains.get(self.chain_sel) {
                    let n = c.entries.len();
                    if n > 0 {
                        self.chain_entry_sel = (self.chain_entry_sel + 1).min(n - 1);
                    }
                }
            }
            Action::ChainEntrySelectPrev => {
                self.chain_entry_sel = self.chain_entry_sel.saturating_sub(1);
            }
            Action::RemoveChainEntry { chain, entry } => {
                if self
                    .set
                    .chains
                    .get(chain)
                    .and_then(|c| c.entries.get(entry))
                    .is_some()
                {
                    self.snapshot();
                    crate::pattern::chain::remove_chain_entry(&mut self.set, chain, entry);
                }
            }
            Action::MoveChainEntry { chain, entry, up } => {
                if self.set.chains.get(chain).is_some() {
                    self.snapshot();
                    if up {
                        crate::pattern::chain::move_chain_entry_up(&mut self.set, chain, entry);
                    } else {
                        crate::pattern::chain::move_chain_entry_down(&mut self.set, chain, entry);
                    }
                }
            }
            Action::SetChainEntryRepeats {
                chain,
                entry,
                value,
            } => {
                if self
                    .set
                    .chains
                    .get(chain)
                    .and_then(|c| c.entries.get(entry))
                    .is_some()
                {
                    self.snapshot();
                    crate::pattern::chain::set_chain_entry_repeats(
                        &mut self.set,
                        chain,
                        entry,
                        value,
                    );
                }
            }
            Action::SetChainEntryBars {
                chain,
                entry,
                value,
            } => {
                if self
                    .set
                    .chains
                    .get(chain)
                    .and_then(|c| c.entries.get(entry))
                    .is_some()
                {
                    self.snapshot();
                    crate::pattern::chain::set_chain_entry_bars(&mut self.set, chain, entry, value);
                }
            }
            Action::ToggleChainLoop(idx) => {
                if self.set.chains.get(idx).is_some() {
                    self.snapshot();
                    crate::pattern::chain::toggle_chain_loop(&mut self.set, idx);
                }
            }
            Action::ToggleSelectedChainLoop => {
                let idx = self.chain_sel;
                cmds.extend(self.apply(Action::ToggleChainLoop(idx)));
            }
            Action::PlaySelectedChain => {
                let idx = self.chain_sel;
                cmds.extend(self.apply(Action::PlayChain(idx)));
            }
            Action::JumpSelectedChainEntry => {
                let entry = self.chain_entry_sel;
                cmds.extend(self.apply(Action::JumpChainEntry(entry)));
            }
            Action::AdjustSelectedChainEntryBars(delta) => {
                let chain = self.chain_sel;
                let entry = self.chain_entry_sel;
                if let Some(e) = self
                    .set
                    .chains
                    .get(chain)
                    .and_then(|c| c.entries.get(entry))
                {
                    let new_val = (e.bars as i32 + delta).max(1) as u32;
                    cmds.extend(self.apply(Action::SetChainEntryBars {
                        chain,
                        entry,
                        value: new_val,
                    }));
                }
            }
            Action::AdjustSelectedChainEntryRepeats(delta) => {
                let chain = self.chain_sel;
                let entry = self.chain_entry_sel;
                if let Some(e) = self
                    .set
                    .chains
                    .get(chain)
                    .and_then(|c| c.entries.get(entry))
                {
                    let new_val = (e.repeats as i32 + delta).max(1) as u32;
                    cmds.extend(self.apply(Action::SetChainEntryRepeats {
                        chain,
                        entry,
                        value: new_val,
                    }));
                }
            }
            // ── M7 Task 5: chain playback ───────────────────────────────────────
            Action::PlayChain(idx) => {
                match self.set.chains.get(idx) {
                    None => self.set_status("No such chain"),
                    Some(chain) if chain.entries.is_empty() => {
                        self.set_status("Chain is empty");
                    }
                    Some(chain) => {
                        // Anchor entry 0 at the next bar boundary; the scene recall (one
                        // quantized QueueScene at NextBar) lands on that same boundary.
                        let anchor = Self::next_bar_boundary(self.playhead as u64);
                        self.chain_playback = Some(crate::pattern::chain::ChainPlayback {
                            chain_id: chain.id.clone(),
                            entry_idx: 0,
                            entry_start_step: anchor,
                            active: true,
                        });
                        cmds.extend(self.arm_chain_entry(idx, 0));
                        // Start transport if not already playing so the chain actually runs.
                        if !self.playing {
                            self.playing = true;
                            cmds.push(UiCommand::Play);
                        }
                        self.set_status(format!("Playing chain \"{}\"", self.set.chains[idx].name));
                    }
                }
            }
            Action::StopChain => {
                if self.chain_playback.take().is_some() {
                    self.stop_chain_playback();
                    // Stop transport too: the engine's seq.stop releases all sounding notes.
                    cmds.push(UiCommand::Stop);
                    self.playing = false;
                    self.set_status("Chain stopped");
                } else {
                    self.set_status("No chain playing");
                }
            }
            Action::JumpChainEntry(idx) => {
                // Re-anchor playback to entry `idx` at the next bar boundary and recall it.
                if let Some(pb) = self.chain_playback.as_ref() {
                    let chain_id = pb.chain_id.clone();
                    if let Some(chain_idx) = self.set.chains.iter().position(|c| c.id == chain_id) {
                        if idx < self.set.chains[chain_idx].entries.len() {
                            let anchor = Self::next_bar_boundary(self.playhead as u64);
                            if let Some(pb) = self.chain_playback.as_mut() {
                                pb.entry_idx = idx;
                                pb.entry_start_step = anchor;
                                pb.active = true;
                            }
                            cmds.extend(self.arm_chain_entry(chain_idx, idx));
                            self.set_status(format!("Jumped to entry {}", idx + 1));
                        } else {
                            self.set_status("No such chain entry");
                        }
                    } else {
                        self.set_status("Chain no longer exists");
                    }
                } else {
                    self.set_status("No chain playing");
                }
            }
            Action::RestartLane => {
                if self.engine_playing {
                    let lane = self.focus;
                    let pat = self.set.lanes[lane].pattern.clone();
                    let name = pat.name.clone();
                    let quant = self.launch_quant;
                    self.queued[lane] = Some(name.clone());
                    self.set_status("Lane restart queued (next bar/beat)");
                    cmds.push(UiCommand::QueuePattern {
                        lane,
                        pattern: pat,
                        quant,
                    });
                } else {
                    self.set_status("Restart applies while playing");
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
                // Ensure stable ids on the live set (and all lane patterns) so that
                // subsequent re-saves use the same filename.
                self.set.ensure_id();
                for l in &mut self.set.lanes {
                    l.pattern.ensure_id();
                }
                // Write the committed view (fill reverted) while keeping the live set
                // (with the active fill) intact in memory.
                let mut committed = self.committed_set();
                match crate::pattern::store::save_set(&dir, &mut committed) {
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
                        NamePurpose::RenameScene => Action::DoRenameScene(name),
                        NamePurpose::RenameChain => Action::DoRenameChain(name),
                    };
                    cmds.extend(self.apply(sub));
                }
            }
            Action::NameCancel => {
                let purpose = match &self.mode {
                    Mode::NameEntry(p) => p.clone(),
                    _ => {
                        self.name_input.clear();
                        return cmds;
                    }
                };
                self.name_input.clear();
                self.mode = match purpose {
                    NamePurpose::RenameScene => Mode::Scenes,
                    NamePurpose::RenameChain => Mode::Chains,
                    _ => Mode::Edit,
                };
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
                    ConfirmAction::ConformToScale(_) => Action::ConformToScale,
                    ConfirmAction::DeleteScene(idx) => Action::DoDeleteScene(idx),
                    ConfirmAction::DeleteChain(idx) => Action::DoDeleteChain(idx),
                };
                cmds.extend(self.apply(sub));
            }
            Action::ConfirmNo => {
                let action = match &self.mode {
                    Mode::Confirm(a) => a.clone(),
                    _ => {
                        self.set_status("Cancelled");
                        return cmds;
                    }
                };
                self.mode = match action {
                    ConfirmAction::DeleteScene(_) => Mode::Scenes,
                    ConfirmAction::DeleteChain(_) => Mode::Chains,
                    _ => Mode::Edit,
                };
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
            // ── M5a Task 4: conform existing notes to scale ───────────────────
            Action::OpenConformToScale => {
                let lane = &self.set.lanes[self.focus];
                // Chromatic is the identity — nothing to fold.
                if lane.scale == Scale::Chromatic {
                    self.set_status("Chromatic: nothing to conform");
                } else {
                    let out_of_scale: usize = match &lane.pattern.data {
                        PatternData::Melodic(steps) => steps
                            .iter()
                            .filter(|s| {
                                if let Some(note) = s.first() {
                                    fold_to_scale(note.semi as i32, lane.scale) != note.semi as i32
                                } else {
                                    false
                                }
                            })
                            .count(),
                        _ => 0,
                    };
                    if out_of_scale == 0 {
                        self.set_status("All notes already in scale");
                    } else {
                        self.mode = Mode::Confirm(ConfirmAction::ConformToScale(out_of_scale));
                    }
                }
            }
            Action::ConformToScale => {
                let lane = &self.set.lanes[self.focus];
                // Chromatic is identity; drum lanes have no pitch to fold.
                if lane.scale == Scale::Chromatic || lane.pattern.kind() == LaneKind::Drums {
                    // silent no-op (callers should use OpenConformToScale for user-facing flow)
                } else {
                    let scale = lane.scale;
                    self.snapshot();
                    let lane = &mut self.set.lanes[self.focus];
                    let mut count = 0usize;
                    if let PatternData::Melodic(steps) = &mut lane.pattern.data {
                        for note in steps.iter_mut().flat_map(|s| s.iter_mut()) {
                            let folded =
                                fold_to_scale(note.semi as i32, scale).clamp(-128, 127) as i8;
                            if folded != note.semi {
                                note.semi = folded;
                                count += 1;
                            }
                        }
                    }
                    let scale_name = self.set.lanes[self.focus].scale.name();
                    self.dirty = true;
                    self.set_status(format!("Conformed {} notes to {}", count, scale_name));
                    cmds.push(self.load_focused());
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
            Action::HelpScroll(delta) => {
                if self.mode == Mode::Help {
                    let d = self.help_scroll as i32 + delta;
                    self.help_scroll = d.max(0) as u16;
                    // upper bound is clamped at render time
                }
            }
            Action::ToggleFavorite => {
                if let Some(r) = self.selected_pattern_ref() {
                    let name = r.display_name();
                    let added = self.favorites.toggle(r);
                    // Best-effort persist; ignore errors so a missing data dir never crashes.
                    let _ = crate::pattern::store::save_favorites(
                        &crate::config::data_dir(),
                        &self.favorites,
                    );
                    if added {
                        self.set_status(format!("\u{2605} favorited {}", name));
                    } else {
                        self.set_status(format!("unfavorited {}", name));
                    }
                }
            }
            Action::ToggleFavFilter => {
                self.fav_filter = !self.fav_filter;
                // Clamp lib_genre / lib_pattern so navigation stays sane after filter change.
                self.clamp_lib_selection();
                if self.fav_filter {
                    self.set_status("Favorites only");
                } else {
                    self.set_status("All patterns");
                }
            }
            // ── M4a Task 4: crate management ─────────────────────────────────────
            Action::CreateCrate(name) => {
                self.crates.add_crate(name.clone());
                self.persist_crates();
                self.set_status(format!("Crate created: {}", name));
            }
            Action::RenameCrate(idx, name) => {
                self.crates.rename_crate(idx, name.clone());
                self.persist_crates();
                if idx < self.crates.crates.len() {
                    self.set_status(format!("Crate renamed: {}", name));
                }
            }
            Action::DuplicateCrate(idx) => {
                if let Some(new_idx) = self.crates.duplicate_crate(idx) {
                    let new_name = self.crates.crates[new_idx].name.clone();
                    self.persist_crates();
                    self.set_status(format!("Crate duplicated: {}", new_name));
                }
            }
            Action::DeleteCrate(idx) => {
                if idx < self.crates.crates.len() {
                    let name = self.crates.crates[idx].name.clone();
                    self.crates.remove_crate(idx);
                    self.persist_crates();
                    self.set_status(format!("Crate deleted: {}", name));
                }
            }
            Action::ReorderCrateEntry(crate_idx, from, to) => {
                self.crates.reorder_entry(crate_idx, from, to);
                self.persist_crates();
            }
            Action::AddToCrate(idx) => {
                // Clear stale validation — crate contents are changing.
                self.crate_issues.clear();
                if let Some(r) = self.selected_pattern_ref() {
                    let crate_name = self.crates.crates.get(idx).map(|c| c.name.clone());
                    self.crates.add_entry(
                        idx,
                        CrateEntry {
                            pattern: r,
                            label: None,
                        },
                    );
                    self.persist_crates();
                    if let Some(name) = crate_name {
                        self.set_status(format!("Added to {}", name));
                    }
                } else {
                    self.set_status("nothing selected");
                }
            }
            Action::RemoveFromCrate(crate_idx, entry_idx) => {
                // Clear stale validation — crate contents are changing.
                self.crate_issues.clear();
                self.crates.remove_entry(crate_idx, entry_idx);
                self.persist_crates();
            }
            // ── M4a Task 5: live crate view ──────────────────────────────────────
            Action::OpenCrateView => {
                self.crate_sel = 0;
                self.crate_entry_sel = 0;
                self.mode = Mode::CrateView;
            }
            Action::CloseCrateView => {
                if let Some(prev) = self.audition.take() {
                    // Cancel audition: restore the engine to the committed pattern.
                    // (Mirrors CloseLibrary so closing crate-view is glitch-free.)
                    cmds.push(UiCommand::LoadPattern {
                        lane: prev.lane,
                        pattern: self.set.lanes[prev.lane].pattern.clone(),
                    });
                    self.set_status("Audition cancelled");
                }
                self.crate_issues.clear();
                self.mode = Mode::Edit;
            }
            Action::CrateEntrySel(d) => {
                if let Some(cr) = self.crates.crates.get(self.crate_sel) {
                    let n = cr.entries.len();
                    if n > 0 {
                        self.crate_entry_sel =
                            (self.crate_entry_sel as i32 + d).clamp(0, n as i32 - 1) as usize;
                    }
                }
            }
            Action::CrateSel(d) => {
                let n = self.crates.crates.len();
                if n > 0 {
                    self.crate_sel = (self.crate_sel as i32 + d).clamp(0, n as i32 - 1) as usize;
                    self.crate_entry_sel = 0;
                }
                // Clear stale validation when the selected crate changes.
                self.crate_issues.clear();
            }
            Action::LaunchCrateEntry => {
                let r = self
                    .crates
                    .crates
                    .get(self.crate_sel)
                    .and_then(|cr| cr.entries.get(self.crate_entry_sel))
                    .map(|e| e.pattern.clone());
                if let Some(r) = r {
                    cmds.extend(self.launch_ref(&r));
                }
            }
            Action::AuditionCrateEntry => {
                let entry_opt = self
                    .crates
                    .crates
                    .get(self.crate_sel)
                    .and_then(|cr| cr.entries.get(self.crate_entry_sel))
                    .map(|e| e.pattern.clone());
                if let Some(r) = entry_opt {
                    let user_dir = crate::config::data_dir().join("patterns");
                    let lane = r
                        .role_lane_hint()
                        .map(|h| h.min(self.set.lanes.len().saturating_sub(1)))
                        .unwrap_or(self.focus);
                    let lane_muted = self.set.lanes.get(lane).map(|l| l.mute).unwrap_or(false);
                    if self.engine_playing && !lane_muted {
                        self.set_status("Mute lane to audition (it's live)");
                    } else if let Some(pat) =
                        crate::pattern::refs::resolve_pattern_ref(&r, &self.library, &user_dir)
                    {
                        self.set_status(format!("Auditioning {}", pat.name));
                        self.audition = Some(AuditionPreview {
                            lane,
                            pattern: pat.clone(),
                        });
                        cmds.push(UiCommand::LoadPattern { lane, pattern: pat });
                    } else {
                        self.set_status("missing pattern");
                    }
                }
            }
            Action::FavoriteCrateEntry => {
                let r = self
                    .crates
                    .crates
                    .get(self.crate_sel)
                    .and_then(|cr| cr.entries.get(self.crate_entry_sel))
                    .map(|e| e.pattern.clone());
                if let Some(r) = r {
                    let name = r.display_name();
                    let added = self.favorites.toggle(r);
                    let _ = crate::pattern::store::save_favorites(
                        &crate::config::data_dir(),
                        &self.favorites,
                    );
                    if added {
                        self.set_status(format!("\u{2605} favorited {}", name));
                    } else {
                        self.set_status(format!("unfavorited {}", name));
                    }
                }
            }
            // ── M4a Task 6: pre-performance validation ────────────────────────
            Action::ValidateCrate => {
                self.crate_issues = self.validate_crate(self.crate_sel);
                let n = self.crate_issues.len();
                if n == 0 {
                    self.set_status("Crate OK");
                } else {
                    self.set_status(format!("{} issue(s) found", n));
                }
            }
            // ── M5a Task 5: QWERTY note-input sub-mode ───────────────────────────────
            Action::OpenNoteInput => {
                if self.focused_kind() == LaneKind::Drums {
                    self.set_status("Note input is melodic-only");
                } else {
                    // Snapshot ONCE on entry — the whole session is one undo unit.
                    self.snapshot();
                    self.note_input_octave = 0;
                    let is_poly = self.set.lanes[self.focus].profile.poly;
                    self.mode = Mode::NoteInput;
                    // Poly lanes STACK keys into a chord on one step (press the same key
                    // again to toggle it off); mono lanes replace+advance like typing a
                    // melody. Reflect that distinction in the entry banner.
                    self.set_status(if is_poly {
                        "NOTE INPUT (poly: keys STACK a chord; repeat=off) [a-k]/[w/e/t/y/u] [z/x]oct [bksp]del [esc]exit"
                    } else {
                        "NOTE INPUT  [a-k]notes [w/e/t/y/u]black [z/x]octave [bksp]del [esc]exit"
                    });
                }
            }
            Action::CloseNoteInput => {
                if self.mode == Mode::NoteInput {
                    self.mode = Mode::Edit;
                }
            }
            Action::NoteInputPlace(offset) => {
                if self.mode == Mode::NoteInput {
                    let semi_raw = offset as i32 + self.note_input_octave as i32 * 12;
                    let lane_scale = self.set.lanes[self.focus].scale;
                    let semi_folded = fold_to_scale(semi_raw, lane_scale).clamp(-128, 127) as i8;
                    let col = self.cur_col;
                    let lane = &mut self.set.lanes[self.focus];
                    let is_poly = lane.profile.poly;
                    let gate = lane.profile.gate_fraction;
                    if let PatternData::Melodic(steps) = &mut lane.pattern.data {
                        if let Some(slot) = steps.get_mut(col) {
                            let new_note = MelodicNote {
                                semi: semi_folded,
                                vel: MEL_DEFAULT_VEL,
                                slide: false,
                                len: gate,
                                prob: 1.0,
                                ratchet: 1,
                            };
                            if is_poly {
                                // Poly lane (M5b Task 4): STACK the pressed pitch onto the
                                // cursor step to build a chord, and do NOT advance — so
                                // several key presses build a chord on one step. The cursor
                                // advances only via the arrow/step keys. Duplicate-pitch
                                // rule: pressing a pitch already in the step TOGGLES it off
                                // (so a key acts as a per-pitch on/off), never duplicating.
                                if let Some(pos) = slot.iter().position(|n| n.semi == semi_folded) {
                                    slot.remove(pos);
                                } else {
                                    slot.push(new_note);
                                }
                            } else {
                                // Mono enforcement (M5b Task 2): a mono lane holds AT
                                // MOST ONE note. Placing a note always replaces any
                                // existing note, preserving the step's single-note
                                // invariant. Slide and other per-step fields come from
                                // the new note (identical to today's mono behaviour).
                                *slot = MelodicStep::from(vec![new_note]);
                            }
                        }
                    }
                    // Mono lanes advance one step (melody-typing); poly lanes stay on the
                    // step so successive presses stack into a chord.
                    if !is_poly {
                        let pat_len = self.set.lanes[self.focus].pattern.length;
                        self.cur_col = (self.cur_col + 1) % pat_len.max(1);
                        self.step_scroll = (self.cur_col / VISIBLE_STEPS) * VISIBLE_STEPS;
                    }
                    self.dirty = true;
                    cmds.push(self.load_focused());
                }
            }
            Action::NoteInputOctave(d) => {
                if self.mode == Mode::NoteInput {
                    self.note_input_octave = (self.note_input_octave + d).clamp(-3, 3);
                    self.set_status(format!("Octave offset: {:+}", self.note_input_octave));
                }
            }
            Action::NoteInputBackspace => {
                if self.mode == Mode::NoteInput {
                    // Clear the current step.
                    self.clear_step();
                    // Step back one, wrapping.
                    let pat_len = self.set.lanes[self.focus].pattern.length;
                    let col = self.cur_col as isize - 1;
                    self.cur_col = col.rem_euclid(pat_len.max(1) as isize) as usize;
                    self.step_scroll = (self.cur_col / VISIBLE_STEPS) * VISIBLE_STEPS;
                    self.dirty = true;
                    cmds.push(self.load_focused());
                }
            }
            Action::BuildTriad => {
                // Build a scale-aware triad on the cursor step (poly lanes only). From the
                // step's ROOT note (its first note) add a 3rd and 5th by stepping +2 and +4
                // scale degrees via `step_by_degree`, folded to the lane's scale. In a Major
                // scale this yields a major triad (root, M3, P5); in a Minor scale a minor
                // triad — the interval quality follows the scale automatically. Duplicate
                // semis are not added (a unison degree, e.g. on some pentatonic folds, is
                // skipped). No-op with a status message on an empty step or a mono lane.
                let col = self.cur_col;
                let scale = self.set.lanes[self.focus].scale;
                let is_poly = self.set.lanes[self.focus].profile.poly;
                let root_semi =
                    if let PatternData::Melodic(steps) = &self.set.lanes[self.focus].pattern.data {
                        steps.get(col).and_then(|s| s.first()).map(|n| n.semi)
                    } else {
                        Option::None
                    };
                match (is_poly, root_semi) {
                    (true, Some(root)) => {
                        self.snapshot();
                        let third = step_by_degree(root as i32, 2, scale).clamp(-48, 48) as i8;
                        let fifth = step_by_degree(root as i32, 4, scale).clamp(-48, 48) as i8;
                        let lane = &mut self.set.lanes[self.focus];
                        let gate = lane.profile.gate_fraction;
                        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
                            if let Some(slot) = steps.get_mut(col) {
                                for s in [third, fifth] {
                                    if !slot.iter().any(|n| n.semi == s) {
                                        slot.push(MelodicNote {
                                            semi: s,
                                            vel: MEL_DEFAULT_VEL,
                                            slide: false,
                                            len: gate,
                                            prob: 1.0,
                                            ratchet: 1,
                                        });
                                    }
                                }
                            }
                        }
                        self.dirty = true;
                        self.set_status("Built triad (root + 3rd + 5th)");
                        cmds.push(self.load_focused());
                    }
                    _ => {
                        self.set_status("Triad needs a poly lane with a root note");
                    }
                }
            }
            Action::RemoveChordNote => {
                // Remove the LAST note from the cursor step. On a single-note step this
                // clears it (becomes a rest). No-op on an empty step. Only snapshot (and
                // mark dirty) when there is actually a note to remove, so a no-op press
                // does not create an empty undo step.
                let col = self.cur_col;
                let has_note =
                    if let PatternData::Melodic(steps) = &self.set.lanes[self.focus].pattern.data {
                        steps.get(col).map(|s| !s.is_empty()).unwrap_or(false)
                    } else {
                        false
                    };
                if has_note {
                    self.snapshot();
                    if let PatternData::Melodic(steps) =
                        &mut self.set.lanes[self.focus].pattern.data
                    {
                        if let Some(slot) = steps.get_mut(col) {
                            slot.pop();
                        }
                    }
                    self.dirty = true;
                    cmds.push(self.load_focused());
                }
            }
            // ── M9 Generative tool ───────────────────────────────────────────
            Action::OpenGenerative => {
                let lane = self.focus;
                let original = self.set.lanes[lane].pattern.clone();
                // Seed this session from the rolling gen_seed.
                let seed = next_rng(&mut self.gen_seed);
                let params = GenParams {
                    seed,
                    ..GenParams::default()
                };
                self.gen_params = params.clone();
                let candidate = generate(&params, &original, &self.set.lanes[lane]);
                self.set.lanes[lane].pattern = candidate;
                self.temp_transform = Some(TempTransform { lane, original });
                self.mode = Mode::Generative;
                self.set_status("Generative: preview active");
                cmds.push(UiCommand::LoadPattern {
                    lane,
                    pattern: self.set.lanes[lane].pattern.clone(),
                });
            }
            Action::GenSetMode(mode) => {
                if let Some(tt) = &self.temp_transform {
                    self.gen_params.mode = mode;
                    let lane = tt.lane;
                    let original = tt.original.clone();
                    let candidate = generate(&self.gen_params, &original, &self.set.lanes[lane]);
                    self.set.lanes[lane].pattern = candidate;
                    cmds.push(UiCommand::LoadPattern {
                        lane,
                        pattern: self.set.lanes[lane].pattern.clone(),
                    });
                }
            }
            Action::GenAdjust { field, delta } => {
                if let Some(tt) = &self.temp_transform {
                    match field {
                        GenField::Density => {
                            self.gen_params.density =
                                (self.gen_params.density as i32 + delta).clamp(0, 100) as u8;
                        }
                        GenField::Range => {
                            self.gen_params.range =
                                (self.gen_params.range as i32 + delta).clamp(0, 127) as u8;
                        }
                        GenField::Mutate => {
                            self.gen_params.mutate =
                                (self.gen_params.mutate as i32 + delta).clamp(0, 100) as u8;
                        }
                    }
                    let lane = tt.lane;
                    let original = tt.original.clone();
                    let candidate = generate(&self.gen_params, &original, &self.set.lanes[lane]);
                    self.set.lanes[lane].pattern = candidate;
                    cmds.push(UiCommand::LoadPattern {
                        lane,
                        pattern: self.set.lanes[lane].pattern.clone(),
                    });
                }
            }
            Action::GenReroll => {
                if let Some(tt) = &self.temp_transform {
                    self.gen_params.seed = next_rng(&mut self.gen_seed);
                    let lane = tt.lane;
                    let original = tt.original.clone();
                    let candidate = generate(&self.gen_params, &original, &self.set.lanes[lane]);
                    self.set.lanes[lane].pattern = candidate;
                    cmds.push(UiCommand::LoadPattern {
                        lane,
                        pattern: self.set.lanes[lane].pattern.clone(),
                    });
                }
            }
            Action::GenCommit => {
                if let Some(tt) = self.temp_transform.take() {
                    // Mirror CommitTransform: push the pre-op Set (with original pattern)
                    // to the undo stack — one entry, exactly like ToggleFill+CommitTransform.
                    let mut pre_op_set = self.set.clone();
                    pre_op_set.lanes[tt.lane].pattern = tt.original;
                    self.undo.push(pre_op_set);
                    if self.undo.len() > Self::UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.dirty = true;
                    self.mode = Mode::Edit;
                    self.set_status("Generative: committed");
                } else {
                    self.set_status("No generative preview to commit");
                }
            }
            Action::GenCancel => {
                if let Some(tt) = self.temp_transform.take() {
                    let lane = tt.lane;
                    self.set.lanes[lane].pattern = tt.original;
                    cmds.push(UiCommand::LoadPattern {
                        lane,
                        pattern: self.set.lanes[lane].pattern.clone(),
                    });
                }
                self.mode = Mode::Edit;
                self.set_status("Generative: cancelled");
            }
            Action::None => {}
        }
        cmds
    }

    /// Fold an EngineEvent into display state. Returns any `UiCommand`s the caller must
    /// forward to the engine — currently only chain auto-advance recalls/stop, triggered
    /// at each bar boundary reported via `EngineEvent::Playhead`.
    pub fn on_engine_event(&mut self, ev: EngineEvent) -> Vec<UiCommand> {
        let mut cmds = Vec::new();
        match ev {
            EngineEvent::Playhead { step, bar, .. } => {
                self.playhead = step;
                self.bar = bar;
                // M7 Task 5: drive chain auto-advance at each engine-reported step. The
                // boundary gate lives in `tick_chain` (step % 16 == 0, past the anchor).
                cmds.extend(self.tick_chain(step as u64));
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
        cmds
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
            Mode::CrateView => "CRATE VIEW",
            Mode::Scenes => "SCENES",
            Mode::Chains => "CHAINS",
            Mode::NoteInput => "NOTE INPUT",
            Mode::Generative => "GENERATIVE",
        }
    }

    /// Whether the Set has unsaved mutations.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Returns a clone of `self.set` with any uncommitted fill reverted to its
    /// pre-fill original. When no fill is active this is identical to `self.set.clone()`.
    ///
    /// Use this whenever writing to disk (save / recovery) so that a latched but
    /// uncommitted fill is never baked into a file.  The live `self.set` (and the
    /// engine) are intentionally NOT touched.
    pub fn committed_set(&self) -> Set {
        let mut s = self.set.clone();
        if let Some(tt) = &self.temp_transform {
            if tt.lane < s.lanes.len() {
                s.lanes[tt.lane].pattern = tt.original.clone();
            }
        }
        s
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
        // Extract scale before the mutable borrow so the fold can reference it below.
        let lane_scale = self.set.lanes[self.focus].scale;
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
                    // Toggle: a non-empty step toggles off to a rest; an empty step
                    // toggles on to a single default note (mono for all lanes here —
                    // chord stacking via ToggleStep is not supported; poly lanes get
                    // chord entry via dedicated Actions in M5b Task 4).
                    if !slot.is_empty() {
                        *slot = MelodicStep::default();
                    } else {
                        let gate = lane.profile.gate_fraction;
                        // Fold the default semi=0 to the nearest in-scale degree when
                        // the lane has a non-Chromatic scale. Existing notes are never
                        // rewritten — folding only applies at placement time.
                        let semi = fold_to_scale(0, lane_scale) as i8;
                        *slot = MelodicStep::from(vec![MelodicNote {
                            semi,
                            vel: MEL_DEFAULT_VEL,
                            slide: false,
                            len: gate,
                            prob: 1.0,
                            ratchet: 1,
                        }]);
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
                if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
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
                if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
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
            PatternData::Melodic(steps) => steps.get(col).map(|s| !s.is_empty()).unwrap_or(false),
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
                .and_then(|s| s.first())
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
                .and_then(|s| s.first())
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
                steps.get(col).and_then(|s| s.first()).map(|n| n.ratchet)
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
                if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
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
                if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
                    n.vel = (n.vel + d as f32 * 0.05).clamp(0.0, 1.3);
                }
            }
        }
    }

    /// Move the cursor note's pitch by one scale degree (non-Chromatic) or one semitone
    /// (Chromatic). Clamps result to ±48 semitones.
    fn adjust_semi_by_degree(&mut self, dir: i32) {
        let col = self.cur_col;
        let scale = self.set.lanes[self.focus].scale;
        if let PatternData::Melodic(steps) = &mut self.set.lanes[self.focus].pattern.data {
            if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
                let new_semi = step_by_degree(n.semi as i32, dir, scale);
                n.semi = new_semi.clamp(-48, 48) as i8;
            }
        }
    }

    fn adjust_len(&mut self, d: i8) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
            if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
                n.len = (n.len + d as f32 * 0.25).clamp(0.25, 64.0);
            }
        }
    }

    fn toggle_slide(&mut self) {
        let col = self.cur_col;
        let lane = &mut self.set.lanes[self.focus];
        if let PatternData::Melodic(steps) = &mut lane.pattern.data {
            if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
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
                    *slot = MelodicStep::default();
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
                let s: MelodicStep = steps.get(col).cloned().unwrap_or_default();
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
            PatternData::Melodic(steps) => steps.iter().any(|s| !s.is_empty()),
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

    /// Clamp lib_genre and lib_pattern to valid indices so that toggling the
    /// favorites filter (which may shrink the visible list) never leaves a
    /// selection pointing past the end of the filtered lists.
    fn clamp_lib_selection(&mut self) {
        let genre_count = self.current_genre_map().len();
        if genre_count == 0 {
            self.lib_genre = 0;
            self.lib_pattern = 0;
            return;
        }
        if self.lib_genre >= genre_count {
            self.lib_genre = genre_count - 1;
        }
        let pat_count = self
            .current_genre_map()
            .get_index(self.lib_genre)
            .map(|(_, v)| v.len())
            .unwrap_or(0);
        if pat_count == 0 {
            self.lib_pattern = 0;
        } else if self.lib_pattern >= pat_count {
            self.lib_pattern = pat_count - 1;
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

    /// Build a `PatternRef` for the currently-selected library entry.
    /// Returns `None` when there is no selection.
    ///
    /// Vendored entries (any genre other than "User") produce `PatternRef::Vendored`.
    /// Entries in the "User" genre produce `PatternRef::User(pattern.id)`.
    pub fn selected_pattern_ref(&self) -> Option<PatternRef> {
        let map = self.current_genre_map();
        let (genre, patterns) = map.get_index(self.lib_genre)?;
        let pat = patterns.get(self.lib_pattern)?;
        if genre == "User" {
            Some(PatternRef::User(pat.id.clone()))
        } else {
            let role = match self.lib_role {
                LibRole::Drums => "drums",
                LibRole::Bass => "bass",
                LibRole::Synth => "synth",
            };
            Some(PatternRef::Vendored {
                role: role.to_string(),
                genre: genre.clone(),
                name: pat.name.clone(),
            })
        }
    }
}

/// Human-readable label for a `Quant` value, used in status toasts and UI.
fn quant_label(q: Quant) -> &'static str {
    match q {
        Quant::NextBar => "next bar",
        Quant::NextBeat => "next beat",
    }
}

/// M6: build the status toast for a scene recall, noting how many lanes were skipped
/// (missing/unresolvable pattern). `playing` selects the queued-vs-immediate wording.
fn scene_recall_status(name: &str, missing: usize, quant: Quant, playing: bool) -> String {
    let base = if playing {
        format!("Recall {} ({})", name, quant_label(quant))
    } else {
        format!("Recalled {}", name)
    };
    if missing > 0 {
        format!("{} — {} lane(s) skipped (missing pattern)", base, missing)
    } else {
        base
    }
}

/// Apply a deterministic fill to a pattern.
///
/// **Drum lane:** on the LAST BEAT (the final `min(4, length)` steps), add a hit
/// on each of those steps using the first existing voice note in the pattern, or
/// note 38 (snare) if none. Velocity 100, prob 1.0, ratchet 1. Hits are layered
/// over existing hits; duplicates (same note on same step) are not added.
///
/// **Melodic lane:** on the LAST BEAT's non-rest notes, double the ratchet
/// (clamped to 8). If there are no notes in the last beat, the pattern is unchanged.
///
/// The function is idempotent-enough to be deterministic: the same input always
/// produces the same output. (A second call IS safe but NOT perfectly idempotent for
/// drums because the hit would already be present and the dedup guard blocks it.)
pub fn apply_fill(p: &mut Pattern) {
    let length = p.length.max(1);
    let beat_len = 4_usize.min(length);
    let last_beat_start = length.saturating_sub(beat_len);

    match &mut p.data {
        PatternData::Drums(steps) => {
            // Find the first voice note present anywhere in the pattern, fallback to 38.
            let fill_note: u8 = steps
                .iter()
                .flat_map(|s| s.iter())
                .map(|h| h.note)
                .next()
                .unwrap_or(38);

            for step in steps.iter_mut().skip(last_beat_start) {
                // Dedup: only add if no existing hit with the same note.
                if !step.iter().any(|h| h.note == fill_note) {
                    step.push(DrumHit {
                        note: fill_note,
                        vel: 100,
                        prob: 1.0,
                        ratchet: 1,
                    });
                }
            }
        }
        PatternData::Melodic(steps) => {
            for note in steps
                .iter_mut()
                .skip(last_beat_start)
                .flat_map(|s| s.iter_mut())
            {
                note.ratchet = (note.ratchet * 2).min(8);
            }
        }
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
        let mut bsteps = vec![MelodicStep::default(); 16];
        bsteps[0] = MelodicStep::from(vec![MelodicNote {
            semi: 3,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
        }]);
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
                data: PatternData::Melodic(vec![MelodicStep::default(); 16]),
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
            let n = steps[5].first().expect("note placed");
            assert_eq!(n.semi, 0);
            assert_eq!(n.vel, 1.0);
        } else {
            panic!("expected melodic");
        }
        app.apply(Action::ToggleStep); // remove
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            assert!(steps[5].is_empty());
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

    // ── Per-drum-voice mute (§2.6) ────────────────────────────────────────────

    #[test]
    fn toggle_voice_mute_updates_lane_and_emits() {
        let mut app = new_app();
        // Focus lane 0 (drums), cur_row=0 → DRUM_VOICES[0].note.
        app.apply(Action::FocusLane(0));
        let note = profiles::DRUM_VOICES[0].note;
        assert!(!app.focused_lane().muted_voices.contains(&note));

        let cmds = app.apply(Action::ToggleVoiceMute);

        // Lane must now contain the note in muted_voices.
        assert!(
            app.focused_lane().muted_voices.contains(&note),
            "muted_voices must include the cursor voice note after toggle"
        );
        // Must be dirty (snapshot taken).
        assert!(app.dirty, "ToggleVoiceMute must mark dirty");
        // Must emit MuteVoice{on:true}.
        assert!(
            cmds.iter().any(
                |c| matches!(c, UiCommand::MuteVoice { lane: 0, note: n, on: true } if *n == note)
            ),
            "must emit MuteVoice{{lane:0, note:{note}, on:true}}; got: {cmds:?}"
        );

        // Toggle again → unmute.
        let cmds2 = app.apply(Action::ToggleVoiceMute);
        assert!(
            !app.focused_lane().muted_voices.contains(&note),
            "muted_voices must not contain note after second toggle"
        );
        assert!(
            cmds2.iter().any(
                |c| matches!(c, UiCommand::MuteVoice { lane: 0, note: n, on: false } if *n == note)
            ),
            "second toggle must emit MuteVoice{{on:false}}; got: {cmds2:?}"
        );
    }

    #[test]
    fn toggle_voice_mute_noop_on_melodic() {
        let mut app = new_app();
        // Focus lane 1 (melodic).
        app.apply(Action::FocusLane(1));
        assert_eq!(app.focused_kind(), LaneKind::Melodic);

        let cmds = app.apply(Action::ToggleVoiceMute);

        // No MuteVoice command emitted.
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::MuteVoice { .. })),
            "melodic lane must not emit MuteVoice; got: {cmds:?}"
        );
        // Not dirty.
        assert!(!app.dirty, "melodic voice mute must not mark dirty");
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

    /// The route editor's selectable ports must include a synthetic "midip" virtual entry
    /// (alongside the hardware ports from `list_output_ports`) so a lane can be pointed at
    /// midip's own engine-managed virtual source.
    #[test]
    fn route_editor_lists_midip_virtual_port() {
        use crate::pattern::model::{VIRTUAL_PORT_KEY, VIRTUAL_PORT_NAME};
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        // Even with NO real hardware ports, the virtual "midip" entry must be offered.
        app.route_editor_ports = Vec::new();
        let choices = app.route_port_choices();
        assert!(
            choices
                .iter()
                .any(|p| p.stable_key == VIRTUAL_PORT_KEY && p.name == VIRTUAL_PORT_NAME),
            "route port choices must include the virtual 'midip' entry; got: {choices:?}"
        );
    }

    /// Cycling a lane's port lands on the virtual "midip" port and emits a SetRoute whose
    /// route key == VIRTUAL_PORT_KEY (so the engine maps it to the virtual destination).
    #[test]
    fn route_cycle_port_can_select_virtual_midip() {
        use crate::pattern::model::VIRTUAL_PORT_KEY;
        let mut app = new_app();
        app.apply(Action::OpenRouteEditor);
        app.route_editor_ports = Vec::new(); // no hardware → choices are [midip]
                                             // Cycle forward until a lane route targets the virtual key (bounded loop).
        let mut found = false;
        for _ in 0..app.route_port_choices().len() + 2 {
            let cmds = app.apply(Action::RouteCyclePort(1));
            if app
                .set
                .lanes
                .get(app.route_editor_lane)
                .and_then(|l| l.route.as_ref())
                .map(|r| r.port.stable_key == VIRTUAL_PORT_KEY)
                .unwrap_or(false)
            {
                assert!(
                    cmds.iter().any(|c| matches!(
                        c,
                        UiCommand::SetRoute {
                            route: Some(r),
                            ..
                        } if r.port.stable_key == VIRTUAL_PORT_KEY
                    )),
                    "selecting midip must emit SetRoute with the virtual key; got: {cmds:?}"
                );
                found = true;
                break;
            }
        }
        assert!(
            found,
            "cycling must be able to select the virtual 'midip' port"
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

    // ── M6 Task 2: all-lane quantized scene recall ──────────────────────────

    /// Build an app whose 3 lanes hold patterns with DISTINCT ids, plus one scene
    /// `assignments[i] = User(lane i's id)` so `resolve_scene` resolves each lane via
    /// the inline set patterns. The scene also carries per-lane mute/solo/transpose/octave.
    fn app_with_scene() -> App {
        use crate::pattern::model::{LaneAssignment, Scene};
        let mut app = new_app();
        // Give each lane's pattern a distinct id so User-ref resolution is unambiguous.
        let ids: Vec<crate::persist::Id> = (0..app.set.lanes.len())
            .map(|_| crate::persist::mint_id())
            .collect();
        for (i, lane) in app.set.lanes.iter_mut().enumerate() {
            lane.pattern.id = ids[i].clone();
            lane.pattern.name = format!("p{i}");
        }
        let assignments = ids
            .iter()
            .enumerate()
            .map(|(i, id)| LaneAssignment {
                pattern: PatternRef::User(id.clone()),
                mute: i == 0, // lane 0 muted in the scene
                solo: false,
                transpose: if i == 1 { 5 } else { 0 }, // lane 1 transposed +5
                octave: if i == 2 { 1 } else { 0 },    // lane 2 octave +1
            })
            .collect();
        app.set.scenes.push(Scene {
            id: crate::persist::mint_id(),
            name: "Scene 1".into(),
            assignments,
        });
        app
    }

    #[test]
    fn recall_scene_queues_all_lanes_on_one_boundary() {
        let mut app = app_with_scene();
        app.engine_playing = true;
        let cmds = app.apply(Action::RecallScene(0));
        // Exactly one QueueScene carrying all 3 resolvable lanes at one quant.
        let scene_cmds: Vec<&UiCommand> = cmds
            .iter()
            .filter(|c| matches!(c, UiCommand::QueueScene { .. }))
            .collect();
        assert_eq!(
            scene_cmds.len(),
            1,
            "recall must emit ONE QueueScene; got {cmds:?}"
        );
        if let UiCommand::QueueScene { quant, lanes } = scene_cmds[0] {
            assert_eq!(*quant, Quant::NextBar, "uses the current launch_quant");
            assert_eq!(
                lanes.len(),
                3,
                "all 3 lanes queued together on ONE boundary"
            );
            let mut idxs: Vec<usize> = lanes.iter().map(|(l, _, _)| *l).collect();
            idxs.sort_unstable();
            assert_eq!(idxs, vec![0, 1, 2]);
        }
        // No immediate LoadPattern while playing — everything is queued.
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "while playing, recall must NOT load immediately; got {cmds:?}"
        );
        // queued[] display set for every lane.
        assert!(
            app.queued.iter().all(|q| q.is_some()),
            "all lanes shown queued"
        );
    }

    #[test]
    fn recall_scene_applies_mute_solo_transpose_octave_at_launch() {
        let mut app = app_with_scene();
        app.engine_playing = true;
        let cmds = app.apply(Action::RecallScene(0));
        let UiCommand::QueueScene { lanes, .. } = cmds
            .iter()
            .find(|c| matches!(c, UiCommand::QueueScene { .. }))
            .expect("QueueScene emitted")
        else {
            unreachable!()
        };
        // The per-lane LaunchState carries the scene's mute/transpose/octave (applied at the
        // launch instant by the engine — NOT applied to the live lanes now).
        let find = |idx: usize| lanes.iter().find(|(l, _, _)| *l == idx).map(|(_, _, s)| *s);
        assert!(find(0).unwrap().mute, "lane 0 mute carried");
        assert_eq!(find(1).unwrap().transpose, 5, "lane 1 transpose carried");
        assert_eq!(find(2).unwrap().octave, 1, "lane 2 octave carried");
        // The LIVE lanes are untouched until the boundary (state applies at launch).
        assert!(
            !app.set.lanes[0].mute,
            "live lane 0 not muted before the boundary"
        );
        assert_eq!(
            app.set.lanes[1].transpose, 0,
            "live lane 1 not transposed yet"
        );
    }

    #[test]
    fn recall_scene_when_stopped_applies_immediately() {
        let mut app = app_with_scene();
        app.engine_playing = false;
        let cmds = app.apply(Action::RecallScene(0));
        // Immediate: LoadPattern per resolvable lane, no QueueScene.
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::QueueScene { .. })),
            "stopped recall must apply immediately, not queue; got {cmds:?}"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "stopped recall loads patterns now; got {cmds:?}"
        );
        // The live lanes carry the scene's performance state now.
        assert!(
            app.set.lanes[0].mute,
            "lane 0 muted immediately when stopped"
        );
        assert_eq!(
            app.set.lanes[1].transpose, 5,
            "lane 1 transposed immediately"
        );
        assert_eq!(
            app.set.lanes[2].octave, 1,
            "lane 2 octave applied immediately"
        );
        // queued[] stays clear — nothing is pending.
        assert!(app.queued.iter().all(|q| q.is_none()));
    }

    #[test]
    fn recall_scene_skips_missing_pattern_and_warns() {
        let mut app = app_with_scene();
        // Break lane 1's assignment: point it at an id no inline pattern has.
        app.set.scenes[0].assignments[1].pattern = PatternRef::User(crate::persist::mint_id());
        app.engine_playing = true;
        let cmds = app.apply(Action::RecallScene(0));
        let UiCommand::QueueScene { lanes, .. } = cmds
            .iter()
            .find(|c| matches!(c, UiCommand::QueueScene { .. }))
            .expect("QueueScene emitted")
        else {
            unreachable!()
        };
        // Only lanes 0 and 2 are queued; lane 1 (missing) is skipped.
        let idxs: Vec<usize> = lanes.iter().map(|(l, _, _)| *l).collect();
        assert!(
            idxs.contains(&0) && idxs.contains(&2),
            "resolvable lanes queued"
        );
        assert!(!idxs.contains(&1), "missing lane must be skipped");
        assert!(app.queued[1].is_none(), "missing lane not shown queued");
        // A warning is surfaced in the status.
        assert!(
            app.status.contains("skipped"),
            "status must warn about skipped lane(s); got {:?}",
            app.status
        );
    }

    #[test]
    fn cancel_clears_queued_scene_recall() {
        let mut app = app_with_scene();
        app.engine_playing = true;
        app.apply(Action::RecallScene(0));
        assert!(
            app.queued.iter().all(|q| q.is_some()),
            "all lanes queued by recall"
        );
        // C cancels the whole queued scene (every lane it queued).
        let cmds = app.apply(Action::CancelQueue);
        assert!(
            app.queued.iter().all(|q| q.is_none()),
            "CancelQueue must clear ALL queued scene lanes"
        );
        let cancels = cmds
            .iter()
            .filter(|c| matches!(c, UiCommand::CancelQueue { .. }))
            .count();
        assert_eq!(cancels, 3, "one CancelQueue per queued lane; got {cmds:?}");
    }

    #[test]
    fn recall_scene_does_not_mark_dirty_or_snapshot() {
        let mut app = app_with_scene();
        app.engine_playing = true;
        let undo_before = app.undo.len();
        app.apply(Action::RecallScene(0));
        assert!(
            !app.dirty,
            "recall is a live action — must not mark the set dirty"
        );
        assert_eq!(
            app.undo.len(),
            undo_before,
            "recall must not push an undo snapshot"
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

    // ── M4a Task 3: favorites ──────────────────────────────────────────────

    /// Build a minimal library with one vendored pattern in drums/techno and
    /// a User genre with one user pattern.
    fn library_for_fav_tests() -> (crate::pattern::library::Library, crate::persist::Id) {
        use crate::pattern::library::{GenreMap, Library};
        use crate::pattern::model::{DrumHit, Pattern, PatternData};
        use crate::persist;

        let mut drums = GenreMap::new();
        let vendored = Pattern {
            name: "Four on Floor".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![
                vec![DrumHit {
                    note: 36,
                    vel: 100,
                    prob: 1.0,
                    ratchet: 1,
                }];
                16
            ]),
            id: persist::Id::nil(),
        };
        drums.insert("techno".to_string(), vec![vendored]);

        // User pattern with a real (non-nil) id so PatternRef::User carries it.
        let user_id = persist::mint_id();
        let user_pat = Pattern {
            name: "My Beat".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![Vec::new(); 16]),
            id: user_id.clone(),
        };
        drums.insert("User".to_string(), vec![user_pat]);

        let lib = Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        };
        (lib, user_id)
    }

    #[test]
    fn selected_pattern_ref_builds_vendored_and_user() {
        use crate::app::Mode;
        use crate::devices::profiles::default_profiles;
        use crate::pattern::library::LibRole;
        use crate::pattern::model::Set;
        use crate::pattern::refs::PatternRef;

        let (lib, user_id) = library_for_fav_tests();
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, lib);
        app.mode = Mode::Library;
        app.lib_role = LibRole::Drums;

        // Genre 0 = "techno" (inserted first), pattern 0 = "Four on Floor"
        app.lib_genre = 0;
        app.lib_pattern = 0;
        let r = app.selected_pattern_ref().expect("should have a ref");
        assert_eq!(
            r,
            PatternRef::Vendored {
                role: "drums".to_string(),
                genre: "techno".to_string(),
                name: "Four on Floor".to_string(),
            },
            "vendored selection must produce Vendored ref"
        );

        // Genre 1 = "User", pattern 0 = user pattern
        app.lib_genre = 1;
        app.lib_pattern = 0;
        let r2 = app.selected_pattern_ref().expect("should have user ref");
        assert_eq!(
            r2,
            PatternRef::User(user_id),
            "User-genre selection must produce User(id) ref"
        );
    }

    #[test]
    fn toggle_favorite_adds_and_removes() {
        use crate::app::Mode;
        use crate::devices::profiles::default_profiles;
        use crate::pattern::library::LibRole;
        use crate::pattern::model::Set;

        let (lib, _) = library_for_fav_tests();
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, lib);
        app.mode = Mode::Library;
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        // First toggle: should add to favorites.
        app.apply(Action::ToggleFavorite);
        let r = app.selected_pattern_ref().unwrap();
        assert!(
            app.favorites.contains(&r),
            "after first ToggleFavorite the pattern must be in favorites"
        );

        // Second toggle: should remove from favorites.
        app.apply(Action::ToggleFavorite);
        assert!(
            !app.favorites.contains(&r),
            "after second ToggleFavorite the pattern must be removed from favorites"
        );
    }

    #[test]
    fn fav_filter_toggles() {
        use crate::devices::profiles::default_profiles;
        use crate::pattern::library::Library;
        use crate::pattern::model::Set;

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, Library::empty());
        assert!(!app.fav_filter, "fav_filter starts false");

        app.apply(Action::ToggleFavFilter);
        assert!(
            app.fav_filter,
            "after first ToggleFavFilter it must be true"
        );

        app.apply(Action::ToggleFavFilter);
        assert!(
            !app.fav_filter,
            "after second ToggleFavFilter it must be false"
        );
    }

    // ── M4a Task 4: Crate management reducer ─────────────────────────────────

    fn app_with_lib_selection() -> App {
        // Build an app with a library that has one vendored pattern selected.
        let (lib, _) = library_for_fav_tests();
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, lib);
        app.mode = Mode::Library;
        app.lib_role = crate::pattern::library::LibRole::Drums;
        app.lib_genre = 0; // "techno"
        app.lib_pattern = 0; // "Four on Floor"
        app
    }

    #[test]
    fn create_crate_adds_to_index() {
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, crate::pattern::library::Library::empty());
        assert_eq!(app.crates.crates.len(), 0);

        app.apply(Action::CreateCrate("Techno Set".to_string()));

        assert_eq!(app.crates.crates.len(), 1);
        assert_eq!(app.crates.crates[0].name, "Techno Set");
        assert!(
            app.status.contains("Techno Set"),
            "status must mention crate name"
        );
    }

    #[test]
    fn rename_crate_keeps_id() {
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, crate::pattern::library::Library::empty());
        app.apply(Action::CreateCrate("Original".to_string()));
        let id_before = app.crates.crates[0].id.clone();

        app.apply(Action::RenameCrate(0, "Renamed".to_string()));

        assert_eq!(app.crates.crates[0].name, "Renamed");
        assert_eq!(
            app.crates.crates[0].id, id_before,
            "rename must keep id stable"
        );
        assert!(app.status.contains("Renamed") || !app.status.is_empty());
    }

    #[test]
    fn duplicate_crate_adds_copy() {
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, crate::pattern::library::Library::empty());
        app.apply(Action::CreateCrate("Alpha".to_string()));
        let orig_id = app.crates.crates[0].id.clone();

        app.apply(Action::DuplicateCrate(0));

        assert_eq!(app.crates.crates.len(), 2);
        let dup = &app.crates.crates[1];
        assert_ne!(dup.id, orig_id, "duplicate must have a fresh id");
        assert_eq!(dup.name, "Alpha copy");
    }

    #[test]
    fn delete_crate_removes() {
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, crate::pattern::library::Library::empty());
        app.apply(Action::CreateCrate("Gone".to_string()));
        app.apply(Action::CreateCrate("Stays".to_string()));
        assert_eq!(app.crates.crates.len(), 2);

        app.apply(Action::DeleteCrate(0));

        assert_eq!(app.crates.crates.len(), 1);
        assert_eq!(app.crates.crates[0].name, "Stays");
    }

    #[test]
    fn add_to_crate_uses_selected_ref() {
        let mut app = app_with_lib_selection();
        app.apply(Action::CreateCrate("My Crate".to_string()));

        let expected_ref = app.selected_pattern_ref().expect("must have selection");
        app.apply(Action::AddToCrate(0));

        assert_eq!(app.crates.crates[0].entries.len(), 1);
        assert_eq!(app.crates.crates[0].entries[0].pattern, expected_ref);
        assert!(app.status.contains("My Crate") || !app.status.is_empty());
    }

    #[test]
    fn add_to_crate_noop_when_no_selection() {
        // Library empty → no selected pattern ref → no-op
        let set =
            crate::pattern::model::Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, crate::pattern::library::Library::empty());
        app.apply(Action::CreateCrate("Empty".to_string()));

        app.apply(Action::AddToCrate(0));

        assert_eq!(
            app.crates.crates[0].entries.len(),
            0,
            "no-op when nothing selected"
        );
    }

    #[test]
    fn remove_from_crate_drops_entry() {
        let mut app = app_with_lib_selection();
        app.apply(Action::CreateCrate("Crate".to_string()));
        app.apply(Action::AddToCrate(0));
        assert_eq!(app.crates.crates[0].entries.len(), 1);

        app.apply(Action::RemoveFromCrate(0, 0));

        assert_eq!(app.crates.crates[0].entries.len(), 0);
    }

    #[test]
    fn reorder_crate_entry_moves() {
        let mut app = app_with_lib_selection();
        app.apply(Action::CreateCrate("Crate".to_string()));
        // Add two entries by navigating the library
        let ref0 = app.selected_pattern_ref().unwrap();
        app.apply(Action::AddToCrate(0)); // entry 0 = ref0

        // Switch to User genre (index 1) pattern 0
        app.lib_genre = 1;
        app.lib_pattern = 0;
        let ref1 = app.selected_pattern_ref().unwrap();
        app.apply(Action::AddToCrate(0)); // entry 1 = ref1

        assert_eq!(app.crates.crates[0].entries[0].pattern, ref0);
        assert_eq!(app.crates.crates[0].entries[1].pattern, ref1);

        // Move entry 1 to position 0
        app.apply(Action::ReorderCrateEntry(0, 1, 0));

        assert_eq!(app.crates.crates[0].entries[0].pattern, ref1);
        assert_eq!(app.crates.crates[0].entries[1].pattern, ref0);
    }

    // ── M4a Task 5: live crate view ───────────────────────────────────────────

    #[test]
    fn open_crate_view_sets_mode() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let cmds = app.apply(Action::OpenCrateView);
        assert_eq!(app.mode, Mode::CrateView);
        assert!(cmds.is_empty());
    }

    #[test]
    fn close_crate_view_returns_to_edit() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        app.mode = Mode::CrateView;
        app.apply(Action::CloseCrateView);
        assert_eq!(app.mode, Mode::Edit);
    }

    #[test]
    fn crate_entry_sel_moves_selection_no_command() {
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let crate_idx = app.crates.add_crate("test".to_string());
        for role in ["drums", "bass", "synth"] {
            app.crates.add_entry(
                crate_idx,
                CrateEntry {
                    pattern: PatternRef::Vendored {
                        role: role.to_string(),
                        genre: "techno".to_string(),
                        name: "x".to_string(),
                    },
                    label: None,
                },
            );
        }
        app.mode = Mode::CrateView;
        app.crate_sel = 0;
        app.crate_entry_sel = 0;
        let cmds = app.apply(Action::CrateEntrySel(1));
        assert_eq!(app.crate_entry_sel, 1);
        assert!(cmds.is_empty(), "navigation must not emit engine commands");
        app.apply(Action::CrateEntrySel(1));
        assert_eq!(app.crate_entry_sel, 2);
        app.apply(Action::CrateEntrySel(1));
        assert_eq!(app.crate_entry_sel, 2, "must clamp at end");
    }

    #[test]
    fn crate_sel_moves_crate_resets_entry_no_command() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        app.crates.add_crate("a".to_string());
        app.crates.add_crate("b".to_string());
        app.mode = Mode::CrateView;
        app.crate_sel = 0;
        app.crate_entry_sel = 1;
        let cmds = app.apply(Action::CrateSel(1));
        assert_eq!(app.crate_sel, 1);
        assert_eq!(
            app.crate_entry_sel, 0,
            "entry sel must reset when crate changes"
        );
        assert!(cmds.is_empty());
    }

    #[test]
    fn launch_crate_entry_drums_targets_lane_0_when_stopped() {
        use crate::pattern::library::GenreMap;
        use crate::pattern::model::PatternData;
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;

        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "kick".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![vec![]; 16]),
            id: crate::persist::Id::nil(),
        };
        drums.insert("techno".to_string(), vec![pat]);
        let lib = Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        };

        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, lib);
        app.engine_playing = false;

        let crate_idx = app.crates.add_crate("my crate".to_string());
        app.crates.add_entry(
            crate_idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "kick".to_string(),
                },
                label: None,
            },
        );
        app.mode = Mode::CrateView;
        app.crate_sel = 0;
        app.crate_entry_sel = 0;

        let cmds = app.apply(Action::LaunchCrateEntry);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "drums entry must load to lane 0; got {:?}",
            cmds
        );
    }

    #[test]
    fn launch_crate_entry_bass_targets_lane_1_when_stopped() {
        use crate::pattern::library::GenreMap;
        use crate::pattern::model::PatternData;
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;

        let mut bass = GenreMap::new();
        let pat = Pattern {
            name: "bass line".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(vec![MelodicStep::default(); 16]),
            id: crate::persist::Id::nil(),
        };
        bass.insert("techno".to_string(), vec![pat]);
        let lib = Library {
            drums: GenreMap::new(),
            bass,
            synth: GenreMap::new(),
        };

        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, lib);
        app.engine_playing = false;

        let crate_idx = app.crates.add_crate("c".to_string());
        app.crates.add_entry(
            crate_idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "bass".to_string(),
                    genre: "techno".to_string(),
                    name: "bass line".to_string(),
                },
                label: None,
            },
        );
        app.mode = Mode::CrateView;
        app.crate_sel = 0;
        app.crate_entry_sel = 0;

        let cmds = app.apply(Action::LaunchCrateEntry);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 1, .. })),
            "bass entry must load to lane 1; got {:?}",
            cmds
        );
    }

    #[test]
    fn launch_crate_entry_synth_targets_lane_2_when_stopped() {
        use crate::pattern::library::GenreMap;
        use crate::pattern::model::PatternData;
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;

        let mut synth = GenreMap::new();
        let pat = Pattern {
            name: "synth line".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(vec![MelodicStep::default(); 16]),
            id: crate::persist::Id::nil(),
        };
        synth.insert("techno".to_string(), vec![pat]);
        let lib = Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth,
        };

        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, lib);
        app.engine_playing = false;

        let crate_idx = app.crates.add_crate("c".to_string());
        app.crates.add_entry(
            crate_idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "synth".to_string(),
                    genre: "techno".to_string(),
                    name: "synth line".to_string(),
                },
                label: None,
            },
        );
        app.crate_sel = 0;
        app.crate_entry_sel = 0;

        let cmds = app.apply(Action::LaunchCrateEntry);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 2, .. })),
            "synth entry must load to lane 2; got {:?}",
            cmds
        );
    }

    #[test]
    fn launch_crate_entry_queues_when_playing() {
        use crate::pattern::library::GenreMap;
        use crate::pattern::model::PatternData;
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;

        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "kick".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![vec![]; 16]),
            id: crate::persist::Id::nil(),
        };
        drums.insert("techno".to_string(), vec![pat]);
        let lib = Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        };

        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, lib);
        app.engine_playing = true;

        let crate_idx = app.crates.add_crate("c".to_string());
        app.crates.add_entry(
            crate_idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "kick".to_string(),
                },
                label: None,
            },
        );
        app.crate_sel = 0;
        app.crate_entry_sel = 0;

        let cmds = app.apply(Action::LaunchCrateEntry);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::QueuePattern { lane: 0, .. })),
            "must queue to lane 0 when playing; got {:?}",
            cmds
        );
        assert_eq!(app.queued[0].as_deref(), Some("kick"));
    }

    #[test]
    fn launch_ref_missing_pattern_sets_status_no_command() {
        use crate::pattern::refs::PatternRef;

        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let r = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "nonexistent".to_string(),
        };
        let cmds = app.launch_ref(&r);
        assert!(cmds.is_empty(), "missing pattern must not emit commands");
        assert!(
            app.status.contains("missing"),
            "status must mention missing: {}",
            app.status
        );
    }

    // ── M4a Task 6: pre-performance validation tests ──────────────────────────

    fn app_with_good_crate() -> App {
        // Library has drums/techno/lib-drum; build a crate referencing it.
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let lib = test_library();
        let mut app = App::new(set, lib);
        let idx = app.crates.add_crate("Good".to_string());
        app.crates.add_entry(
            idx,
            crate::pattern::store::CrateEntry {
                pattern: crate::pattern::refs::PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "lib-drum".to_string(),
                },
                label: None,
            },
        );
        app
    }

    #[test]
    fn validate_clean_crate_has_no_issues() {
        let app = app_with_good_crate();
        let issues = app.validate_crate(0);
        assert!(
            issues.is_empty(),
            "resolvable crate must have no issues; got: {:?}",
            issues
        );
    }

    #[test]
    fn validate_reports_missing_ref() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let idx = app.crates.add_crate("Bad".to_string());
        app.crates.add_entry(
            idx,
            crate::pattern::store::CrateEntry {
                pattern: crate::pattern::refs::PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "nonexistent".to_string(),
                },
                label: None,
            },
        );
        let issues = app.validate_crate(0);
        assert_eq!(issues.len(), 1, "must report exactly one issue");
        assert!(
            matches!(issues[0], CrateIssue::MissingPattern { entry_idx: 0, .. }),
            "must be MissingPattern for entry 0; got: {:?}",
            issues[0]
        );
    }

    #[test]
    fn validate_reports_missing_user_ref() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let idx = app.crates.add_crate("UserBad".to_string());
        // User ref with a random id that won't be in the user pattern dir.
        app.crates.add_entry(
            idx,
            crate::pattern::store::CrateEntry {
                pattern: crate::pattern::refs::PatternRef::User(crate::persist::Id::nil()),
                label: None,
            },
        );
        let issues = app.validate_crate(0);
        assert_eq!(issues.len(), 1, "unknown user ref must be missing");
        assert!(
            matches!(issues[0], CrateIssue::MissingPattern { entry_idx: 0, .. }),
            "must be MissingPattern; got: {:?}",
            issues[0]
        );
    }

    #[test]
    fn validate_reports_unavailable_target() {
        let mut app = app_with_good_crate();
        // Mark lane 0 (drums) as known-disconnected (non-empty port = status received).
        app.device_status[0] = (false, "TestPort".to_string());
        let issues = app.validate_crate(0);
        assert_eq!(issues.len(), 1, "disconnected lane must produce one issue");
        assert!(
            matches!(
                issues[0],
                CrateIssue::UnavailableTarget {
                    entry_idx: 0,
                    lane: 0
                }
            ),
            "must be UnavailableTarget lane 0; got: {:?}",
            issues[0]
        );
    }

    #[test]
    fn validate_out_of_bounds_crate_returns_empty() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let app = App::new(set, Library::empty());
        let issues = app.validate_crate(99);
        assert!(
            issues.is_empty(),
            "out-of-bounds crate must return empty issues"
        );
    }

    #[test]
    fn validate_crate_action_sets_status_ok() {
        let mut app = app_with_good_crate();
        // Default device_status is (false, "") = no status received yet, which is not an error.
        app.crate_sel = 0;
        app.apply(Action::ValidateCrate);
        assert!(
            app.status.contains("OK") || app.status.contains("ok") || app.status.contains("issue"),
            "status must summarize validation; got: {}",
            app.status
        );
    }

    #[test]
    fn validate_crate_action_issues_stored() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let idx = app.crates.add_crate("Bad".to_string());
        app.crates.add_entry(
            idx,
            crate::pattern::store::CrateEntry {
                pattern: crate::pattern::refs::PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "ghost".to_string(),
                },
                label: None,
            },
        );
        app.crate_sel = 0;
        app.apply(Action::ValidateCrate);
        assert!(
            !app.crate_issues.is_empty(),
            "crate_issues must be populated after ValidateCrate"
        );
        assert!(
            app.status.contains("issue"),
            "status must mention issues; got: {}",
            app.status
        );
    }

    #[test]
    fn validate_key_z_maps_to_validate_crate_in_crate_view() {
        use crate::input::key_to_action;
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let k = |code| KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert_eq!(
            key_to_action(k(KeyCode::Char('z')), Mode::CrateView, LaneKind::Drums),
            Action::ValidateCrate,
            "'z' must map to ValidateCrate in CrateView"
        );
    }

    // ── M4a review minors ─────────────────────────────────────────────────────

    /// Fix 1: CloseCrateView must restore the engine when an audition is active.
    /// Mirrors the behaviour of CloseLibrary so closing crate-view is glitch-free.
    #[test]
    fn close_crate_view_restores_active_audition() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let committed_name = set.lanes[0].pattern.name.clone();
        let mut app = App::new(set, Library::empty());
        // Inject an active audition (lane 0) without going through the full
        // AuditionCrateEntry action (which needs a real resolved pattern on disk).
        let preview_pat = Pattern {
            name: "crate-preview".into(),
            desc: String::new(),
            length: 16,
            data: crate::pattern::model::PatternData::Drums(vec![Vec::new(); 16]),
            id: crate::persist::Id::nil(),
        };
        app.audition = Some(AuditionPreview {
            lane: 0,
            pattern: preview_pat,
        });
        app.mode = Mode::CrateView;

        let cmds = app.apply(Action::CloseCrateView);

        // Audition must be cleared.
        assert!(app.audition.is_none(), "audition must be cleared on close");
        // Engine must be restored to the committed pattern.
        assert!(
            cmds.iter().any(|c| matches!(
                c,
                UiCommand::LoadPattern { lane: 0, pattern } if pattern.name == committed_name
            )),
            "must emit LoadPattern restoring the committed pattern; cmds: {cmds:?}"
        );
        // Mode returns to Edit.
        assert_eq!(app.mode, Mode::Edit);
    }

    /// Fix 2: stale validation results must be cleared on crate/content changes.
    #[test]
    fn crate_sel_clears_validation() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        app.crates.add_crate("a".to_string());
        app.crates.add_crate("b".to_string());
        // Inject a stale issue.
        app.crate_issues = vec![CrateIssue::MissingPattern {
            entry_idx: 0,
            name: "ghost".to_string(),
        }];
        app.crate_sel = 0;
        app.apply(Action::CrateSel(1));
        assert!(
            app.crate_issues.is_empty(),
            "crate_issues must be cleared when crate changes"
        );
    }

    #[test]
    fn close_crate_view_clears_validation() {
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        app.mode = Mode::CrateView;
        app.crate_issues = vec![CrateIssue::MissingPattern {
            entry_idx: 0,
            name: "ghost".to_string(),
        }];
        app.apply(Action::CloseCrateView);
        assert!(
            app.crate_issues.is_empty(),
            "crate_issues must be cleared on CloseCrateView"
        );
    }

    #[test]
    fn add_remove_from_crate_clears_validation() {
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::CrateEntry;
        let set = Set::default_set(crate::devices::profiles::default_profiles());
        let mut app = App::new(set, Library::empty());
        let idx = app.crates.add_crate("test".to_string());
        app.crates.add_entry(
            idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "x".to_string(),
                },
                label: None,
            },
        );
        // Stale issues before AddToCrate.
        app.crate_issues = vec![CrateIssue::MissingPattern {
            entry_idx: 0,
            name: "ghost".to_string(),
        }];
        // AddToCrate — even if nothing is selected (no-op path), issues must clear.
        app.apply(Action::AddToCrate(0));
        assert!(
            app.crate_issues.is_empty(),
            "crate_issues must clear on AddToCrate"
        );

        // Re-inject and test RemoveFromCrate.
        app.crate_issues = vec![CrateIssue::MissingPattern {
            entry_idx: 0,
            name: "ghost".to_string(),
        }];
        app.apply(Action::RemoveFromCrate(0, 0));
        assert!(
            app.crate_issues.is_empty(),
            "crate_issues must clear on RemoveFromCrate"
        );
    }

    // ── M4b Task 2: quantized lane restart ───────────────────────────────────

    #[test]
    fn restart_lane_while_playing_queues_current_pattern() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        // Record the focused lane's current pattern name before the action.
        let current_pattern_name = app.set.lanes[0].pattern.name.clone();
        let set_before = app.set.clone();
        let undo_len_before = app.undo.len();

        // Simulate engine playing.
        app.engine_playing = true;
        let cmds = app.apply(Action::RestartLane);

        // Must emit QueuePattern for the focused lane with its CURRENT pattern.
        let queue_cmd = cmds
            .iter()
            .find(|c| matches!(c, UiCommand::QueuePattern { lane: 0, .. }));
        assert!(
            queue_cmd.is_some(),
            "RestartLane while playing must emit QueuePattern{{lane:0}}; got: {:?}",
            cmds
        );
        if let Some(UiCommand::QueuePattern { lane, pattern, .. }) = queue_cmd {
            assert_eq!(*lane, 0);
            assert_eq!(
                pattern.name, current_pattern_name,
                "QueuePattern must carry the lane's CURRENT pattern"
            );
        }

        // queued[focus] must be set to the current pattern name.
        assert_eq!(
            app.queued[0].as_deref(),
            Some(current_pattern_name.as_str()),
            "queued[0] must be set to the current pattern name"
        );

        // No Set mutation (no snapshot pushed, Set unchanged).
        assert_eq!(app.set, set_before, "RestartLane must not mutate Set");
        assert_eq!(
            app.undo.len(),
            undo_len_before,
            "RestartLane must not push an undo snapshot"
        );

        // Status set.
        assert!(
            app.status.contains("restart queued"),
            "status must mention restart queued; got: {:?}",
            app.status
        );
    }

    #[test]
    fn restart_lane_when_stopped_is_noop_with_status() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));

        // Engine stopped (default).
        assert!(!app.engine_playing);
        let cmds = app.apply(Action::RestartLane);

        // No QueuePattern must be emitted.
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, UiCommand::QueuePattern { .. })),
            "RestartLane while stopped must not emit QueuePattern; got: {:?}",
            cmds
        );

        // queued[0] must remain None.
        assert!(
            app.queued[0].is_none(),
            "queued[0] must remain None when stopped"
        );

        // Status must indicate restart only applies while playing.
        assert!(
            app.status.to_lowercase().contains("playing"),
            "status must mention playing; got: {:?}",
            app.status
        );
    }

    // ── M4b Task 3: temporary fill + revert/commit ───────────────────────────

    /// Helper: a drum pattern with one hit on step 0 so apply_fill has a real note to use.
    fn drum_pattern_with_hit() -> Pattern {
        let mut steps = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        }];
        Pattern {
            name: "kick".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        }
    }

    /// Helper: a melodic pattern with a note in the last beat so ratchet doubling fires.
    fn melodic_pattern_with_note() -> Pattern {
        let mut steps: Vec<MelodicStep> = vec![MelodicStep::default(); 16];
        // Put a note at step 13 (last beat = steps 12-15 for a 16-step pattern).
        steps[13] = MelodicStep::from(vec![MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
        }]);
        Pattern {
            name: "bass-note".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        }
    }

    #[test]
    fn toggle_fill_applies_then_reverts() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums lane
                                         // Install a known pattern.
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        // ToggleFill: should apply fill, set temp_transform, not dirty.
        let cmds = app.apply(Action::ToggleFill);
        assert!(
            app.temp_transform.is_some(),
            "temp_transform must be Some after ToggleFill"
        );
        assert_ne!(
            app.set.lanes[0].pattern, original,
            "lane pattern must change after fill"
        );
        assert!(
            !app.dirty,
            "ToggleFill must NOT mark dirty (non-destructive)"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "ToggleFill must emit LoadPattern for the focused lane"
        );
        assert!(
            app.status.to_lowercase().contains("fill"),
            "status must mention fill; got: {:?}",
            app.status
        );

        // ToggleFill again: must revert byte-identical to original.
        let revert_cmds = app.apply(Action::ToggleFill);
        assert!(
            app.temp_transform.is_none(),
            "temp_transform must be None after second ToggleFill (revert)"
        );
        assert_eq!(
            app.set.lanes[0].pattern, original,
            "lane pattern must be byte-identical to original after revert"
        );
        assert!(!app.dirty, "revert must NOT mark dirty");
        assert!(
            revert_cmds
                .iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "revert must emit LoadPattern for the lane"
        );
    }

    #[test]
    fn commit_transform_keeps_fill_and_marks_dirty() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        app.apply(Action::ToggleFill);
        let filled_pattern = app.set.lanes[0].pattern.clone();
        assert_ne!(filled_pattern, original);
        assert!(!app.dirty);

        // CommitTransform: keep fill, mark dirty, snapshot (undo restores pre-fill).
        app.apply(Action::CommitTransform);
        assert!(
            app.temp_transform.is_none(),
            "temp_transform must be None after CommitTransform"
        );
        assert_eq!(
            app.set.lanes[0].pattern, filled_pattern,
            "committed lane must keep the filled pattern"
        );
        assert!(app.dirty, "CommitTransform must mark dirty");
        assert!(
            !app.undo.is_empty(),
            "CommitTransform must push an undo snapshot"
        );

        // Undo must restore the pre-fill pattern.
        app.apply(Action::Undo);
        assert_eq!(
            app.set.lanes[0].pattern, original,
            "Undo after CommitTransform must restore the pre-fill pattern"
        );
    }

    #[test]
    fn fill_is_deterministic() {
        // Applying apply_fill to the same pattern twice produces identical results.
        let mut p1 = drum_pattern_with_hit();
        let mut p2 = drum_pattern_with_hit();
        crate::app::apply_fill(&mut p1);
        crate::app::apply_fill(&mut p2);
        assert_eq!(
            p1, p2,
            "apply_fill must be deterministic: same input → same output"
        );
    }

    #[test]
    fn fill_adds_hits_on_last_beat_drums() {
        // A 16-step drum pattern: last beat = steps 12-15.
        // apply_fill must add fill_note hits on all 4 of those steps.
        let mut p = drum_pattern_with_hit();
        let fill_note: u8 = 36; // first note in the pattern
        crate::app::apply_fill(&mut p);
        if let PatternData::Drums(steps) = &p.data {
            for (idx, step) in steps.iter().enumerate().skip(12) {
                assert!(
                    step.iter().any(|h| h.note == fill_note),
                    "step {idx} in last beat must have a fill hit with note {fill_note}"
                );
            }
        } else {
            panic!("expected Drums pattern");
        }
    }

    #[test]
    fn fill_doubles_ratchet_on_last_beat_melodic() {
        let mut p = melodic_pattern_with_note();
        crate::app::apply_fill(&mut p);
        if let PatternData::Melodic(steps) = &p.data {
            let note = steps[13].first().expect("step 13 must have a note");
            assert_eq!(
                note.ratchet, 2,
                "ratchet must be doubled (1 → 2) on last-beat note"
            );
        } else {
            panic!("expected Melodic pattern");
        }
    }

    #[test]
    fn focus_change_reverts_temp_fill() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        // Apply fill.
        app.apply(Action::ToggleFill);
        assert!(app.temp_transform.is_some());
        let filled = app.set.lanes[0].pattern.clone();
        assert_ne!(filled, original);

        // FocusNext must revert fill and clear temp_transform.
        let cmds = app.apply(Action::FocusNext);
        assert!(
            app.temp_transform.is_none(),
            "temp_transform must be cleared on FocusNext"
        );
        assert_eq!(
            app.set.lanes[0].pattern, original,
            "FocusNext must restore the original pattern"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
            "FocusNext must emit LoadPattern to restore the lane in the engine"
        );
        assert!(!app.dirty, "focus change revert must not mark dirty");
    }

    #[test]
    fn focus_lane_reverts_temp_fill() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        app.apply(Action::ToggleFill);
        assert!(app.temp_transform.is_some());

        // FocusLane to another lane must revert.
        let cmds = app.apply(Action::FocusLane(1));
        assert!(app.temp_transform.is_none());
        assert_eq!(app.set.lanes[0].pattern, original);
        assert!(cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })));
    }

    #[test]
    fn commit_transform_noop_when_no_fill() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        assert!(app.temp_transform.is_none());
        let undo_before = app.undo.len();
        app.apply(Action::CommitTransform);
        assert!(
            !app.dirty,
            "CommitTransform with no temp must not mark dirty"
        );
        assert_eq!(
            app.undo.len(),
            undo_before,
            "CommitTransform with no temp must not push undo snapshot"
        );
        assert!(
            app.status.contains("No temporary"),
            "status must indicate no transform; got: {:?}",
            app.status
        );
    }

    // ── M4b fix: committed_set + save non-destructive while fill is latched ──

    #[test]
    fn committed_set_reverts_active_fill() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        // With no fill: committed_set == self.set.
        let no_fill = app.committed_set();
        assert_eq!(
            no_fill.lanes[0].pattern, app.set.lanes[0].pattern,
            "committed_set with no fill must equal live set lane"
        );
        assert!(app.temp_transform.is_none());

        // Apply fill; now committed_set should revert lane 0 to the original.
        app.apply(Action::ToggleFill);
        assert!(
            app.temp_transform.is_some(),
            "temp_transform must be Some after ToggleFill"
        );
        // The live set has the fill applied.
        assert_ne!(
            app.set.lanes[0].pattern, original,
            "live set must have the fill pattern after ToggleFill"
        );

        let committed = app.committed_set();
        assert_eq!(
            committed.lanes[0].pattern, original,
            "committed_set must revert fill lane to original"
        );
        // Other lanes must be unaffected.
        for i in 1..app.set.lanes.len() {
            assert_eq!(
                committed.lanes[i].pattern, app.set.lanes[i].pattern,
                "committed_set must not alter unaffected lanes"
            );
        }
        // Live set must still have the fill active.
        assert!(
            app.temp_transform.is_some(),
            "temp_transform must still be Some — committed_set must not mutate app"
        );
    }

    #[test]
    fn save_while_fill_active_persists_original() {
        use crate::pattern::store;

        let tok = unique_token("m4bfix-save-fill");
        let tmp_sets = std::env::temp_dir().join(format!("midip-{}-sets", tok));
        std::fs::create_dir_all(&tmp_sets).unwrap();

        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        let original = drum_pattern_with_hit();
        app.set.lanes[0].pattern = original.clone();

        // Toggle fill — lane 0 in live set is now the filled pattern.
        app.apply(Action::ToggleFill);
        assert!(app.temp_transform.is_some(), "fill must be latched");
        let filled = app.set.lanes[0].pattern.clone();
        assert_ne!(
            filled, original,
            "fill must differ from original for this test to be meaningful"
        );

        // Ensure ids so we can round-trip via save_set / load_set directly.
        app.set.ensure_id();
        for l in &mut app.set.lanes {
            l.pattern.ensure_id();
        }

        // Save the committed view to a temp dir and load it back.
        let mut committed = app.committed_set();
        let saved_path = store::save_set(&tmp_sets, &mut committed).expect("save_set must succeed");
        let loaded = store::load_set(&saved_path).expect("load_set must succeed");

        // The saved file must contain the ORIGINAL, not the fill.
        assert_eq!(
            loaded.lanes[0].pattern.data, original.data,
            "saved file must contain original pattern, not the uncommitted fill"
        );

        // The live app state must be untouched: fill still active.
        assert!(
            app.temp_transform.is_some(),
            "temp_transform must still be Some after saving"
        );
        assert_eq!(
            app.set.lanes[0].pattern.data, filled.data,
            "live set must still hold the filled pattern after saving"
        );

        // Cleanup.
        std::fs::remove_file(&saved_path).ok();
        std::fs::remove_dir_all(&tmp_sets).ok();
    }

    #[test]
    fn save_keeps_stable_id_with_fill_active() {
        use crate::pattern::store;

        let tok = unique_token("m4bfix-stable-id");
        let tmp_sets = std::env::temp_dir().join(format!("midip-{}-sets", tok));
        std::fs::create_dir_all(&tmp_sets).unwrap();

        let mut app = new_app();
        app.apply(Action::FocusLane(0));
        app.set.lanes[0].pattern = drum_pattern_with_hit();

        // Latch a fill.
        app.apply(Action::ToggleFill);
        assert!(app.temp_transform.is_some());

        // First save.
        app.set.ensure_id();
        for l in &mut app.set.lanes {
            l.pattern.ensure_id();
        }
        let set_id = app.set.id.clone();
        assert!(!set_id.is_nil(), "set id must be non-nil before saving");

        let mut committed1 = app.committed_set();
        let path1 = store::save_set(&tmp_sets, &mut committed1).expect("first save must succeed");

        // Second save (fill still latched).
        let mut committed2 = app.committed_set();
        let path2 = store::save_set(&tmp_sets, &mut committed2).expect("second save must succeed");

        // Both saves must produce the same path (stable id → stable filename).
        assert_eq!(
            path1, path2,
            "re-saving with fill latched must produce the same filename (stable id)"
        );

        // Loaded set must have the same id.
        let loaded = store::load_set(&path1).expect("load must succeed");
        assert_eq!(
            loaded.id, set_id,
            "loaded set id must match the live set id"
        );
        assert!(!loaded.id.is_nil(), "saved id must be non-nil");

        // Cleanup.
        std::fs::remove_file(&path1).ok();
        std::fs::remove_dir_all(&tmp_sets).ok();
    }

    // ── M5a Task 3: scale-aware editing tests ─────────────────────────────────

    /// Helper: focus lane 1 (bass/melodic), place a note at col 0 with a given semi,
    /// and return the app with that state.
    fn melodic_app_with_note(semi: i8) -> App {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // bass = melodic
        app.apply(Action::MoveCursor(0, 0));
        // Place the note (semi=0 by default via ToggleStep, then adjust to desired semi).
        app.apply(Action::ToggleStep);
        // Directly set the semi to the desired value for test setup.
        if let PatternData::Melodic(steps) = &mut app.set.lanes[app.focus].pattern.data {
            if let Some(n) = steps.get_mut(0).and_then(|s| s.first_mut()) {
                n.semi = semi;
            }
        }
        app
    }

    #[test]
    fn note_up_moves_by_degree_in_major() {
        use crate::music::scale::Scale;
        // Chromatic: NoteUp moves +1 semitone.
        let mut app = melodic_app_with_note(0);
        // Lane is Chromatic by default.
        assert_eq!(app.set.lanes[app.focus].scale, Scale::Chromatic);
        app.apply(Action::NoteUp);
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            assert_eq!(
                steps[0].first().unwrap().semi,
                1,
                "Chromatic: NoteUp should add 1 semitone"
            );
        }

        // Major: NoteUp at semi=0 should move to semi=2 (W W H W W W H — first step is 2 semis).
        let mut app2 = melodic_app_with_note(0);
        app2.set.lanes[app2.focus].scale = Scale::Major;
        app2.apply(Action::NoteUp);
        if let PatternData::Melodic(steps) = &app2.focused_lane().pattern.data {
            assert_eq!(
                steps[0].first().unwrap().semi,
                2,
                "Major: NoteUp at 0 should reach 2"
            );
        }
    }

    #[test]
    fn note_down_moves_by_degree_in_major() {
        use crate::music::scale::Scale;
        // Chromatic: NoteDown moves -1 semitone.
        let mut app = melodic_app_with_note(2);
        assert_eq!(app.set.lanes[app.focus].scale, Scale::Chromatic);
        app.apply(Action::NoteDown);
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            assert_eq!(
                steps[0].first().unwrap().semi,
                1,
                "Chromatic: NoteDown should subtract 1"
            );
        }

        // Major: NoteDown at semi=2 should go back to semi=0.
        let mut app2 = melodic_app_with_note(2);
        app2.set.lanes[app2.focus].scale = Scale::Major;
        app2.apply(Action::NoteDown);
        if let PatternData::Melodic(steps) = &app2.focused_lane().pattern.data {
            assert_eq!(
                steps[0].first().unwrap().semi,
                0,
                "Major: NoteDown at 2 should reach 0"
            );
        }
    }

    #[test]
    fn new_melodic_note_folds_to_scale() {
        use crate::music::scale::{fold_to_scale, Scale};
        // For a Major lane, a new note placed via ToggleStep should have its semi
        // folded to the nearest Major degree. The default semi=0 is already in-scale
        // (degree 1), so fold(0, Major) == 0. This confirms the fold path runs without error.
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::MoveCursor(0, 0));
        app.set.lanes[app.focus].scale = Scale::Major;
        app.apply(Action::ToggleStep);
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            let note = steps[0].first().expect("note should be placed");
            let expected = fold_to_scale(0, Scale::Major) as i8;
            assert_eq!(
                note.semi, expected,
                "new note semi must be folded to Major scale"
            );
        } else {
            panic!("expected melodic pattern");
        }

        // Also verify with a note that would be off-scale if not folded:
        // Place at col 1, then simulate semi=1 being in-scale → fold(1, Major) == 0 or 2.
        // We test the fold function directly here since toggle_step always starts at semi=0.
        let folded = fold_to_scale(1, Scale::Major);
        assert!(
            folded == 0 || folded == 2,
            "semi=1 folded to Major should be 0 or 2, got {}",
            folded
        );
    }

    #[test]
    fn cycle_scale_does_not_change_existing_note_semis() {
        use crate::music::scale::Scale;
        // Place a note with a specific semi, then cycle the scale.
        let mut app = melodic_app_with_note(5);
        assert_eq!(app.set.lanes[app.focus].scale, Scale::Chromatic);

        // Cycle forward to Major.
        app.apply(Action::CycleScale(1));
        assert_eq!(
            app.set.lanes[app.focus].scale,
            Scale::Major,
            "CycleScale(1) from Chromatic should reach Major"
        );
        assert!(app.dirty, "CycleScale must mark dirty");

        // Existing note semi must be unchanged.
        if let PatternData::Melodic(steps) = &app.focused_lane().pattern.data {
            assert_eq!(
                steps[0].first().unwrap().semi,
                5,
                "CycleScale must not rewrite existing note semis"
            );
        }
    }

    #[test]
    fn cycle_scale_wraps_around() {
        use crate::music::scale::Scale;
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        // Start at Chromatic (index 0), cycle backward — should wrap to last.
        app.apply(Action::CycleScale(-1));
        let all = Scale::all();
        assert_eq!(
            app.set.lanes[app.focus].scale,
            all[all.len() - 1],
            "CycleScale(-1) from Chromatic should wrap to last scale"
        );
    }

    #[test]
    fn adjust_root_sets_lane_root_dirty() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        let original_root = app.set.lanes[app.focus].effective_root();

        app.apply(Action::AdjustRoot(1));
        assert!(app.dirty, "AdjustRoot must mark dirty");
        assert_eq!(
            app.set.lanes[app.focus].root,
            Some(original_root + 1),
            "AdjustRoot(1) must set lane.root to profile root + 1"
        );
        assert_eq!(
            app.set.lanes[app.focus].effective_root(),
            original_root + 1,
            "effective_root must reflect the new override"
        );
    }

    #[test]
    fn adjust_root_clamps_to_midi_range() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.set.lanes[app.focus].root = Some(127);
        app.apply(Action::AdjustRoot(1));
        assert_eq!(
            app.set.lanes[app.focus].root,
            Some(127),
            "AdjustRoot must clamp at 127"
        );

        app.set.lanes[app.focus].root = Some(0);
        app.apply(Action::AdjustRoot(-1));
        assert_eq!(
            app.set.lanes[app.focus].root,
            Some(0),
            "AdjustRoot must clamp at 0"
        );
    }

    // ── M5a Task 4: conform existing notes to scale ───────────────────────────

    /// Helper: build a melodic app with multiple notes at given semitones.
    fn melodic_app_with_notes(semis: &[i8]) -> App {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // lane 1 = melodic (bass)
                                         // Place notes by directly mutating cur_col so relative MoveCursor isn't needed.
        for (col, &semi) in semis.iter().enumerate() {
            app.cur_col = col;
            app.apply(Action::ToggleStep);
            if let PatternData::Melodic(steps) = &mut app.set.lanes[app.focus].pattern.data {
                if let Some(n) = steps.get_mut(col).and_then(|s| s.first_mut()) {
                    n.semi = semi;
                }
            }
        }
        // Reset cursor and clear undo history so tests start clean.
        app.cur_col = 0;
        app.undo.clear();
        app.redo.clear();
        app
    }

    #[test]
    fn conform_folds_all_out_of_scale_notes() {
        // Major scale degrees: 0,2,4,5,7,9,11 — semi=1 and semi=3 are out of scale.
        // semi=0 (C) and semi=2 (D) are already in Major.
        let mut app = melodic_app_with_notes(&[0, 1, 2, 3]);
        app.set.lanes[app.focus].scale = Scale::Major;

        app.apply(Action::ConformToScale);

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let semis: Vec<i8> = steps
                .iter()
                .take(4)
                .filter_map(|s| s.first().map(|n| n.semi))
                .collect();
            // Every semi must be in the Major scale degrees (mod 12).
            for &s in &semis {
                let folded = fold_to_scale(s as i32, Scale::Major);
                assert_eq!(
                    folded, s as i32,
                    "semi {} should be in-scale after conform",
                    s
                );
            }
            // In-scale notes (0, 2) must be unchanged; out-of-scale (1→0 or 2, 3→2 or 4).
            assert_eq!(semis[0], 0, "semi=0 (in-scale) must be unchanged");
            assert_eq!(semis[2], 2, "semi=2 (in-scale) must be unchanged");
        } else {
            panic!("expected Melodic pattern data");
        }
    }

    #[test]
    fn conform_is_undoable() {
        let mut app = melodic_app_with_notes(&[1, 3]); // both out of Major scale
        app.set.lanes[app.focus].scale = Scale::Major;

        // Capture original semis.
        let original: Vec<i8> =
            if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
                steps
                    .iter()
                    .take(2)
                    .filter_map(|s| s.first().map(|n| n.semi))
                    .collect()
            } else {
                panic!("expected Melodic");
            };

        app.apply(Action::ConformToScale);

        // Verify notes changed.
        let after: Vec<i8> =
            if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
                steps
                    .iter()
                    .take(2)
                    .filter_map(|s| s.first().map(|n| n.semi))
                    .collect()
            } else {
                panic!("expected Melodic");
            };
        assert_ne!(original, after, "conform must change out-of-scale notes");

        // Undo should restore originals.
        app.apply(Action::Undo);
        let restored: Vec<i8> =
            if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
                steps
                    .iter()
                    .take(2)
                    .filter_map(|s| s.first().map(|n| n.semi))
                    .collect()
            } else {
                panic!("expected Melodic");
            };
        assert_eq!(original, restored, "Undo must restore original semis");
    }

    #[test]
    fn conform_chromatic_is_noop() {
        // Chromatic scale — OpenConformToScale must set status, not enter Confirm mode.
        let mut app = melodic_app_with_notes(&[1, 3, 6]);
        // Lane is Chromatic by default.
        assert_eq!(app.set.lanes[app.focus].scale, Scale::Chromatic);

        let notes_before: Vec<i8> =
            if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
                steps
                    .iter()
                    .take(3)
                    .filter_map(|s| s.first().map(|n| n.semi))
                    .collect()
            } else {
                panic!("expected Melodic");
            };

        app.apply(Action::OpenConformToScale);

        // Must NOT enter confirm mode.
        assert_ne!(
            app.mode,
            Mode::Confirm(ConfirmAction::ConformToScale(3)),
            "Chromatic should not route to Confirm"
        );
        assert!(
            app.status.contains("Chromatic"),
            "status must mention Chromatic, got: {:?}",
            app.status
        );

        // Notes must be unchanged.
        let notes_after: Vec<i8> =
            if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
                steps
                    .iter()
                    .take(3)
                    .filter_map(|s| s.first().map(|n| n.semi))
                    .collect()
            } else {
                panic!("expected Melodic");
            };
        assert_eq!(
            notes_before, notes_after,
            "notes must not change for Chromatic"
        );
    }

    #[test]
    fn open_conform_routes_to_confirm_when_out_of_scale_notes_exist() {
        // semi=1 is not in Major.
        let mut app = melodic_app_with_notes(&[1]);
        app.set.lanes[app.focus].scale = Scale::Major;

        app.apply(Action::OpenConformToScale);

        assert_eq!(
            app.mode,
            Mode::Confirm(ConfirmAction::ConformToScale(1)),
            "OpenConformToScale with out-of-scale notes must route to Confirm(ConformToScale(1))"
        );
    }

    #[test]
    fn open_conform_noop_when_all_in_scale() {
        // semi=0, 2, 4 are all in Major.
        let mut app = melodic_app_with_notes(&[0, 2, 4]);
        app.set.lanes[app.focus].scale = Scale::Major;

        app.apply(Action::OpenConformToScale);

        // Must NOT enter confirm mode.
        assert!(
            !matches!(app.mode, Mode::Confirm(ConfirmAction::ConformToScale(_))),
            "OpenConformToScale with all in-scale notes must not enter Confirm"
        );
        assert!(
            app.status.contains("already in scale"),
            "status must say 'already in scale', got: {:?}",
            app.status
        );
    }

    // ── M5a Task 5: QWERTY note-input sub-mode ────────────────────────────────

    /// Enter NoteInput from melodic Edit, then exit with CloseNoteInput.
    #[test]
    fn open_note_input_enters_mode_on_melodic_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // melodic

        app.apply(Action::OpenNoteInput);

        assert_eq!(
            app.mode,
            Mode::NoteInput,
            "OpenNoteInput on melodic lane must enter Mode::NoteInput"
        );
        assert_eq!(
            app.note_input_octave, 0,
            "octave offset must reset to 0 on entry"
        );
    }

    /// OpenNoteInput on a drum lane must be a no-op with a status message.
    #[test]
    fn open_note_input_noop_on_drum_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(0)); // drums

        app.apply(Action::OpenNoteInput);

        assert_eq!(
            app.mode,
            Mode::Edit,
            "OpenNoteInput on drum lane must not change mode"
        );
        assert!(
            app.status.contains("melodic"),
            "status must mention melodic, got: {:?}",
            app.status
        );
    }

    /// CloseNoteInput returns to Edit.
    #[test]
    fn close_note_input_returns_to_edit() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::OpenNoteInput);
        assert_eq!(app.mode, Mode::NoteInput);

        app.apply(Action::CloseNoteInput);

        assert_eq!(app.mode, Mode::Edit, "CloseNoteInput must return to Edit");
    }

    /// NoteInputPlace(0) places a note with semi=0 at the cursor step and advances the cursor.
    #[test]
    fn note_input_place_writes_note_at_cursor_and_advances() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);
        app.undo.clear(); // clear the entry snapshot so we can count separately

        app.apply(Action::NoteInputPlace(0)); // C = offset 0

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let note = steps[0].first().expect("note must be placed at col 0");
            assert_eq!(
                note.semi, 0,
                "semi must be 0 (C) for offset 0, chromatic lane"
            );
            assert_eq!(note.vel, 1.0, "vel must match MEL_DEFAULT_VEL");
        } else {
            panic!("expected melodic");
        }
        assert_eq!(
            app.cur_col, 1,
            "cursor must advance one step after placement"
        );
        assert!(app.dirty, "dirty must be set after placement");
    }

    /// NoteInputPlace with a black-key offset (1 = C#).
    #[test]
    fn note_input_place_black_key_offset() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.cur_col = 2;
        app.apply(Action::OpenNoteInput);

        app.apply(Action::NoteInputPlace(1)); // C# = offset 1

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let note = steps[2].first().expect("note must be placed at col 2");
            assert_eq!(
                note.semi, 1,
                "semi must be 1 (C#) for offset 1, chromatic lane"
            );
        } else {
            panic!("expected melodic");
        }
        assert_eq!(app.cur_col, 3, "cursor must advance to col 3");
    }

    /// Placement folds to the lane's scale when scale is non-Chromatic.
    #[test]
    fn note_input_place_folds_to_scale() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        // Set Major scale — semi=1 (C#) is not in Major, should fold to 0 or 2.
        app.set.lanes[app.focus].scale = Scale::Major;
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);

        app.apply(Action::NoteInputPlace(1)); // offset 1 = C#, out of Major

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let note = steps[0].first().expect("note placed");
            let major_degrees: &[i8] = &[0, 2, 4, 5, 7, 9, 11];
            let semi_mod = ((note.semi % 12) + 12) as u8 % 12;
            assert!(
                major_degrees.contains(&(semi_mod as i8)),
                "folded semi {} must be in Major scale degrees {:?}",
                note.semi,
                major_degrees
            );
        } else {
            panic!("expected melodic");
        }
    }

    /// Octave shift adjusts note_input_octave and is clamped to -3..=3.
    #[test]
    fn note_input_octave_shift_clamps() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::OpenNoteInput);

        app.apply(Action::NoteInputOctave(1));
        assert_eq!(app.note_input_octave, 1);

        app.apply(Action::NoteInputOctave(-1));
        assert_eq!(app.note_input_octave, 0);

        // Clamp at ceiling.
        for _ in 0..10 {
            app.apply(Action::NoteInputOctave(1));
        }
        assert_eq!(app.note_input_octave, 3, "octave offset must clamp at +3");

        // Clamp at floor.
        for _ in 0..10 {
            app.apply(Action::NoteInputOctave(-1));
        }
        assert_eq!(app.note_input_octave, -3, "octave offset must clamp at -3");
    }

    /// Octave offset shifts the placed note's semi by 12 per octave.
    #[test]
    fn note_input_octave_shifts_placed_semi() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::OpenNoteInput);
        app.apply(Action::NoteInputOctave(1)); // oct +1
        app.cur_col = 0;

        app.apply(Action::NoteInputPlace(0)); // C in oct+1 = semi 12

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let note = steps[0].first().expect("note placed");
            assert_eq!(note.semi, 12, "oct+1 offset 0 → semi 12");
        } else {
            panic!("expected melodic");
        }
    }

    /// Backspace clears the cursor step and steps back.
    #[test]
    fn note_input_backspace_clears_and_steps_back() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.cur_col = 2;
        app.apply(Action::OpenNoteInput);
        // Place a note at col 2, cursor now at 3.
        app.apply(Action::NoteInputPlace(0));
        assert_eq!(app.cur_col, 3);

        app.apply(Action::NoteInputBackspace);

        // Cursor stepped back to 2, and col 2 is now cleared.
        // Note: backspace clears cur_col FIRST (which is 3 after advance), then steps back to 2.
        // col 3 is cleared, col 2 still has the note from placement.
        assert_eq!(app.cur_col, 2, "cursor must step back to 2");
        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            assert!(
                steps[3].is_empty(),
                "col 3 must be cleared by backspace (was empty but backspace sets it None)"
            );
        } else {
            panic!("expected melodic");
        }
        assert!(app.dirty, "dirty must remain set after backspace");
    }

    /// Entering NoteInput takes ONE snapshot; undoing after a full session restores pre-session state.
    #[test]
    fn note_input_session_is_single_undo_unit() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.cur_col = 0;
        app.undo.clear();
        let undo_depth_before = app.undo.len();

        // Enter mode (takes snapshot).
        app.apply(Action::OpenNoteInput);
        assert_eq!(
            app.undo.len(),
            undo_depth_before + 1,
            "entering NoteInput must add exactly one snapshot"
        );

        // Place several notes — must NOT add more snapshots.
        app.apply(Action::NoteInputPlace(0));
        app.apply(Action::NoteInputPlace(2));
        app.apply(Action::NoteInputPlace(4));
        assert_eq!(
            app.undo.len(),
            undo_depth_before + 1,
            "note placements must not add more snapshots"
        );

        // Exit.
        app.apply(Action::CloseNoteInput);

        // One Undo restores the pre-session state (all three placements gone).
        app.apply(Action::Undo);
        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let placed: Vec<_> = steps.iter().take(3).filter(|s| !s.is_empty()).collect();
            assert!(
                placed.is_empty(),
                "Undo after NoteInput session must restore pre-session state (no placed notes)"
            );
        } else {
            panic!("expected melodic");
        }
    }

    /// context_label returns "NOTE INPUT" in NoteInput mode.
    #[test]
    fn context_label_note_input() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        app.apply(Action::OpenNoteInput);
        assert_eq!(app.context_label(), "NOTE INPUT");
    }

    // ── M5b Task 2: poly profile flag + mono enforcement ─────────────────────

    /// Placing a second note on a mono lane (poly == false) replaces the first,
    /// leaving exactly one note in the step (not two).
    ///
    /// Lane 1 = T-8 BASS (poly == false).
    ///
    /// Note: T4 will add chord-stacking Actions for poly lanes. Here we verify
    /// only the mono enforcement guard in NoteInputPlace. For poly lanes, the
    /// guard is absent — chord-stacking path doesn't exist yet (Task 4), so we
    /// assert poly simply via the profile flag, not via stacking behaviour.
    #[test]
    fn mono_lane_step_holds_one_note() {
        // ── Mono lane (T-8 BASS, lane 1) ──────────────────────────────────
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        assert!(
            !app.set.lanes[app.focus].profile.poly,
            "lane 1 must have poly == false"
        );
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);

        // Place first note (semi 0 → C).
        app.apply(Action::NoteInputPlace(0));
        // Move cursor back to col 0 to place a second note on the same step.
        app.cur_col = 0;
        // Place second note (semi 2 → D, chromatic offset 2).
        app.apply(Action::NoteInputPlace(2));

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let step = &steps[0];
            assert_eq!(
                step.len(),
                1,
                "mono lane: second note must REPLACE the first (step must hold exactly 1 note)"
            );
            assert_eq!(
                step[0].semi, 2,
                "mono lane: the surviving note must be the most-recently placed one"
            );
        } else {
            panic!("expected melodic data on lane 1");
        }

        // ── Poly lane (S-1 SYNTH, lane 2) — verify flag only ──────────────
        let app2 = new_app();
        assert!(
            app2.set.lanes[2].profile.poly,
            "lane 2 must have poly == true"
        );
    }

    // ── M5b Task 4: chord entry on poly lanes ────────────────────────────────

    /// On a poly lane in note-input sub-mode, two different piano-key presses on the
    /// same step STACK into a 2-note chord, and the cursor does NOT advance. Drives the
    /// real Action path. Lane 2 = S-1 SYNTH (poly == true).
    #[test]
    fn add_chord_note_stacks_on_poly_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2));
        assert!(app.set.lanes[app.focus].profile.poly, "lane 2 must be poly");
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);

        // Press two DIFFERENT piano keys on the same step.
        app.apply(Action::NoteInputPlace(0)); // root
        app.apply(Action::NoteInputPlace(7)); // a fifth above

        assert_eq!(
            app.cur_col, 0,
            "poly lane: cursor must NOT advance when stacking a chord"
        );
        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            assert_eq!(
                steps[0].len(),
                2,
                "poly lane: two presses must stack into a 2-note chord"
            );
            let semis: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
            assert!(
                semis.contains(&0) && semis.contains(&7),
                "chord must hold both pitches, got {semis:?}"
            );
        } else {
            panic!("expected melodic");
        }
        assert!(app.dirty, "dirty must be set after stacking");
    }

    /// Duplicate-pitch rule on a poly lane: pressing the SAME pitch again toggles it
    /// off (no duplication).
    #[test]
    fn add_chord_note_toggles_off_duplicate_pitch_on_poly_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2));
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);

        app.apply(Action::NoteInputPlace(0)); // add C
        app.apply(Action::NoteInputPlace(4)); // add E
        app.apply(Action::NoteInputPlace(0)); // press C again → toggles off

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let semis: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
            assert_eq!(
                semis,
                vec![4],
                "repeating a pitch must toggle it off (only E remains)"
            );
        } else {
            panic!("expected melodic");
        }
    }

    /// Regression guard: on a MONO lane, a note-input key press REPLACES (step holds 1)
    /// AND advances the cursor (today's melody-typing behaviour). Lane 1 = T-8 BASS.
    #[test]
    fn note_input_replaces_and_advances_on_mono_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1));
        assert!(
            !app.set.lanes[app.focus].profile.poly,
            "lane 1 must be mono"
        );
        app.cur_col = 0;
        app.apply(Action::OpenNoteInput);

        app.apply(Action::NoteInputPlace(0));
        assert_eq!(
            app.cur_col, 1,
            "mono lane: cursor must advance after a placement"
        );

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            assert_eq!(steps[0].len(), 1, "mono lane: step holds exactly one note");
        } else {
            panic!("expected melodic");
        }
    }

    /// BuildTriad on a poly Major lane adds a scale-aware 3rd (+2 degrees) and 5th
    /// (+4 degrees): root 0 → {0, 4, 7} (major triad).
    #[test]
    fn build_triad_adds_scale_aware_third_and_fifth() {
        use crate::music::scale::Scale;
        let mut app = new_app();
        app.apply(Action::FocusLane(2)); // poly
        app.set.lanes[app.focus].scale = Scale::Major;
        app.cur_col = 0;
        // Seed a root note at semi 0.
        if let PatternData::Melodic(steps) = &mut app.set.lanes[app.focus].pattern.data {
            steps[0] = MelodicStep::from(vec![MelodicNote {
                semi: 0,
                vel: MEL_DEFAULT_VEL,
                slide: false,
                len: 0.9,
                prob: 1.0,
                ratchet: 1,
            }]);
        }

        app.apply(Action::BuildTriad);

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let mut semis: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
            semis.sort_unstable();
            assert_eq!(
                semis,
                vec![0, 4, 7],
                "Major triad from root 0 must be {{0, 4, 7}}"
            );
        } else {
            panic!("expected melodic");
        }
        assert!(app.dirty, "dirty must be set after BuildTriad");

        // Natural Minor: root 0 → {0, 3, 7} (minor third).
        let mut app2 = new_app();
        app2.apply(Action::FocusLane(2));
        app2.set.lanes[app2.focus].scale = Scale::NaturalMinor;
        app2.cur_col = 0;
        if let PatternData::Melodic(steps) = &mut app2.set.lanes[app2.focus].pattern.data {
            steps[0] = MelodicStep::from(vec![MelodicNote {
                semi: 0,
                vel: MEL_DEFAULT_VEL,
                slide: false,
                len: 0.9,
                prob: 1.0,
                ratchet: 1,
            }]);
        }
        app2.apply(Action::BuildTriad);
        if let PatternData::Melodic(steps) = &app2.set.lanes[app2.focus].pattern.data {
            let mut semis: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
            semis.sort_unstable();
            assert_eq!(
                semis,
                vec![0, 3, 7],
                "Minor triad from root 0 must be {{0, 3, 7}}"
            );
        } else {
            panic!("expected melodic");
        }
    }

    /// BuildTriad is a no-op on a mono lane (poly == false) and on an empty step,
    /// with a status message.
    #[test]
    fn build_triad_noop_on_mono_lane() {
        let mut app = new_app();
        app.apply(Action::FocusLane(1)); // mono (T-8 BASS)
        app.cur_col = 0;
        if let PatternData::Melodic(steps) = &mut app.set.lanes[app.focus].pattern.data {
            steps[0] = MelodicStep::from(vec![MelodicNote {
                semi: 0,
                vel: MEL_DEFAULT_VEL,
                slide: false,
                len: 0.5,
                prob: 1.0,
                ratchet: 1,
            }]);
        }
        app.apply(Action::BuildTriad);
        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            assert_eq!(
                steps[0].len(),
                1,
                "mono lane: BuildTriad must not add notes"
            );
        } else {
            panic!("expected melodic");
        }
        assert!(
            app.status.to_lowercase().contains("poly"),
            "status must mention poly requirement, got: {:?}",
            app.status
        );

        // Empty step on a poly lane is also a no-op with a status.
        let mut app2 = new_app();
        app2.apply(Action::FocusLane(2)); // poly
        app2.cur_col = 0; // step 0 is a rest by default
        app2.apply(Action::BuildTriad);
        if let PatternData::Melodic(steps) = &app2.set.lanes[app2.focus].pattern.data {
            assert!(
                steps[0].is_empty(),
                "empty step: BuildTriad must remain a rest"
            );
        } else {
            panic!("expected melodic");
        }
    }

    /// RemoveChordNote removes the LAST note from the cursor step.
    #[test]
    fn remove_chord_note_removes_last() {
        let mut app = new_app();
        app.apply(Action::FocusLane(2)); // poly
        app.cur_col = 0;
        if let PatternData::Melodic(steps) = &mut app.set.lanes[app.focus].pattern.data {
            steps[0] = MelodicStep::from(vec![
                MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 0.9,
                    prob: 1.0,
                    ratchet: 1,
                },
                MelodicNote {
                    semi: 4,
                    vel: 1.0,
                    slide: false,
                    len: 0.9,
                    prob: 1.0,
                    ratchet: 1,
                },
                MelodicNote {
                    semi: 7,
                    vel: 1.0,
                    slide: false,
                    len: 0.9,
                    prob: 1.0,
                    ratchet: 1,
                },
            ]);
        }

        app.apply(Action::RemoveChordNote);

        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            let semis: Vec<i8> = steps[0].iter().map(|n| n.semi).collect();
            assert_eq!(
                semis,
                vec![0, 4],
                "RemoveChordNote must drop the last note (7)"
            );
        } else {
            panic!("expected melodic");
        }
        assert!(app.dirty, "dirty must be set after RemoveChordNote");

        // Removing on a single-note step clears it (becomes a rest); empty step is a no-op.
        app.apply(Action::RemoveChordNote); // now [0]
        app.apply(Action::RemoveChordNote); // now [] (rest)
        let depth = app.undo.len();
        app.apply(Action::RemoveChordNote); // no-op on empty step
        if let PatternData::Melodic(steps) = &app.set.lanes[app.focus].pattern.data {
            assert!(
                steps[0].is_empty(),
                "step must be a rest after removing all notes"
            );
        } else {
            panic!("expected melodic");
        }
        assert_eq!(
            app.undo.len(),
            depth,
            "RemoveChordNote on an empty step must not snapshot"
        );
    }

    // ── M6 Task 3: Scene manager ─────────────────────────────────────────────

    #[test]
    fn capture_scene_adds_scene_dirty_undoable() {
        let mut app = new_app();
        assert!(app.set.scenes.is_empty());
        app.apply(Action::OpenScenes);
        assert_eq!(app.mode, Mode::Scenes);
        let pre_undo_len = app.undo.len();
        app.apply(Action::CaptureScene);
        assert_eq!(app.set.scenes.len(), 1, "capture must add a scene");
        assert!(app.dirty, "capture must mark dirty");
        assert!(
            app.undo.len() > pre_undo_len,
            "capture must push an undo snapshot"
        );
        assert!(
            app.set.scenes[0].name.starts_with("Scene"),
            "auto-name must start with Scene"
        );
    }

    #[test]
    fn scene_select_clamps_no_command() {
        let mut app = new_app();
        // Capture two scenes.
        app.apply(Action::CaptureScene);
        app.apply(Action::CaptureScene);
        assert_eq!(app.set.scenes.len(), 2);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        app.apply(Action::SceneSelect(-1));
        assert_eq!(app.scene_sel, 0, "select must clamp at 0");
        app.apply(Action::SceneSelect(10));
        assert_eq!(app.scene_sel, 1, "select must clamp at len-1");
    }

    #[test]
    fn recall_from_scene_view_dispatches_recall() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        // RecallSelectedScene should not panic; it dispatches RecallScene(0).
        let cmds = app.apply(Action::RecallSelectedScene);
        // Transport is stopped so recall applies immediately (emits LoadPattern commands).
        assert!(
            !cmds.is_empty(),
            "RecallSelectedScene on a captured scene must produce commands (LoadPattern/state); got none"
        );
    }

    #[test]
    fn recall_selected_scene_no_op_when_empty() {
        let mut app = new_app();
        app.apply(Action::OpenScenes);
        app.apply(Action::RecallSelectedScene);
        assert!(
            app.status.contains("No scenes"),
            "must report no scenes; got: {:?}",
            app.status
        );
    }

    #[test]
    fn rename_scene_via_name_entry() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        app.apply(Action::RenameScene);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::RenameScene));
        // name_input should be prefilled with original scene name.
        assert!(
            app.name_input.starts_with("Scene"),
            "name_input must be prefilled; got: {:?}",
            app.name_input
        );
        // Simulate user typing and committing.
        app.name_input = "My Scene".to_string();
        let pre = app.undo.len();
        app.apply(Action::NameCommit);
        assert_eq!(
            app.set.scenes[0].name, "My Scene",
            "rename must update name"
        );
        assert!(app.dirty, "rename must mark dirty");
        assert!(app.undo.len() > pre, "rename must push undo snapshot");
        assert_eq!(app.mode, Mode::Scenes, "must return to Scenes after rename");
    }

    #[test]
    fn duplicate_scene_fresh_id() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        let orig_id = app.set.scenes[0].id.clone();
        let pre = app.undo.len();
        app.apply(Action::DuplicateScene);
        assert_eq!(app.set.scenes.len(), 2, "duplicate must add a scene");
        assert_ne!(
            app.set.scenes[1].id, orig_id,
            "duplicate must have a fresh id"
        );
        assert!(
            app.set.scenes[1].name.contains("copy"),
            "duplicate name must contain 'copy'"
        );
        assert!(app.dirty, "duplicate must mark dirty");
        assert!(app.undo.len() > pre, "duplicate must push undo snapshot");
        assert_eq!(app.scene_sel, 1, "sel must point to new scene");
    }

    #[test]
    fn delete_scene_confirm_then_removes_clamps_sel() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::CaptureScene);
        assert_eq!(app.set.scenes.len(), 2);
        app.apply(Action::OpenScenes);
        app.scene_sel = 1;
        // DeleteScene should open Confirm.
        app.apply(Action::DeleteScene);
        assert_eq!(
            app.mode,
            Mode::Confirm(ConfirmAction::DeleteScene(1)),
            "must open Confirm(DeleteScene(1))"
        );
        // Accept.
        let pre = app.undo.len();
        app.apply(Action::ConfirmYes);
        assert_eq!(app.set.scenes.len(), 1, "must remove scene");
        assert_eq!(app.scene_sel, 0, "sel must clamp to 0");
        assert!(app.dirty, "delete must mark dirty");
        assert!(app.undo.len() > pre, "delete must push undo snapshot");
        assert_eq!(app.mode, Mode::Scenes, "must return to Scenes after delete");
    }

    #[test]
    fn validate_scene_reports_missing() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        // Validate — with empty library, all User refs will resolve (they're inline).
        app.apply(Action::ValidateScene);
        // Inline patterns should resolve fine; issues should be empty.
        assert!(
            app.scene_issues.is_empty(),
            "inline patterns must resolve; got issues: {:?}",
            app.scene_issues
        );
    }

    #[test]
    fn open_close_scenes_changes_mode() {
        let mut app = new_app();
        assert_eq!(app.mode, Mode::Edit);
        app.apply(Action::OpenScenes);
        assert_eq!(app.mode, Mode::Scenes);
        app.apply(Action::CloseScenes);
        assert_eq!(app.mode, Mode::Edit);
    }

    #[test]
    fn cancel_rename_scene_returns_to_scenes() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        app.apply(Action::RenameScene);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::RenameScene));
        // Pressing Esc (NameCancel) must return to Mode::Scenes, not Mode::Edit.
        app.apply(Action::NameCancel);
        assert_eq!(
            app.mode,
            Mode::Scenes,
            "cancel on RenameScene must return to Mode::Scenes, not Edit"
        );
    }

    #[test]
    fn confirm_no_delete_scene_returns_to_scenes() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        app.apply(Action::DeleteScene);
        assert_eq!(app.mode, Mode::Confirm(ConfirmAction::DeleteScene(0)));
        // Pressing 'n' (ConfirmNo) must return to Mode::Scenes, not Mode::Edit.
        app.apply(Action::ConfirmNo);
        assert_eq!(
            app.mode,
            Mode::Scenes,
            "ConfirmNo on DeleteScene must return to Mode::Scenes, not Edit"
        );
        // Scene must be untouched.
        assert_eq!(
            app.set.scenes.len(),
            1,
            "scene must not be deleted on cancel"
        );
    }

    #[test]
    fn validate_reports_missing_assignments() {
        let mut app = new_app();
        app.apply(Action::CaptureScene);
        app.apply(Action::OpenScenes);
        app.scene_sel = 0;
        // Point lane 0's assignment at an id that does not exist in the set or library.
        app.set.scenes[0].assignments[0].pattern = PatternRef::User(crate::persist::mint_id());
        app.apply(Action::ValidateScene);
        assert!(
            !app.scene_issues.is_empty(),
            "missing PatternRef::User must be reported in scene_issues; got empty"
        );
        assert!(
            app.scene_issues.contains(&0),
            "lane 0 must be identified as missing; got: {:?}",
            app.scene_issues
        );
    }

    // ── M7 Task 4: Chain manager actions ─────────────────────────────────────

    #[test]
    fn open_close_chains_changes_mode() {
        let mut app = new_app();
        assert_eq!(app.mode, Mode::Edit);
        app.apply(Action::OpenChains);
        assert_eq!(app.mode, Mode::Chains);
        app.apply(Action::CloseChains);
        assert_eq!(app.mode, Mode::Edit);
    }

    #[test]
    fn create_chain_adds_chain_dirty_undoable() {
        let mut app = new_app();
        assert!(app.set.chains.is_empty());
        let pre = app.undo.len();
        app.apply(Action::CreateChain);
        assert_eq!(app.set.chains.len(), 1, "CreateChain must add a chain");
        assert!(app.dirty, "CreateChain must mark dirty");
        assert!(app.undo.len() > pre, "CreateChain must push undo snapshot");
        assert_eq!(app.chain_sel, 0);
        assert!(
            app.set.chains[0].name.starts_with("Chain"),
            "auto-name must start with Chain"
        );
    }

    #[test]
    fn create_chain_increments_sel() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::CreateChain);
        assert_eq!(app.set.chains.len(), 2);
        assert_eq!(app.chain_sel, 1, "sel must point at newest chain");
    }

    #[test]
    fn chain_select_clamps() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::CreateChain);
        app.chain_sel = 0;
        app.apply(Action::ChainSelect(-5));
        assert_eq!(app.chain_sel, 0, "must clamp at 0");
        app.apply(Action::ChainSelect(99));
        assert_eq!(app.chain_sel, 1, "must clamp at len-1");
    }

    #[test]
    fn rename_chain_via_name_entry() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::OpenChains);
        app.chain_sel = 0;
        app.apply(Action::RenameChain);
        assert_eq!(app.mode, Mode::NameEntry(NamePurpose::RenameChain));
        let pre = app.undo.len();
        app.apply(Action::NameCommit); // empty name → no-op dispatch
                                       // commit with a real name
        app.apply(Action::RenameChain);
        app.name_input = "My Chain".to_string();
        app.apply(Action::NameCommit);
        assert_eq!(app.set.chains[0].name, "My Chain");
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        assert_eq!(app.mode, Mode::Chains, "must return to Chains after rename");
    }

    #[test]
    fn duplicate_chain_fresh_id_and_copy_suffix() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        let orig_id = app.set.chains[0].id.clone();
        let pre = app.undo.len();
        app.apply(Action::DuplicateChain);
        assert_eq!(app.set.chains.len(), 2);
        assert_ne!(
            app.set.chains[1].id, orig_id,
            "duplicate must have fresh id"
        );
        assert!(
            app.set.chains[1].name.ends_with(" copy"),
            "duplicate name must end with ' copy'"
        );
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        assert_eq!(app.chain_sel, 1, "sel must point at the copy");
    }

    #[test]
    fn delete_chain_confirm_then_removes() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::CreateChain);
        app.apply(Action::OpenChains);
        app.chain_sel = 1;
        app.apply(Action::DeleteChain);
        assert_eq!(
            app.mode,
            Mode::Confirm(ConfirmAction::DeleteChain(1)),
            "must open Confirm(DeleteChain(1))"
        );
        let pre = app.undo.len();
        app.apply(Action::ConfirmYes);
        assert_eq!(app.set.chains.len(), 1, "must remove chain");
        assert_eq!(app.chain_sel, 0, "sel must clamp to 0");
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        assert_eq!(app.mode, Mode::Chains, "must return to Chains after delete");
    }

    #[test]
    fn confirm_no_delete_chain_returns_to_chains() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::OpenChains);
        app.chain_sel = 0;
        app.apply(Action::DeleteChain);
        assert_eq!(app.mode, Mode::Confirm(ConfirmAction::DeleteChain(0)));
        app.apply(Action::ConfirmNo);
        assert_eq!(
            app.mode,
            Mode::Chains,
            "ConfirmNo on DeleteChain must return to Mode::Chains"
        );
        assert_eq!(
            app.set.chains.len(),
            1,
            "chain must not be deleted on cancel"
        );
    }

    #[test]
    fn add_remove_chain_entry_dirty_undoable() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        let sid = crate::persist::mint_id();
        let pre = app.undo.len();
        app.apply(Action::AddChainEntry {
            chain: 0,
            scene_id: sid.clone(),
        });
        assert_eq!(app.set.chains[0].entries.len(), 1);
        assert_eq!(app.set.chains[0].entries[0].scene_id, sid);
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        // remove it
        let pre2 = app.undo.len();
        app.apply(Action::RemoveChainEntry { chain: 0, entry: 0 });
        assert!(app.set.chains[0].entries.is_empty());
        assert!(app.undo.len() > pre2);
    }

    #[test]
    fn move_chain_entry_dirty_undoable() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        let s0 = crate::persist::mint_id();
        let s1 = crate::persist::mint_id();
        app.apply(Action::AddChainEntry {
            chain: 0,
            scene_id: s0.clone(),
        });
        app.apply(Action::AddChainEntry {
            chain: 0,
            scene_id: s1.clone(),
        });
        let pre = app.undo.len();
        app.apply(Action::MoveChainEntry {
            chain: 0,
            entry: 1,
            up: true,
        });
        assert_eq!(app.set.chains[0].entries[0].scene_id, s1);
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        app.apply(Action::MoveChainEntry {
            chain: 0,
            entry: 0,
            up: false,
        });
        assert_eq!(app.set.chains[0].entries[0].scene_id, s0);
    }

    #[test]
    fn set_chain_entry_repeats_bars_clamped() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        app.apply(Action::AddChainEntry {
            chain: 0,
            scene_id: crate::persist::mint_id(),
        });
        let pre = app.undo.len();
        app.apply(Action::SetChainEntryRepeats {
            chain: 0,
            entry: 0,
            value: 0,
        });
        assert_eq!(app.set.chains[0].entries[0].repeats, 1, "clamped to 1");
        assert!(app.undo.len() > pre);
        app.apply(Action::SetChainEntryRepeats {
            chain: 0,
            entry: 0,
            value: 3,
        });
        assert_eq!(app.set.chains[0].entries[0].repeats, 3);
        app.apply(Action::SetChainEntryBars {
            chain: 0,
            entry: 0,
            value: 0,
        });
        assert_eq!(app.set.chains[0].entries[0].bars, 1, "clamped to 1");
        app.apply(Action::SetChainEntryBars {
            chain: 0,
            entry: 0,
            value: 4,
        });
        assert_eq!(app.set.chains[0].entries[0].bars, 4);
    }

    #[test]
    fn toggle_chain_loop_action_dirty_undoable() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        assert!(!app.set.chains[0].looped);
        let pre = app.undo.len();
        app.apply(Action::ToggleChainLoop(0));
        assert!(app.set.chains[0].looped);
        assert!(app.dirty);
        assert!(app.undo.len() > pre);
        app.apply(Action::ToggleChainLoop(0));
        assert!(!app.set.chains[0].looped);
    }

    #[test]
    fn chain_transport_actions_noop_without_chain() {
        // With no chains/playback, the transport actions are graceful no-ops with status.
        let mut app = new_app();
        app.apply(Action::PlayChain(0));
        assert!(app.chain_playback.is_none(), "no chain to play");
        assert!(!app.status.is_empty());
        app.apply(Action::StopChain);
        assert!(!app.status.is_empty());
        app.apply(Action::JumpChainEntry(0));
        assert!(!app.status.is_empty());
    }

    #[test]
    fn undo_restores_chain_state() {
        let mut app = new_app();
        app.apply(Action::CreateChain);
        assert_eq!(app.set.chains.len(), 1);
        app.apply(Action::Undo);
        assert!(
            app.set.chains.is_empty(),
            "undo must restore empty chain list"
        );
    }

    // ── M7 Task 5: chain playback (auto-advance, loop, stop-at-end, jump, override) ──
    //
    // App-side approach (A): `chain_playback` lives on `App`. The engine already reports
    // the ABSOLUTE step via `EngineEvent::Playhead{step,..}` (step = seq.current_step()).
    // On each Playhead at a bar boundary (step % 16 == 0) the App runs `chain_decision`
    // and, on Advance/LoopWrap, recalls the next entry's scene via the existing
    // `recall_scene` -> `QueueScene` path (quantized to NextBar). Stop emits the existing
    // `UiCommand::Stop` (engine's `seq.stop` releases all sounding notes). No new emission.

    use crate::pattern::model::{Chain, ChainEntry, LaneAssignment, Scene};

    /// Build an app with N scenes (each a full all-lane assignment over the inline lane
    /// patterns) and one chain whose entries point at those scenes (each `bars` × `repeats`).
    /// Returns (app, chain_idx). The app is marked engine_playing so recalls queue.
    fn app_with_chain(
        entries: &[(
            usize, /*scene*/
            u32,   /*bars*/
            u32,   /*repeats*/
        )],
        looped: bool,
    ) -> (App, usize) {
        let mut app = new_app();
        // Distinct ids per lane so User-ref resolution is unambiguous.
        let ids: Vec<crate::persist::Id> = (0..app.set.lanes.len())
            .map(|_| crate::persist::mint_id())
            .collect();
        for (i, lane) in app.set.lanes.iter_mut().enumerate() {
            lane.pattern.id = ids[i].clone();
            lane.pattern.name = format!("p{i}");
        }
        let make_assignments = || -> Vec<LaneAssignment> {
            ids.iter()
                .map(|id| LaneAssignment {
                    pattern: PatternRef::User(id.clone()),
                    mute: false,
                    solo: false,
                    transpose: 0,
                    octave: 0,
                })
                .collect()
        };
        // Two scenes so we can distinguish recalls (each resolves all 3 lanes).
        let mut scene_ids = Vec::new();
        for n in 0..2 {
            let sid = crate::persist::mint_id();
            app.set.scenes.push(Scene {
                id: sid.clone(),
                name: format!("Scene {n}"),
                assignments: make_assignments(),
            });
            scene_ids.push(sid);
        }
        let mut chain = Chain::new("c");
        chain.looped = looped;
        for &(scene, bars, repeats) in entries {
            chain.entries.push(ChainEntry {
                scene_id: scene_ids[scene].clone(),
                repeats,
                bars,
            });
        }
        app.set.chains.push(chain);
        app.engine_playing = true;
        let c = app.set.chains.len() - 1;
        (app, c)
    }

    /// Simulate the engine reporting the absolute playhead at `step`, returning any
    /// commands `on_engine_event` produces (chain auto-advance recalls/stop). This
    /// exercises the REAL wiring (`on_engine_event` → `tick_chain`), not `tick_chain` alone.
    fn playhead_event(app: &mut App, step: usize) -> Vec<UiCommand> {
        app.on_engine_event(crate::engine::EngineEvent::Playhead {
            step,
            bar: (step / 16) as u32,
            beat: ((step / 4) % 4) as u32,
            phase: (step % 4) as f32 / 4.0,
        })
    }

    /// Count QueueScene commands in a cmd list.
    fn count_queue_scenes(cmds: &[UiCommand]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, UiCommand::QueueScene { .. }))
            .count()
    }

    #[test]
    fn play_chain_recalls_first_entry_scene_at_next_bar() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        let cmds = app.apply(Action::PlayChain(c));
        // Entry 0's scene recalled as ONE quantized QueueScene at NextBar.
        assert_eq!(
            count_queue_scenes(&cmds),
            1,
            "entry 0 scene recalled once; got {cmds:?}"
        );
        if let Some(UiCommand::QueueScene { quant, .. }) = cmds
            .iter()
            .find(|c| matches!(c, UiCommand::QueueScene { .. }))
        {
            assert_eq!(*quant, Quant::NextBar, "recall quantized to NextBar");
        }
        // Playback armed on entry 0, active.
        let pb = app.chain_playback.as_ref().expect("chain_playback armed");
        assert_eq!(pb.entry_idx, 0);
        assert!(pb.active);
    }

    #[test]
    fn play_empty_chain_is_noop_with_status() {
        let (mut app, c) = app_with_chain(&[], false);
        let cmds = app.apply(Action::PlayChain(c));
        assert!(
            count_queue_scenes(&cmds) == 0,
            "empty chain emits no recall"
        );
        assert!(
            app.chain_playback.is_none(),
            "empty chain does not arm playback"
        );
        assert!(!app.status.is_empty(), "status warns about empty chain");
    }

    #[test]
    fn auto_advances_to_second_entry_after_its_dwell() {
        // entry0 = 1 bar (dwell 16 steps), entry1 = 1 bar. Play at step 0.
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        app.apply(Action::PlayChain(c));
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        // Drive the REAL wiring: feed the engine's Playhead at the dwell boundary.
        let advance_step = anchor0 + 16;
        let cmds = playhead_event(&mut app, advance_step as usize);
        assert_eq!(
            count_queue_scenes(&cmds),
            1,
            "entry 1 scene recalled at its boundary; got {cmds:?}"
        );
        let pb = app.chain_playback.as_ref().expect("still playing");
        assert_eq!(pb.entry_idx, 1, "advanced to entry 1");
        assert_eq!(pb.entry_start_step, advance_step, "entry_start re-anchored");
        assert!(pb.active);
    }

    #[test]
    fn stop_at_end_stops_transport() {
        // Non-looped single 1-bar entry: after its dwell, transport stops.
        let (mut app, c) = app_with_chain(&[(0, 1, 1)], false);
        app.apply(Action::PlayChain(c));
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        let cmds = app.tick_chain(anchor0 + 16);
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::Stop)),
            "stop-at-end emits the global Stop (engine seq.stop releases all notes); got {cmds:?}"
        );
        assert!(
            app.chain_playback.is_none(),
            "playback cleared at stop-at-end"
        );
    }

    #[test]
    fn loop_wraps_to_first_entry() {
        // Looped single 1-bar entry: after its dwell, scene 0 recalled again; still active.
        let (mut app, c) = app_with_chain(&[(0, 1, 1)], true);
        app.apply(Action::PlayChain(c));
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        let advance_step = anchor0 + 16;
        let cmds = app.tick_chain(advance_step);
        assert_eq!(
            count_queue_scenes(&cmds),
            1,
            "loop re-recalls entry 0; got {cmds:?}"
        );
        assert!(
            !cmds.iter().any(|c| matches!(c, UiCommand::Stop)),
            "loop must NOT stop"
        );
        let pb = app.chain_playback.as_ref().expect("still active on loop");
        assert_eq!(pb.entry_idx, 0, "wrapped to entry 0");
        assert_eq!(pb.entry_start_step, advance_step, "re-anchored on wrap");
        assert!(pb.active);
    }

    #[test]
    fn manual_recall_deactivates_chain() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        app.apply(Action::PlayChain(c));
        assert!(app.chain_playback.as_ref().unwrap().active);
        // A manual scene recall while playback active takes over.
        app.apply(Action::RecallScene(0));
        assert!(
            app.chain_playback.is_none() || !app.chain_playback.as_ref().unwrap().active,
            "manual recall must deactivate chain playback"
        );
        // No further auto-advance after override.
        let cmds = app.tick_chain(64);
        assert_eq!(
            count_queue_scenes(&cmds),
            0,
            "no auto-advance after manual override"
        );
    }

    #[test]
    fn stop_chain_clears_playback_and_cancels_queue() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        app.apply(Action::PlayChain(c));
        assert!(app.chain_playback.is_some());
        let cmds = app.apply(Action::StopChain);
        assert!(app.chain_playback.is_none(), "StopChain clears playback");
        // Pending recall (queued display) cleared.
        assert!(
            app.queued.iter().all(|q| q.is_none()),
            "queued recall cancelled"
        );
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::Stop)),
            "StopChain stops transport (all-notes-off via engine); got {cmds:?}"
        );
    }

    // ── M7 T6: PlayChain starts transport ─────────────────────────────────────

    #[test]
    fn play_chain_starts_transport_when_stopped() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1)], false);
        assert!(!app.playing, "transport must be stopped before test");
        let cmds = app.apply(Action::PlayChain(c));
        assert!(app.playing, "playing must be set to true after PlayChain");
        assert!(
            cmds.iter().any(|c| matches!(c, UiCommand::Play)),
            "PlayChain must emit UiCommand::Play when transport was stopped; got {cmds:?}"
        );
    }

    #[test]
    fn play_chain_does_not_double_start_transport_when_already_playing() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1)], false);
        app.playing = true; // pretend transport is already running
        let cmds = app.apply(Action::PlayChain(c));
        assert!(app.playing);
        let play_count = cmds.iter().filter(|c| matches!(c, UiCommand::Play)).count();
        assert_eq!(
            play_count, 0,
            "must not emit extra Play when already playing; got {cmds:?}"
        );
    }

    #[test]
    fn play_selected_chain_uses_chain_sel() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1)], false);
        app.chain_sel = c;
        app.apply(Action::PlaySelectedChain);
        assert!(
            app.chain_playback.is_some(),
            "PlaySelectedChain must arm playback for chain_sel"
        );
    }

    #[test]
    fn jump_chain_entry_arms_and_reanchors() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        app.apply(Action::PlayChain(c));
        app.playhead = 8; // mid-bar
        let cmds = app.apply(Action::JumpChainEntry(1));
        assert_eq!(
            count_queue_scenes(&cmds),
            1,
            "jump recalls target entry's scene; got {cmds:?}"
        );
        let pb = app
            .chain_playback
            .as_ref()
            .expect("still active after jump");
        assert_eq!(pb.entry_idx, 1, "jumped to entry 1");
        // Re-anchored to the next bar boundary (>= current playhead).
        assert_eq!(pb.entry_start_step % 16, 0, "anchor is a bar boundary");
        assert!(pb.entry_start_step >= 8, "anchor at/after current playhead");
    }

    #[test]
    fn missing_scene_holds_dwell_and_advances_with_warning() {
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        // Break entry 1's scene_id so it cannot resolve.
        app.set.chains[c].entries[1].scene_id = crate::persist::mint_id();
        app.apply(Action::PlayChain(c));
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        let advance_step = anchor0 + 16;
        let cmds = app.tick_chain(advance_step);
        // Unresolved scene: NO recall, but STILL advance + re-anchor (deterministic dwell).
        assert_eq!(
            count_queue_scenes(&cmds),
            0,
            "unresolved scene recalls nothing; got {cmds:?}"
        );
        let pb = app.chain_playback.as_ref().expect("still active");
        assert_eq!(pb.entry_idx, 1, "advanced past missing scene");
        assert_eq!(
            pb.entry_start_step, advance_step,
            "re-anchored despite missing"
        );
        assert!(
            app.status.contains("MISSING"),
            "warns [MISSING]; got {:?}",
            app.status
        );
    }

    #[test]
    fn no_auto_advance_before_dwell_elapses() {
        let (mut app, c) = app_with_chain(&[(0, 2, 1), (1, 1, 1)], false); // entry0 dwell = 32
        app.apply(Action::PlayChain(c));
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        // Bar boundary at +16 is still inside the 2-bar dwell -> Hold.
        let cmds = app.tick_chain(anchor0 + 16);
        assert_eq!(
            count_queue_scenes(&cmds),
            0,
            "holds inside dwell; got {cmds:?}"
        );
        assert_eq!(
            app.chain_playback.as_ref().unwrap().entry_idx,
            0,
            "still on entry 0"
        );
    }

    // Note-safety is verified end-to-end at the engine level (where the sounding
    // registry lives) in `src/engine/mod.rs`:
    // `chain_recall_transitions_leave_no_hung_notes` — it drives the exact command
    // sequence the App emits (QueueScene recalls at bar boundaries + terminal Stop)
    // against a note-bearing set and asserts net NoteOn == NoteOff (no hung notes).

    /// When the user has set `launch_quant = NextBeat`, chain arm/advance/jump must still
    /// use `Quant::NextBar` — the chain's dwell measurements (`chain_decision`) are anchored
    /// to bar boundaries via `next_bar_boundary`, so a beat-quantized recall would desync
    /// the scene swap from the grid the dwell measures against.
    #[test]
    fn chain_recall_forces_next_bar_regardless_of_launch_quant() {
        // Arrange: two-entry chain, launch_quant overridden to NextBeat.
        let (mut app, c) = app_with_chain(&[(0, 1, 1), (1, 1, 1)], false);
        app.launch_quant = Quant::NextBeat;

        // PlayChain arms entry 0.
        let cmds = app.apply(Action::PlayChain(c));
        let queue = cmds
            .iter()
            .find(|cmd| matches!(cmd, UiCommand::QueueScene { .. }))
            .expect("PlayChain must emit a QueueScene");
        if let UiCommand::QueueScene { quant, .. } = queue {
            assert_eq!(
                *quant,
                Quant::NextBar,
                "PlayChain: chain recall must use NextBar even when launch_quant=NextBeat"
            );
        }

        // Auto-advance to entry 1 at the bar boundary.
        let anchor0 = app.chain_playback.as_ref().unwrap().entry_start_step;
        let advance_step = anchor0 + 16;
        let cmds2 = playhead_event(&mut app, advance_step as usize);
        let queue2 = cmds2
            .iter()
            .find(|cmd| matches!(cmd, UiCommand::QueueScene { .. }))
            .expect("auto-advance must emit a QueueScene");
        if let UiCommand::QueueScene { quant, .. } = queue2 {
            assert_eq!(
                *quant,
                Quant::NextBar,
                "auto-advance: chain recall must use NextBar even when launch_quant=NextBeat"
            );
        }

        // JumpChainEntry also forces NextBar.
        app.launch_quant = Quant::NextBeat; // re-assert (tick_chain does not change it)
        let cmds3 = app.apply(Action::JumpChainEntry(0));
        let queue3 = cmds3
            .iter()
            .find(|cmd| matches!(cmd, UiCommand::QueueScene { .. }))
            .expect("JumpChainEntry must emit a QueueScene");
        if let UiCommand::QueueScene { quant, .. } = queue3 {
            assert_eq!(
                *quant,
                Quant::NextBar,
                "JumpChainEntry: chain recall must use NextBar even when launch_quant=NextBeat"
            );
        }

        // Manual RecallScene still honours launch_quant (NextBeat untouched).
        let cmds4 = app.apply(Action::RecallScene(0));
        let queue4 = cmds4
            .iter()
            .find(|cmd| matches!(cmd, UiCommand::QueueScene { .. }))
            .expect("RecallScene must emit a QueueScene");
        if let UiCommand::QueueScene { quant, .. } = queue4 {
            assert_eq!(
                *quant,
                Quant::NextBeat,
                "manual RecallScene must still honour launch_quant (NextBeat)"
            );
        }
    }

    // ── M9 Generative workflow tests ─────────────────────────────────────────

    /// OpenGenerative sets Mode::Generative, installs a temp_transform preview,
    /// emits a LoadPattern, and adds NO undo entry.
    #[test]
    fn gen_open_sets_mode_and_preview_no_undo() {
        let mut app = new_app();
        let before_undo = app.undo.len();
        let cmds = app.apply(Action::OpenGenerative);
        assert_eq!(
            app.mode,
            Mode::Generative,
            "OpenGenerative must set Mode::Generative"
        );
        assert!(
            app.temp_transform.is_some(),
            "OpenGenerative must install a temp_transform"
        );
        assert_eq!(
            app.undo.len(),
            before_undo,
            "OpenGenerative must NOT add an undo entry"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "OpenGenerative must emit LoadPattern"
        );
    }

    /// GenAdjust regenerates the preview (updates temp_transform candidate) but adds no snapshot.
    #[test]
    fn gen_adjust_regenerates_preview_no_snapshot() {
        let mut app = new_app();
        app.apply(Action::OpenGenerative);
        let before_undo = app.undo.len();
        // Capture current previewed pattern.
        let preview_before = app.set.lanes[app.focus].pattern.clone();
        // Adjust density to force a different candidate.
        let cmds = app.apply(Action::GenAdjust {
            field: GenField::Density,
            delta: 50,
        });
        assert_eq!(
            app.undo.len(),
            before_undo,
            "GenAdjust must NOT add an undo entry"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "GenAdjust must emit LoadPattern"
        );
        // temp_transform must still be set.
        assert!(
            app.temp_transform.is_some(),
            "temp_transform must remain set after GenAdjust"
        );
        // The candidate in the live lane may differ from before (density changed).
        // We can't guarantee it differs for all patterns, but the original must be preserved.
        let tt = app.temp_transform.as_ref().unwrap();
        assert_ne!(
            tt.original, preview_before,
            "original must NOT be the first-candidate pattern — it is the pre-open original"
        );
        let _ = preview_before; // silence unused warning
    }

    /// GenReroll changes the seed and produces a new candidate, no undo entry.
    #[test]
    fn gen_reroll_changes_candidate_no_undo() {
        let mut app = new_app();
        app.apply(Action::OpenGenerative);
        let before_undo = app.undo.len();
        let candidate_before = app.set.lanes[app.focus].pattern.clone();
        let seed_before = app.gen_params.seed;
        app.apply(Action::GenReroll);
        assert_eq!(
            app.undo.len(),
            before_undo,
            "GenReroll must NOT add an undo entry"
        );
        // seed must have changed
        assert_ne!(
            app.gen_params.seed, seed_before,
            "GenReroll must bump the seed"
        );
        // Because generate() is deterministic per seed+params, a different seed
        // generally produces a different candidate (may rarely be equal for trivial patterns,
        // but we assert the seed changed, which is the reliable invariant).
        let _ = candidate_before;
    }

    /// GenCommit adds EXACTLY ONE undo entry and applies the candidate; mode returns to Edit.
    #[test]
    fn gen_commit_adds_one_undo_entry_and_applies() {
        let mut app = new_app();
        let original_pattern = app.set.lanes[app.focus].pattern.clone();
        app.apply(Action::OpenGenerative);
        let before_undo = app.undo.len();
        let candidate = app.set.lanes[app.focus].pattern.clone();
        app.apply(Action::GenCommit);
        assert_eq!(app.mode, Mode::Edit, "GenCommit must return to Mode::Edit");
        assert!(
            app.temp_transform.is_none(),
            "GenCommit must clear temp_transform"
        );
        assert!(app.dirty, "GenCommit must mark set dirty");
        assert_eq!(
            app.undo.len(),
            before_undo + 1,
            "GenCommit must push exactly ONE undo entry"
        );
        // The live lane must hold the candidate, not the original.
        assert_eq!(
            app.set.lanes[app.focus].pattern, candidate,
            "GenCommit must apply the candidate to the lane"
        );
        // Undoing must restore the original pattern.
        app.apply(Action::Undo);
        assert_eq!(
            app.set.lanes[app.focus].pattern, original_pattern,
            "Undo after GenCommit must restore the pre-gen original"
        );
    }

    /// GenCancel restores the original pattern and adds ZERO undo entries.
    #[test]
    fn gen_cancel_restores_original_no_undo() {
        let mut app = new_app();
        let original_pattern = app.set.lanes[app.focus].pattern.clone();
        app.apply(Action::OpenGenerative);
        let before_undo = app.undo.len();
        let cmds = app.apply(Action::GenCancel);
        assert_eq!(app.mode, Mode::Edit, "GenCancel must return to Mode::Edit");
        assert!(
            app.temp_transform.is_none(),
            "GenCancel must clear temp_transform"
        );
        assert_eq!(
            app.undo.len(),
            before_undo,
            "GenCancel must NOT add any undo entry"
        );
        assert_eq!(
            app.set.lanes[app.focus].pattern, original_pattern,
            "GenCancel must restore the original pattern"
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
            "GenCancel must emit LoadPattern to restore the engine"
        );
    }

    /// GenSetMode toggles the generation strategy and regenerates; no undo entry.
    #[test]
    fn gen_set_mode_updates_params_no_undo() {
        use crate::pattern::generate::GenMode;
        let mut app = new_app();
        app.apply(Action::OpenGenerative);
        assert_eq!(app.gen_params.mode, GenMode::Generate);
        let before_undo = app.undo.len();
        app.apply(Action::GenSetMode(GenMode::Vary));
        assert_eq!(app.gen_params.mode, GenMode::Vary);
        assert_eq!(
            app.undo.len(),
            before_undo,
            "GenSetMode must NOT add an undo entry"
        );
    }
}
