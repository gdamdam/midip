# Changelog

All notable changes to midip are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/) (pre‑1.0: each
feature milestone is a minor bump).

## [Unreleased]

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
