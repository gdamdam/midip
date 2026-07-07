# Changelog

All notable changes to midip are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/) (pre‑1.0: each
feature milestone is a minor bump).

## [Unreleased]

## [1.3.4] — 2026-07-07 — Docs

### Changed
- README polish: refreshed the stale version badge and Status line to the current release.

## [1.3.3] — 2026-07-02 — CI green (formatting)

### Fixed
- Applied `cargo fmt` to the v1.3.2 test code so the Linux CI `fmt --check` gate passes.

## [1.3.2] — 2026-07-02 — HIGH-severity bug fixes (timing, Link, persistence, UI)

### Fixed
- **Lane swing / clock-div edits now reach the engine (H1).** `AdjustLaneSwing`,
  `ClearLaneSwing` and `CycleClockDiv` sent only `LoadPattern` (which copies the pattern
  only), so the scheduler kept reading the old `swing`/`clock_div` until an unrelated reload.
  Added a dedicated `UiCommand::UpdateLaneParams { lane, swing, clock_div }` that the handlers
  push, so timing changes take effect immediately.
- **Ableton Link start/stop transport sync actually works with real peers (H2).**
  `set_enabled` now also calls `enable_start_stop_sync(on)` (it was never enabled, so
  `isPlaying` was never shared across peers). Local Stop now publishes to Link via a new
  `request_stop` (`set_is_playing(false, …)`), and the engine resets its `link_playing` latch
  on Stop so bar-realignment is no longer a one-shot per process.
- **External-clock Start is no longer one 16th late (H3).** `ClockIn::on_tick` evaluates the
  step boundary on the pre-increment (0-based) count, so the first F8 after a MIDI Start
  materializes step 0 (per spec), instead of waiting for the 6th clock.
- **No more stuck notes when restarting playback (H4).** The three engine paths that call
  `play()` while already playing (external Start, `UiCommand::Play`, Link armed-start) now
  release sounding notes first, instead of clearing the registry with no NoteOffs.
- **Joining a running Link session mid-song now produces notes (H5).** `sync_to_beat` anchors
  from the current beat's fractional step phase instead of assuming beat 0 ↔ origin, so a
  mid-session join (session beat > 0) materializes steps at their correct near-now time.
- **Corrupt/foreign set files no longer crash Save (H6).** `validate_and_repair` now validates
  and regenerates malformed set ids, and all id truncations use a char-boundary-safe helper —
  a short (`"abc"`) or multibyte id can no longer panic on load or Save (session data loss).
- **Library favorites filter no longer desyncs display from actions (H7).** Render and
  navigation/selection/favorite actions now index the same filtered ("visible") list via a
  shared accessor, so the highlighted row is the one that loads/favorites and the count
  indicator matches.

## [1.3.1] — 2026-07-01 — CI green (test + formatting)

### Fixed
- **CI is green again.** Removed the obsolete `link_enabled_sync_drives_step_from_beat`
  integration test: it asserted the pre-1.3.0 immediate-start behaviour (Play under Link
  started the moment `beat_at >= 0.0`), which the current arm-then-start-on-bar-crossing
  model can't reproduce with the static `FakeLink` batch driver. The behaviour it checked
  (beat→step sync while playing, arm/start on a bar boundary) is already covered by the
  step-by-step unit tests in `engine`/`scheduler`.
- Applied `cargo fmt` to the 1.3.0 Link test code so the Linux CI `fmt --check` gate passes.

## [1.2.1] — 2026-06-30 — Routing/undo, tempo-source & persistence fixes

### Fixed
- **Undo/redo re-routes correctly** — restoring a snapshot that changed a lane's device or
  route now re-plans the engine's output ports and releases the affected notes, instead of
  leaving MIDI flowing to the post-change port (held notes could hang on the wrong port).
  A plain step-edit undo (no routing change) is left untouched, so it never cuts sounding notes.
- **Tempo-source exclusivity** — a manual BPM change or Tap now disables Ableton Link (as it
  already did for Clock-In), so manual tempo and Link can't both drive the playhead. Undo/redo
  restores the stored BPM without changing the active tempo source, so undoing an unrelated
  edit no longer silently drops you out of Link.
- **Device/route changes no longer stall timing** — the MIDI device-watcher parks on a timed
  receive, so tearing it down on a port change wakes it immediately instead of blocking the
  timing loop for up to the scan interval (~250 ms).
- **Persistence failures are surfaced** — favorites, crates, and autosave now report a
  "save failed" status when a write fails (e.g. a read-only data dir) instead of silently
  reporting success.

