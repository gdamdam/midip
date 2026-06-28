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
▸1 DRUM   techno #03   ●  M– S–  [●···●···●···●···]
 2 BASS   acid #11     ●  M– S–  [●··●····●··●····]
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
  (cue a pattern before committing it).
- **Full step authoring** — toggle hits, velocity, note entry, per‑note length, slides (303/
  SH‑1 style), per‑step **probability** and **ratcheting**, **Euclidean** generation,
  copy/paste/rotate, pattern length 1–64, and global undo/redo.
- **Tempo** — type an exact BPM, nudge it, **tap tempo**, or sync to **Ableton Link**
  (embedded — no separate bridge app). midip is the clock master (24 PPQN).
- **Auto‑detects** your T‑8 / S‑1 by name, with live connection status and basic hot‑plug.
- **Save / load sets** (the whole 3‑lane combination + tempo/swing).
- Tasteful static color, a context‑sensitive footer, and a full `?` controls overlay.

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
   and **↑/↓** to move within a list. Press **`a`** to **audition** the selected pattern (you
   hear it in the focused lane); keep auditioning as you scroll, then **Enter** to keep it or
   **Esc** to revert.
3. Press **space** to play. Set the tempo with **`t`** (type a BPM) or **`T`** (tap), or press
   **`k`** to follow an **Ableton Link** session.
4. Switch lanes with **Tab**, edit the grid, and **`s`** to save the set.

## The interface

- **Transport bar** — play state · BPM · Link (peers + `LOCKED`) · `bar.beat.16th` · swing ·
  `SAVED`/`EDITED`, with a status/toast line beneath it ("Saved", "Loaded dub #07",
  "Velocity 96", "Link lost", …).
- **Lanes** — one row each for `DRUM` / `BASS` / `SYNTH`: focus marker, pattern name,
  connection `●/○`, mute/solo (`M●`/`S●`), and a live activity strip.
- **Editor** — adapts to the focused lane:
  - **Drums**: a TR‑style grid (voice rows × steps); velocity shown as cell shading.
  - **Melodic**: a note lane with pitch names, note length (sustain spans cells), and slides
    drawn as a glide tie between notes.
  - Shows an `EDIT … | Steps x‑y of N | Cursor | Playhead` header and a per‑step detail line.
  - Patterns longer than 16 steps **page** (the view follows the cursor).
- **Library / Open‑set** overlays for browsing patterns and loading saved sets.

## Controls

Press **`?`** in‑app for this list. `[space]` play and `[!]` panic work in every mode.

| Group | Keys |
|------|------|
| **Transport** | `space` play/stop · `esc` panic (transport keeps running) · `!` full MIDI panic · `t` type BPM · `;`/`'` BPM −/+ · `T` tap tempo · `k` toggle Ableton Link · `<`/`>` swing · `{`/`}` pattern length |
| **Edit (both)** | `tab`/`shift+tab` cycle lane · `enter` toggle step / place note · `0–9` velocity bucket · `+`/`-` fine velocity · `p`/`P` probability · `y`/`Y` ratchet · `x`/`c`/`v` cut/copy/paste · `r`/`R` rotate · `del` clear |
| **Drums** | `←`/`→`/`↑`/`↓` move cursor · `e`/`E` euclid pulses · `[`/`]` euclid rotation |
| **Melodic** | `←`/`→` step · `↑`/`↓` pitch · `g` slide · `,`/`.` note length · `[`/`]` octave |
| **Library** | `←`/`→` column · `↑`/`↓` select · `a` audition · `enter` load · `esc`/`l` close |
| **Global** | `ctrl+z`/`u` undo · `ctrl+y` redo · `m` mute · `S` solo · `l` library · `o` open set · `s` save · `?` help · `q` quit (twice while playing) |

## Devices & MIDI

midip auto‑detects output ports by name (`T-8`, `S-1`). The default lane → channel map
(matching the AIRA Compacts):

| Lane | Device | MIDI channel |
|------|--------|--------------|
| DRUM | T‑8 (drum part) | 10 |
| BASS | T‑8 (bass part) | 2 |
| SYNTH | S‑1 | 1 |

The two T‑8 lanes share one physical connection (distinguished by channel). midip sends
**MIDI Clock** (so the devices' delays/arps follow its tempo) but **not** transport
Start/Stop — so your gear plays *only* the notes midip sends, never its own stored pattern.
A failed send or unplugged device flips that lane to `○`; replugging reconnects automatically.

## Tempo & Ableton Link

- **Manual**: `t` to type an exact BPM (20–300), `;`/`'` to nudge ±1, `T` to tap.
- **Ableton Link**: `k` toggles Link. When enabled, midip phase‑locks to the session tempo and
  shows `LINK <peers> LOCKED`. Link is embedded directly (via `rusty_link`) — no companion app.

## Patterns, library & sets

- The library lives in `assets/patterns/` (`patterns-t8-drums.json`, `patterns-t8-bass.json`,
  `patterns-s1.json`, `catalog.json`) — a **read‑only vendored snapshot** of the mpump set,
  never modified at runtime. Genres are listed alphabetically; each pattern has a name and
  description from the catalog.
- **Audition** (`a`) previews without committing; **Enter** keeps, **Esc** reverts.
- **Save** (`s`) writes the current set; **open set** (`o`) browses and loads saved sets.
  Saved sets live in the data dir (see below).

## Configuration

Environment variables (all optional):

| Variable | Effect |
|----------|--------|
| `MIDIP_DATA` | Directory for saved sets (default: `<exe-dir>/data`, dev fallback `./data`). |
| `MIDIP_ASSETS` | Directory of the vendored pattern library (default: `<exe-dir>/assets/patterns`, dev fallback `./assets/patterns`). |
| `MIDIP_ASCII` | Set to `1`/`true` to use ASCII glyphs instead of Unicode (for limited terminals). |

## Project layout

```
src/
  main.rs            entry, terminal lifecycle, event loop
  app.rs             App state + Action reducer (edits, undo, library, audition…)
  input.rs           key → Action mapping
  pattern/           model · library loader · store (save/load) · euclid
  devices/           T‑8 / S‑1 profiles (channels, drum voices, pitch/velocity)
  midi/              MidiMessage · MidiSink (RecordingSink / MidirSink / NullSink)
  engine/            scheduler (timing/swing/slide/ratchet) · clock · transport · thread
  link/              embedded Ableton Link (rusty_link) + a test fake
  ui/                transport · lanes · editor_drums · editor_melodic · library · help · theme
assets/patterns/     vendored mpump pattern library (read-only)
docs/                design spec, implementation plan, KNOWN-ISSUES
```

## Testing

```sh
cargo test
```

The engine writes through a `MidiSink` trait, so playback, scheduling, slides, probability,
ratcheting, polymeter, and the reducer are all tested with a recording sink — **no hardware
needed**. UI views are checked with ratatui's `TestBackend`. (Live MIDI and Ableton Link are
hardware paths, verified by inspection + manual acceptance.)

## Status

Feature‑complete and green; the remaining checks are a manual hardware pass on real gear.
See [`docs/KNOWN-ISSUES.md`](docs/KNOWN-ISSUES.md) for the current status and minor open items,
and [`docs/superpowers/specs/`](docs/superpowers/specs/) for the design.

## License

MIT.
