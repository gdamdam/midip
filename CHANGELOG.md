# Changelog

All notable changes to midip are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/) (pre‑1.0: each
feature milestone is a minor bump).

## [Unreleased]

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