## [1.2.0] — 2026-06-30 — Device library & picker

### Added
- **Device library** — device profiles are now data-driven: a bundled
  `assets/devices/catalog.json` is embedded at build time, and an optional user `devices.json`
  in the data dir is layered on top (user entries override shipped ones by id; the three
  built-in T-8/S-1 profiles stay reserved). Ships profiles for the AIRA Compact T-8/S-1/J-6,
  Behringer RD-8 & TD-3, Arturia DrumBrute Impact & MicroFreak, Korg monologue & minilogue xd,
  Elektron Digitakt, Novation Circuit Tracks (synth + drums), and generic GM-drum / mono / poly
  fallbacks. Third-party note maps were sourced from each device's MIDI implementation chart.
- **Device picker** (`d`) — assign any catalog device to a lane from an overlay, filtered to the
  lane's kind (drum lanes list drum machines, melodic lanes list synths) so the lane's pattern
  stays valid. Selecting a device swaps the lane's profile and re-routes it to that device's
  port + default channel; the engine reconnects automatically.
- **Printable cheat sheet** — a single-page `midip-cheatsheet.pdf`, linked from the README.

### Changed
- Loading a set that references an unknown device id (e.g. a removed custom profile) now falls
  back to a same-kind generic profile and reports a repair note, instead of failing the load.

## [1.1.0] — 2026-06-30 — Routing & persistence fixes

### Fixed
- **Per-lane MIDI routing** — note/CC delivery now routes by the *emitting lane's* port
  rather than by MIDI channel. Two lanes sharing a channel on different ports no longer
  collapse onto a single port. MIDI Clock is unchanged (still sent once per clock-out port).
- **Clock-input persistence** — choosing a clock-in port now marks the set dirty and is saved
  with it; loading a set emits `SetSet` before `SetClockInPort` so the clock-in source lands on
  the freshly-built sequencer (previously the rebuild reset it to Manual); recovery now restores
  the saved clock-in port instead of dropping it.
- **Confirm before discarding unsaved work** — loading a set from the browser while there are
  unsaved edits now prompts a confirmation (matching the documented guarantee); cancelling
  returns to the browser with the work intact.
- **Rename / duplicate are non-destructive with an active fill** — both now serialize the
  committed view (a latched fill reverted), consistent with Save / Save-as.
- **Release archives ship the pattern library** — the release workflow now stages `assets/`
  (the vendored patterns) beside the binary, so published builds no longer start empty.

## [1.0.0] — 2026-06-29 — First stable release

First stable release. The full planned feature set (milestones M1–M10) is complete:
safety/note-ownership foundations, versioned persistence + configurable routing, quantized
launch · audition · set/pattern management, favorites · crates · setlists · performance controls,
scale-aware + chord-capable editing, scenes, song mode / chaining, per-step CC · microtiming ·
trig conditions · per-lane swing/division, generative tools, and MIDI clock input. This release
finalizes the README and the in-app `?` help to cover every feature and keybinding; there are no
functional changes since 0.15.0.

## [0.15.0] — 2026-06-29 — Ember theme

### Changed
- **New "Ember" color palette** — a warm, cozy dark theme replaces the previous mostly-grayscale
  look. Colors are now centralized in a single `Palette` (cream text on a warm-dark background;
  orange/pink/aqua lane accents; amber for pending, green for active/locked, red for errors), so
  the whole UI is cohesive and every surface is recolored from one place. The velocity ramp now
  shades warm (ember→cream) instead of flat gray. Still fully static (no flashing, no
  hue-from-audio) and degrades to monochrome on terminals without color.
- The transport header now shows the version next to the name (` midip v0.15.0 `).

## [0.14.0] — 2026-06-29 — MIDI Clock Input

### Added
- **MIDI clock input** — midip can now follow an external 24-PPQN clock as a slave. Select a
  clock-in port with `W`; the transport header shows `CLK-IN <port> [LOCKED|FREE|LOST]`. While
  following, incoming clock ticks (6 per step) drive both tempo and step advancement, and the
  transport messages Start / Continue / Stop are obeyed (Start plays from the top, Continue
  resumes, Stop halts and releases all notes). If the external clock disappears, midip stops
  cleanly after a short timeout (no drift, no hung notes). Song Position Pointer is parsed
  (SPP-ready) but does not yet reposition. The chosen clock-in port is saved with the set.
- This is midip's first MIDI **input** path; it never disturbs the existing clock-output, Link,
  or per-lane routing — the tempo source (Manual / Link / Clock-In) is mutually exclusive.

