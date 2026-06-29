# midip

A **terminal MIDI pattern sequencer** for the Roland AIRA Compact **T‑8** (drums + bass)
and **S‑1** (synth). Browse a large built‑in pattern library, play it to your hardware as
a 3‑lane groovebox, edit patterns live, and stay in time manually or via **Ableton Link** —
all from an ASCII UI in your terminal.

midip is **MIDI‑only**: it makes *your* devices play the notes. It is the sequencer; your
gear is the sound. (It never triggers a device's own internal pattern — see
[Devices & MIDI](#devices--midi).)

```
▶ PLAY  124 BPM  LINK 2 LOCKED  001.2.3  SW 56%  SAVED
▸1 DRUM   techno #03   ●  M– S–  [●···●···●···●···]        ACTIVE
 2 BASS   acid #11     ●  M– S–  [●··●····●··●····]        QUEUED⟶
 3 SYNTH  dub #07      ○  M– S–  [··●····●····●···]
EDIT DRUM | Steps 1-16 of 16 | Cursor 1 | Playhead 4
 ... step grid ...
Step 1 · BD · Velocity 120 [-/+] · Probability 100% [p/P] · Ratchet x1 [y/Y]
[space]play [tab]lane [arrows]move [enter]toggle [0-9]vel [?]controls
```

## Features

- **3‑lane groovebox** — T‑8 drums + T‑8 bass + S‑1 synth play together, each with its own
  pattern, with **polymeter** (lanes can have different lengths and drift in and out of phase).
- **Built‑in library** — hundreds of named patterns across 20 genres (a vendored snapshot of
  the [mpump](https://github.com/gdamdam) pattern set), browsable with **audible audition**
  (cue a pattern before committing it — gated to stopped or muted lanes so it never collides
  with a live lane).
- **Full step authoring** — toggle hits, velocity, note entry, per‑note length, slides (303/
  SH‑1 style), per‑step **probability** and **ratcheting**, **Euclidean** generation,
  copy/paste/rotate, pattern length 1–64, **double‑length** (`L`), and global undo/redo.
- **Quantized pattern launching** — loading a pattern while playing queues it and launches it
  exactly at the next bar or next beat (toggle with `b`), restarting the lane at step 1 without
  disrupting the others. Lanes show `ACTIVE` and `QUEUED⟶` markers; `C` cancels a queue.
- **Configurable per‑lane MIDI routing** — assign each lane its own output port, MIDI channel,
  and clock‑out flag via the route editor (`w`).
- **Virtual mirror output** — `M` toggles an optional virtual MIDI source named `midip` that
  other apps on the same machine can subscribe to; shows `MIR` when active. Purely additive —
  hardware output is identical with or without it.
- **Tempo** — type an exact BPM, nudge it, **tap tempo**, or sync to **Ableton Link**
  (embedded — no separate bridge app). Link bar‑locks playback start so no notes fire before
  the bar boundary. midip is the clock master (24 PPQN).
- **Auto‑detects** your T‑8 / S‑1 by name, with live connection status and basic hot‑plug.
- **Persistence** — versioned + atomic saves, stable set IDs, **autosave + crash recovery**
  (startup offers Recover / Discard / Open on an unclean shutdown). Full set management:
  save‑as, rename, duplicate, new, and delete. Save the focused lane as a user pattern (`A`),
  clear it (`Z`), and load user patterns from a "User" section in the library.
- Tasteful static color, a context‑sensitive footer, and a full scrollable `?` controls overlay.

## Requirements

- **Rust** (stable, 2021 edition) — install via [rustup](https://rustup.rs).
- A **terminal** at least **60×16** (it shows a resize hint if smaller).
- **MIDI**: macOS (CoreMIDI) or any platform [`midir`](https://crates.io/crates/midir)
  supports. The first build downloads crates from crates.io.
- **Hardware** is optional — without a T‑8/S‑1 connected, midip still runs (silently) so you
  can browse and edit.

## Build & run

```sh
git clone <this-repo> midip && cd midip
cargo run --release          # launches the terminal UI
```

Other commands:

```sh
cargo build --release        # just build the binary (target/release/midip)
cargo test                   # run the test suite
```

> Run it in a real terminal (not piped) — it takes over the screen while running and restores
> it on exit. Press `?` any time for the full control list, `q` (twice while playing) to quit.

## Quick start

1. Plug in your **T‑8** and/or **S‑1** over USB and start midip — connected lanes show `●`.
2. Press **`l`** to open the **library**. Use **←/→** to switch the genre / pattern columns
   and **↑/↓** to move within a list. Press **`a`** to **audition** the selected pattern (only
   available when the focused lane is stopped or muted); keep auditioning as you scroll, then
   **Enter** to keep it or **Esc** to revert.
3. Press **space** to play. Set the tempo with **`t`** (type a BPM) or **`T`** (tap), or press
   **`k`** to follow an **Ableton Link** session.
4. While playing, press **`l`** and hit **Enter** on a new pattern — it queues and launches on
   the next bar without stopping the other lanes.
5. Switch lanes with **Tab**, edit the grid, and **`s`** to save the set.

## The interface

- **Transport bar** — play state · BPM · Link (peers + `LOCKED`) · `bar.beat.16th` · swing ·
  `SAVED`/`EDITED`, with a status/toast line beneath it ("Saved", "Loaded dub #07",
  "Velocity 96", "Link lost", …).
- **Lanes** — one row each for `DRUM` / `BASS` / `SYNTH`: focus marker, pattern name,
  connection `●/○`, mute/solo (`M●`/`S●`), mirror indicator (`MIR`), a live activity strip,
  and `ACTIVE` / `QUEUED⟶` launch markers when queuing is in play.
- **Editor** — adapts to the focused lane:
  - **Drums**: a TR‑style grid (voice rows × steps); velocity shown as cell shading.
  - **Melodic**: a note lane with pitch names, note length (sustain spans cells), and slides
    drawn as a glide tie between notes.
  - Shows an `EDIT … | Steps x‑y of N | Cursor | Playhead` header and a per‑step detail line.
  - Patterns longer than 16 steps **page** (the view follows the cursor).
- **Library** overlay — genre column + pattern column; `a` to audition, Enter to commit.
- **Set manager** overlay (`o`) — load, save‑as, rename, duplicate, new, delete.
- **Route editor** overlay (`w`) — per‑lane port / channel / clock‑out assignment.

## Controls

Press **`?`** in‑app for the full scrollable list. `space` and `!` work in every mode.

### Transport

| Key | Action |
|-----|--------|
| `space` | Play / stop |
| `esc` | Panic — all notes off (transport keeps running) |
| `!` | Full MIDI panic |
| `t` | Type BPM (Enter confirm, Esc cancel) |
| `;` / `'` | BPM −1 / +1 |
| `T` | Tap tempo |
| `k` | Toggle Ableton Link |
| `<` / `>` | Swing − / + |
| `{` / `}` | Pattern length − / + |
| `L` | Double length (repeats content, max 64) |

### Edit (both lane types)

| Key | Action |
|-----|--------|
| `tab` / `shift+tab` | Cycle lane focus next / prev |
| `enter` | Toggle step (Drums) / place note (Melodic) |
| `0–9` | Velocity bucket |
| `+` / `-` | Fine velocity |
| `p` / `P` | Step probability up / down |
| `y` / `Y` | Ratchet up / down |
| `x` `c` `v` | Cut / copy / paste |
| `r` / `R` | Rotate |
| `del` | Clear step |

### Drums

| Key | Action |
|-----|--------|
| `←` `→` `↑` `↓` | Move cursor |
| `e` / `E` | Euclidean pulses add / remove |
| `[` / `]` | Euclidean rotation |

### Melodic

| Key | Action |
|-----|--------|
| `←` / `→` | Step cursor |
| `↑` / `↓` | Pitch up / down |
| `g` | Toggle slide |
| `,` / `.` | Note length |
| `[` / `]` | Octave down / up |

### Library (`l` to open)

| Key | Action |
|-----|--------|
| `←` / `→` | Switch column (genre / pattern) |
| `↑` / `↓` | Select |
| `a` | Audition (preview; lane must be stopped or muted) |
| `enter` | Commit pattern (queues at next bar/beat when playing) |
| `b` | Toggle launch quantization: next bar / next beat |
| `C` | Cancel pending queued launch |
| `esc` / `l` | Close library |

### Set Manager (`o` to open)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select set |
| `enter` | Load set |
| `r` | Rename set |
| `a` / `S` | Save as new |
| `D` | Duplicate |
| `d` | Delete (with confirmation) |
| `n` | New set (confirms if unsaved) |
| `esc` / `o` | Close |

### Route Editor (`w` to open)

| Key | Action |
|-----|--------|
| `↑` / `↓` | Select lane |
| `←` / `→` | Move between fields (Port / Channel / Clock-out) |
| `c` / `C` | Cycle port forward / backward |
| `[` / `]` | Channel −1 / +1 (1‑based, range 1–16) |
| `z` | Toggle MIDI clock output on/off for the lane |
| `esc` | Close route editor |

### Global

| Key | Action |
|-----|--------|
| `ctrl+z` / `u` | Undo |
| `ctrl+y` | Redo |
| `m` | Mute focused lane |
| `S` | Solo focused lane |
| `M` | Toggle virtual mirror output |
| `A` | Save focused lane as user pattern |
| `Z` | Clear focused lane pattern (with confirmation) |
| `b` | Toggle launch quantization: next bar / next beat |
| `C` | Cancel pending queued launch |
| `w` | Open route editor |
| `l` | Open library |
| `o` | Open set manager |
| `s` | Save set |
| `?` | Help overlay |
| `q` | Quit (press twice while playing) |

## Devices & MIDI

midip auto‑detects output ports by name (`T-8`, `S-1`). The default lane → channel map
(matching the AIRA Compacts):

| Lane | Device | MIDI channel |
|------|--------|--------------|
| DRUM | T‑8 (drum part) | 10 |
| BASS | T‑8 (bass part) | 2 |
| SYNTH | S‑1 | 1 |

The two T‑8 lanes share one physical connection (distinguished by channel). You can reassign
any lane's port, channel, and clock‑out in the **route editor** (`w`).

midip sends **MIDI Clock** (24 PPQN, so the devices' delays/arps follow its tempo) but **not**
transport Start/Stop — so your gear plays *only* the notes midip sends, never its own stored
pattern. A failed send or unplugged device flips that lane to `○`; replugging reconnects
automatically.

### Virtual mirror output

`M` creates an optional virtual MIDI source named **`midip`** (macOS / Linux) that other apps
on the same machine can subscribe to. When `MIR` is shown, the full output stream (all lanes'
notes + 24 PPQN clock) is also sent to this virtual port. The mirror is purely additive:
hardware output is identical whether it is on or off.

## Tempo & Ableton Link

- **Manual**: `t` to type an exact BPM (20–300), `;`/`'` to nudge ±1, `T` to tap.
- **Ableton Link**: `k` toggles Link. When enabled, midip phase‑locks to the session tempo and
  shows `LINK <peers> LOCKED`. Playback start is **bar‑locked**: pressing play arms the engine
  and the first note fires only at the next bar boundary — no early notes. Link is embedded
  directly (via `rusty_link`) — no companion app required.

## Patterns, library & sets

- The library lives in `assets/patterns/` (`patterns-t8-drums.json`, `patterns-t8-bass.json`,
  `patterns-s1.json`, `catalog.json`) — a **read‑only vendored snapshot** of the mpump set,
  never modified at runtime. Genres are listed alphabetically; each pattern has a name and
  description from the catalog.
- **Audition** (`a`) previews a library pattern without committing (only when the lane is
  stopped or muted); focus change or Esc reverts. **Enter** commits.
- **Quantized launch**: committing a pattern while playing queues it (`QUEUED⟶`) for the next
  bar or beat (toggle `b`). `C` cancels.
- **User patterns**: `A` saves the focused lane as a named user pattern; `Z` clears it. User
  patterns appear in the library under a "User" section and can be renamed, duplicated, or
  deleted from there.
- **Sets** hold all three lanes + tempo/swing. Set files are named `<name>-<id>.json` (stable
  IDs prevent silent overwrites). The format is versioned; old files upgrade automatically; a
  file from a newer midip is rejected cleanly.
- **Autosave** writes a recovery file in the background (never overwrites a deliberate save).
  On an **unclean shutdown**, startup prompts **Recover / Discard / Open** saved.
- **Set management** (`o`): save‑as, rename, duplicate, new, and delete with confirmation.

## Configuration

Environment variables (all optional):

| Variable | Effect |
|----------|--------|
| `MIDIP_DATA` | Directory for saved sets and user patterns (default: `<exe-dir>/data`, dev fallback `./data`). |
| `MIDIP_ASSETS` | Directory of the vendored pattern library (default: `<exe-dir>/assets/patterns`, dev fallback `./assets/patterns`). |
| `MIDIP_ASCII` | Set to `1`/`true` to use ASCII glyphs instead of Unicode (for limited terminals). |

## Project layout

```
src/
  main.rs            entry, terminal lifecycle, event loop
  app.rs             App state + Action reducer (edits, undo, library, audition…)
  input.rs           key → Action mapping
  config.rs          env-var configuration
  pattern/           model · library loader · store (save/load/user patterns) · euclid
  devices/           T‑8 / S‑1 profiles (channels, drum voices, pitch/velocity)
  midi/              MidiMessage · MidiSink (RecordingSink / MidirSink / NullSink)
  engine/            scheduler (timing/swing/slide/ratchet) · clock · transport · thread
  link/              embedded Ableton Link (rusty_link) + a test fake
  ui/                transport · lanes · editor_drums · editor_melodic · library · help · theme
assets/patterns/     vendored mpump pattern library (read-only)
docs/                KNOWN-ISSUES · HARDWARE-ACCEPTANCE · design specs
```

## Testing

```sh
cargo test
```

The engine writes through a `MidiSink` trait, so playback, scheduling, slides, probability,
ratcheting, polymeter, quantized launch, and the reducer are all tested with a recording sink —
**no hardware needed**. UI views are checked with ratatui's `TestBackend`. (Live MIDI and
Ableton Link are hardware paths — see [`docs/HARDWARE-ACCEPTANCE.md`](docs/HARDWARE-ACCEPTANCE.md).)

## Status

v0.5.0 — feature‑complete and green. See [`docs/KNOWN-ISSUES.md`](docs/KNOWN-ISSUES.md) for
minor open items, and [`docs/superpowers/specs/`](docs/superpowers/specs/) for the design.

## License

midip is licensed under the **GNU Affero General Public License v3.0 or later**
(AGPL-3.0-or-later). See [`LICENSE`](LICENSE).
