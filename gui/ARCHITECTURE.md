# midip-gui — Architecture & Implementation Plan

A Tauri 2 + Svelte 5 desktop frontend over the **existing** midip Rust engine.
The Ratatui TUI (`midip`) is unchanged; the GUI (`midip-gui`) is a second driver
over the same sequencing core.

## Key discovery: the core is already decoupled

- `App` (src/app.rs) is **engine-agnostic and pure**:
  - `App::new(set: Set, library: Library) -> App`
  - `App::apply(&mut self, Action) -> Vec<UiCommand>`  — central domain dispatcher (app.rs:1502)
  - `App::on_engine_event(&mut self, EngineEvent) -> Vec<UiCommand>` (app.rs:3956)
  - `App::committed_set(&self) -> Set` — persist view (app.rs:4368)
  - Public fields we read/position: `cur_row`, `cur_col`, `step_scroll`, `focus`, `playhead`,
    `bar`, `dirty`, `link_*`, `device_status`, `queued`, `set`, `mirror_on`, `clock_in_*`.
- Engine seam: `spawn_engine(set, Box<dyn LinkClock>) -> EngineHandle { tx, rx, join }`
  (engine/mod.rs:1401). The GUI reuses this verbatim.
- `main.rs` loop = draw + key→Action→apply→forward UiCommands→engine.tx + drain
  engine.rx→on_engine_event. The GUI backend is the **same loop** minus rendering.

Therefore: **no engine/editing logic is duplicated and no core code is modified.**
The GUI backend holds a headless `App` + `EngineHandle` and mirrors `main.rs`.

## Serde split (drives DTO design)

- Already `Serialize`/`Deserialize`: `Pattern`, `PatternData`, `DrumHit`, `MelodicNote`,
  `MelodicStep`, `TrigCond`, `CcLock`, `PortRef`, `LaneRoute`, `PatternRef`.
- **Not** serde (in-memory): `Set`, `Lane`, `DeviceProfile`, `LaneKind`, `Library`.
  → we build deliberate GUI DTOs (`dto.rs`) that project only what the UI needs.

## Backend (gui/src-tauri, crate `midip-gui`)

- `midip = { path = "../.." }` path dependency (no workspace conversion).
- `GuiState { core: Mutex<Core>, engine_join: Mutex<Option<JoinHandle>>, cmd_tx }`.
  - `Core { app: App, cmd_tx: Sender<UiCommand>, data_dir }`.
- **Bridge contract:**
  - `GuiCommand` enum (serde adjacently-tagged) → `gui_to_actions()` (pure, tested) → `Vec<Action>`.
  - Cell-edit commands carry `(lane,row,col)`; `place_cursor()` clamps + focuses before applying.
  - Tauri commands: `gui_snapshot`, `gui_dispatch(cmd)`, `gui_library`, `gui_set_list`,
    `gui_save`, `gui_save_as`, `gui_load_set`, `gui_load_pattern`.
  - DTOs: `Snapshot { transport, lanes[], focused_pattern, selection, inspector }`.
- **Event pump:** std thread owns a cloned `rx`; per event: brief lock → `on_engine_event`
  → forward cmds → emit. Never holds the Core lock across `recv()`.
  - `transport` events (lightweight, coalesced): position + playhead + play state.
  - `snapshot` events (structural) on Started/Stopped/Launched/Device/Link changes.
- **Shutdown (RunEvent::ExitRequested):** send `Quit`, join engine thread (panic/all-notes-off
  runs in the engine's Quit handler). Never holds a lock while joining.
- Link injected as `Box<dyn LinkClock>`: `AbletonLink` in prod, `FakeLink` in tests.

## Frontend (gui/src)

Svelte 5 (runes) + TS strict + Vite. Ember design tokens in `tokens.css`.
Tabs: Perform · Pattern · Library · Song(later) · Setup.
Components: TransportBar, LaneStrip, PatternGrid→{DrumGrid,MelodicGrid}, StepCell,
StepInspector, PatternLibrary, SetupPanel, StatusToast. Typed `bridge.ts` over invoke/listen;
a `$state` snapshot store updated by events + command returns.

## Tests (Rust, FakeLink/RecordingSink — no hardware)

- `gui_to_actions` translation; `place_cursor` clamping (invalid lane/step).
- `Snapshot` DTO generation from a known Set; pattern lengths 1..=64.
- Commands with no MIDI device (spawn engine w/ FakeLink, dispatch, no panic).
- Shutdown: Quit → join returns.

## Milestones (TUI green after each)

1. Backend crate compiles against lib (path dep) + DTO/command contract + tests. ← integration risk, done first
2. Frontend shell + tokens + bridge wired to real snapshot.
3. Transport + lane strip live.
4. Drum + melodic grids (click/paint) + playhead.
5. Step inspector + library + setup.
6. Persistence (save / save-as / load) + undo/redo.

## Implemented since the first slice

- **Routing** (Setup): per-lane output-port cycling, MIDI-channel adjust, clock-out
  toggle — reuses the engine's route-editor actions via `route_editor_lane` +
  `gui_output_ports`.
- **Library**: audition (isolated preview, gated to muted/stopped lanes),
  favorites (persisted), favorites-only filter.
- **Song mode**: scene recall + capture, chain play/stop with a live current-entry
  highlight (chain auto-advance flows through the event pump). `SongDto` in the snapshot.
- **Chromatic note entry**: click any pitch row in the piano-roll to place that
  (scale-folded) pitch via the note-input path; double-click removes; drag re-pitches.

## Still TUI-only / next

- Scene/chain *editing* (create/reorder/delete entries) — playback + recall work in the GUI.
- Crates, generative tools, command palette, onboarding/recovery.
- Live GUI QA on a desktop session (window open, close-during-playback) — verified
  logically via the headless engine boot/shutdown test, not run headlessly here.