### Changed
- Set format version bumped 3 → 4 (**backward-compatible**: old sets load with no clock-in port;
  additive migration).

## [0.13.0] — 2026-06-29 — Per-step CC · Microtiming · Trig Conditions · Per-lane Swing/Division

### Added
- **Per-step CC locks** — lock one or more control-change values to a step (`cc{n}={v}`); they
  send just before that step's NoteOn, with a per-route cache that suppresses redundant resends.
- **Signed microtiming** — nudge a note earlier/later within its step (`\`/`|`, shown `µ±N`),
  clamped to ±half a step so a note never crosses its neighbours; the NoteOff and ratchets move with it.
- **Trig conditions** — fire a note only on certain loops/states (`z` cycles
  Always / 1:2 / 1:3 / 1:4 / Fill / !Fill / 1st / !1st), evaluated before the probability roll.
  A latched **fill** toggle drives the Fill/!Fill conditions.
- **Per-lane swing override** (`a`/`_`) — a lane can swing differently from the global feel
  (`None` = follow global).
- **Per-lane clock division** (`Q`, divide-only /1../4) — a lane can run at half/third/quarter time,
  advancing one step every N global steps; composes with polymeter.

### Changed
- Set format version bumped 2 → 3 (**backward-compatible**: old sets load with all new
  per-step/per-lane fields defaulted; additive migration, no behaviour change).
- The `?` controls overlay is scrollable (`↑`/`↓`, `PageUp`/`PageDown`, `Home`/`End`).

## [0.12.0] — 2026-06-29 — Generative

### Added
- **Generative tools** (`D`) — a panel to generate or vary the focused lane's pattern.
  **Generate** builds a fresh pattern from a target density (drums via Euclidean distribution) and,
  for melodic lanes, pitches within a range **folded to the lane's scale**; **Vary** perturbs the
  current pattern by a mutation amount. Both are **seeded and reproducible** (visible seed, `z` to
  reroll). The candidate previews live and auditions non-destructively, then **commits as a single
  undo** (`Enter`) or reverts (`Esc`) — reusing the existing transform/undo machinery. Panel keys:
  `Tab`/`Shift+Tab` switch Vary/Generate, `d`/`r`/`m` adjust density/range/mutate, `z` rerolls.
  Generation writes only rhythm, pitch, and velocity (no persistence change).

## [0.11.0] — 2026-06-29 — Song Mode

### Added
- **Song mode / chaining** — build an ordered **chain** of scenes that plays back automatically.
  Each entry holds for `bars × repeats` bars, then the chain quantize-launches the next scene on
  the next bar boundary (reusing scene recall — note-safe, no hung notes). Chains can **loop**,
  **stop at the end**, and be **jumped live** to any entry; a manual scene recall takes over and
  stops the chain. Multiple named chains per set.
- **Chain manager** (`K`) — create (`c`), rename (`r`), duplicate (`d`), delete (`x`), play
  (`Enter`, which starts transport), stop (`C`), and jump to the selected entry (`j`); add the
  focused scene as an entry (`a`), navigate entries (`Tab`), reorder/edit `bars` (`[` / `]`) and
  `repeats` (`{` / `}`), and toggle loop (`m`). A live "now playing" line shows the current entry,
  bar position, and loop state; an unresolved scene shows `[MISSING]` and holds its dwell without
  recalling.

### Changed
- Chains are stored inside the set file (**backward-compatible**: old sets load with no chains;
  set format version bumped 1 → 2 via an additive migration).

## [0.10.0] — 2026-06-29 — Scenes

### Added
- **Scenes** — capture the current per-lane performance state (each lane's pattern + mute,
  solo, transpose, and octave) as a named scene, and recall it live. Recall is a **quantized
  all-lane launch on one boundary**: every lane switches to its assigned pattern and state
  together on the next bar/beat (next-beat/next-bar follows the `b` toggle), so the outgoing
  scene plays until the boundary; when stopped, recall applies immediately. A lane whose
  pattern is missing is left untouched and reported, and `C` cancels a queued recall.
- **Scene manager** (`G`) — list, capture (`c`), recall (`Enter`), rename (`r`), duplicate
  (`d`), delete (`x`, with confirmation), and validate (`z`, flags missing assignments)
  scenes, with a per-lane assignment detail view and a queued-recall marker.

### Changed
- Scenes are stored inside the set file (backward-compatible: old sets load with no scenes,
  and adding scenes needs no format-version bump).

## [0.9.0] — 2026-06-29 — Chords & polyphony

### Added
- **Chords on synth lanes** — a melodic step can now hold multiple notes. The S‑1 synth
  lane is polyphonic; the T‑8 bass lane stays monophonic (single note + slide), enforced
  at the edit layer. Chords play as simultaneous notes and every note has a guaranteed
  release path, so stop/panic/mute never leave a hung note.
- **Chord entry** — in the note-input sub-mode on a poly lane, each key **stacks** a note
  onto the current step (pressing the same pitch again removes it) instead of advancing;
  mono lanes still replace-and-advance. `j` builds a **scale-aware triad** from the step's
  root note (a major triad in a major scale, minor in a minor scale, etc.); `J` removes the
  last note of a chord.
- **Chord display** — multi-note steps render with a chord indicator and the detail line
  lists the chord's note names and scale degrees; single-note steps are unchanged.
- **GitHub Actions CI** — fmt + `clippy -D warnings` + the full test suite on Linux and macOS.

### Changed
- The melodic step data model migrated from a single optional note to a list of notes.
  This is **fully backward-compatible**: every existing set, user pattern, and the vendored
  library loads unchanged, and a mono pattern saved by this version still loads in earlier
  builds (rests serialize as `null`, single notes as objects, only true chords as arrays).

## [0.8.0] — 2026-06-29 — Scale-aware melodic editing + note input

### Added
- **Per-lane scales** — choose a root + scale (Chromatic, Major, Natural/Harmonic Minor, the modes,
  Major/Minor Pentatonic, Blues) per melodic lane: `n`/`N` cycles the scale, `h`/`H` moves the root.
  Default is Chromatic, so existing patterns are unchanged.
- **Scale-aware editing** — `↑`/`↓` moves a note by scale degree (semitone in Chromatic); new notes
  fold into the scale; the editor shows the note name and scale degree. Changing the scale never
  rewrites existing notes.
- **Conform to scale** (`X`) — explicitly fold all existing notes in a lane into its scale, with a
  confirmation (showing the count) and undo.
- **Note-input sub-mode** (`I`) — a dedicated QWERTY piano for entering melodies: white keys
  `a s d f g h j k`, black keys `w e t y u`, `z`/`x` shift octave, Backspace clears, Esc exits; entered
  notes fold to the scale. The whole session is a single undo step.

### Changed
- License changed from MIT to AGPL-3.0-or-later.

## [0.7.0] — 2026-06-29 — Performance controls + routable virtual port

### Added
- **Per-drum-voice mute** — mute an individual drum voice (e.g. just the hat) live with backtick
  (`` ` ``), latched and non-destructive; muting releases that voice's sounding note immediately.
