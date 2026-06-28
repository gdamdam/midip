# midip

Terminal (ratatui) multitrack MIDI pattern sequencer. Plays and edits the vendored
`mpump` pattern library across Roland AIRA Compact T-8 (drums + bass) and S-1 (synth),
with manual tempo, tap tempo, and embedded Ableton Link sync.

MIDI-only (no built-in synthesis). The pattern library in `assets/patterns/` is a
read-only vendored snapshot of mpump's data and is never modified at runtime.

## Build & test

```sh
cargo build
cargo test
```

## Run

```sh
cargo run --release
```