- **Quantized lane restart** (`i`) — re-sync a drifted lane by restarting its phase at the next
  bar/beat without changing its pattern.
- **Temporary fill** (`f` to toggle on/off, `F` to commit) — overlay a deterministic fill on the
  focused lane; toggling off reverts it exactly, committing makes it a permanent (undoable) edit.
  Changing lane focus reverts an un-committed fill, and a fill is never saved to disk until committed.

### Fixed
- **The virtual `midip` port is now a first-class routable destination** — select "midip" as a lane's
  output in the route editor (`w`) and that lane's MIDI goes straight to the virtual source that other
  apps read, with `CON ●`. (Previously "midip" only carried audio when the mirror toggle was on and
  could not be targeted per-lane.) The mirror toggle (`M`) still works as a full-stream feed, without
  double-sending a lane that's also routed to "midip".

## [0.6.0] — 2026-06-28 — Favorites · crates · live launch

### Added
- **Favorite patterns** — star any vendored or user pattern in the library (`f`), filter to
  favorites-only (`F`); favorites persist across runs.
- **Crates** — named, ordered, reusable collections of pattern references. Create, rename,
  duplicate, delete, reorder, and add/remove entries; a pattern can live in multiple crates.
- **Live crate view** (`V`) — browse a crate and launch from it live: `↑/↓` select an entry
  (never changes playback), `Enter` launches it **quantized** to the **role-matched lane** (drums→
  drum lane, bass→bass, synth→synth), `a` auditions (gated), `←/→` switches crates, `f` favorites,
  `C` cancels a queued launch.
- **Pre-performance validation** (`z` in the crate view) — reports entries whose pattern is missing
  or whose target lane's device is unavailable, so you can catch problems before a set.
- The `?` controls overlay is now **scrollable + two-column** so it fits any terminal, and documents
  all current keys.

## [0.5.0] — 2026-06-28 — Quantized launch · audition · set/pattern management

### Added
- **Quantized pattern launching** — loading a pattern while playing now QUEUES it and launches it
  exactly once on the next bar (or next beat), restarting that lane at step 1 without disturbing the
  others. Lanes show ACTIVE and `QUEUED⟶` markers; `b` toggles next-bar/next-beat; `C` cancels a queue.
- **Gated, non-destructive audition** — cueing a library pattern (`a`) is allowed only when the focused
  lane is stopped or muted (so it never collides with a live lane), never mutates the saved pattern,
  and is reverted by changing focus or closing the library.
- **Set management** — save-as, rename, duplicate, new, and delete, with confirmation before discarding
  unsaved work or deleting files (from the set browser, `o`).
- **Pattern management** — save the focused lane as a user pattern (`A`), clear a lane (`Z`, with
  confirmation), and duplicate/rename/delete user patterns; user patterns are loadable from the library
  (a "User" section). Vendored library patterns remain immutable.
- **Double-length edit** (`L`) — doubles a pattern's length and repeats its content (16→32), capped at
  64; the lane overview strip now reads correctly for patterns longer than 16 steps.

## [0.4.0] — 2026-06-28 — Virtual mirror output

### Added
- **Optional virtual `midip` MIDI output** — midip can create a virtual MIDI source named `midip`
  (macOS/Linux) that other apps on the same machine can subscribe to.
- **Mirror toggle** (`M`, default off, with a `MIR` indicator) — when on, midip's full output
  stream (all lanes' notes/CC on their channels + the 24 PPQN MIDI clock) is ALSO sent to the
  virtual port, for layering with hardware and letting external apps tempo-sync. The mirror
  preference is persisted across runs.
- The mirror is purely **additive**: it never replaces or disturbs USB/hardware routing, and the
  hardware output is identical whether the mirror is on or off (no double-clock).

## [0.3.0] — 2026-06-28 — Milestone 2: versioned persistence + configurable routing

### Added
- **Crash-safe saves** — all persisted files are written atomically (temp file → fsync → rename),
  so a crash mid-write can never corrupt or truncate the previous good file.
- **Versioned save format** with a migration ladder — old set files (no version) load and are
  upgraded automatically; a file from a newer midip is rejected cleanly instead of mis-parsed.
- **Stable IDs** for sets and patterns; set files are named `<name>-<id>.json`, so two sets with
  the same name no longer silently overwrite each other, and IDs stay stable across re-saves.
- **Load validation & repair** — out-of-range fields (BPM, swing, lengths, MIDI values,
  probability, ratchet) are clamped/repaired on load; a malformed set never panics.
- **Configurable per-lane MIDI routing** — assign each lane an output port, MIDI channel, and
  clock-out via a route editor (`w`); output is delivered only to the assigned destination
  (no broadcast), and MIDI clock is sent once per clock-out port.
- **Debounced autosave** to a separate recovery file (never overwrites a deliberate save).
- **Crash recovery** — after an unclean shutdown, startup offers Recover / Discard / Open saved.

## [0.2.0] — 2026-06-28 — Milestone 1: safety foundations

### Fixed
- A malformed set with `bpm = 0` no longer hangs the scheduler (BPM clamped to a musical range).
- Ableton Link: pressing play no longer fires notes before the quantized bar boundary; the
  transport shows an armed state until the engine confirms it actually started.
- Loading a set no longer splits note timing and MIDI clock onto different BPMs.
- Tap tempo now updates the displayed BPM.
- Auditioning a library pattern no longer mutates your saved pattern or corrupts undo history
  (isolated preview); committing/reverting an audition is correct.
- Undo/redo now also revert tempo and swing; mute/solo/BPM changes are undoable and mark the set dirty.

### Added
- An authoritative active-note registry: every sounding note has an owner and a guaranteed
  release path, so stop, panic, quit, mute/solo, set-load, and device disconnect never leave a
  hung note.
- A device-watcher thread that moves MIDI port enumeration off the timing-critical loop (tighter
  timing) and releases notes when a device disconnects.
- Engine-confirmed transport state, status-line auto-expiry, and global play/stop/panic from every
  screen with a per-mode footer.

### Changed
- Quality gate tightened to `cargo clippy --all-targets --all-features -- -D warnings`.

## [0.1.0] — Baseline

Initial terminal MIDI pattern sequencer for the Roland AIRA Compact **T‑8** (drums + bass) and
**S‑1** (synth): 3-lane groovebox with polymeter, a large built-in pattern library with audible
audition, full step authoring (velocity, notes, slides, per-step probability and ratcheting,
Euclidean generation, copy/paste/rotate), manual tempo / tap / Ableton Link sync, save/load sets,
and an ASCII terminal UI.
