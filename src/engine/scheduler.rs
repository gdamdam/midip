//! Step sequencer: pure timing math (this task) plus the stateful `Sequencer`
//! (Task 9). All time is `u64` microseconds on a monotonic timeline.

use crate::midi::ports::MidiSink;
use crate::midi::MidiMessage;
#[cfg(test)]
use crate::pattern::model::TrigCond;
use crate::pattern::model::{CcLock, Lane, MelodicNote, Pattern, PatternData, Set};
use std::collections::HashMap;

/// Ownership domain of a sounding note (design §3.1). M1 only produces Playback;
/// the field + release_domain exist so M3 audition / M15 preview can reuse the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteDomain {
    Playback,
    Audition,
    Preview,
    Performance,
}

/// Quantization grid for a queued per-lane launch (design §1: launch grid = the
/// GLOBAL 4/4 bar of 16 sixteenth steps). `NextBar` launches on a bar boundary
/// (absolute step % 16 == 0); `NextBeat` on a beat boundary (% 4 == 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quant {
    NextBar,
    NextBeat,
}

/// True when absolute `step` lands on quant `q`'s grid. Both grids are true at
/// step 0 (a queue placed before play would launch on the very first step — fine).
pub fn is_boundary(step: usize, q: Quant) -> bool {
    match q {
        Quant::NextBar => step.is_multiple_of(16),
        Quant::NextBeat => step.is_multiple_of(4),
    }
}

/// Per-lane performance state applied AT a launch instant (M6 scene recall).
/// Carried alongside a queued launch so mute/solo/transpose/octave switch on the
/// SAME boundary the pattern does — not before (the outgoing scene keeps sounding
/// until the boundary). `None` on a per-lane queue means "leave the lane's current
/// mute/solo/transpose/octave unchanged" (the M3 single-pattern launch path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaunchState {
    pub mute: bool,
    pub solo: bool,
    pub transpose: i8,
    pub octave: i8,
}

/// A note currently sounding, tracked by the authoritative registry.
#[derive(Clone, Debug)]
pub struct SoundingNote {
    pub channel: u8,
    pub note: u8,
    pub lane: usize,
    pub domain: NoteDomain,
}

/// Pulses Per Quarter Note for the MIDI clock.
pub const PPQN: u64 = 24;

/// Minimum musical BPM accepted by the scheduler. Prevents a zero/negative BPM
/// from producing a zero or near-infinite step duration that hangs the tick loop.
pub const MIN_BPM: f64 = 20.0;

/// Maximum musical BPM accepted by the scheduler. Clamps absurdly high BPM
/// values to a sane ceiling that keeps step durations positive.
pub const MAX_BPM: f64 = 300.0;

/// Duration of one 16th-note step in microseconds at `bpm`.
/// A quarter note is `60_000_000 / bpm` µs; a 16th is a quarter of that.
/// `bpm` is clamped to `MIN_BPM..=MAX_BPM` so the result is always a finite,
/// positive µs value regardless of what a loaded set contains.
pub fn step_dur_micros(bpm: f64) -> u64 {
    let bpm = if bpm.is_finite() {
        bpm.clamp(MIN_BPM, MAX_BPM)
    } else {
        MIN_BPM
    };
    (60_000_000.0 / (bpm * 4.0)).round() as u64
}

/// Swing offset for `step_index` given a `swing` ratio (0.5 = straight) and the
/// straight step duration. Even (down-beat) steps are unshifted; odd (off-beat)
/// steps are delayed by `(swing - 0.5) * step_dur * 2`. Signed so off-steps can be
/// pulled earlier if `swing < 0.5`.
pub fn swing_offset_micros(step_index: usize, swing: f32, step_dur: u64) -> i64 {
    if step_index.is_multiple_of(2) {
        0
    } else {
        ((swing as f64 - 0.5) * step_dur as f64 * 2.0).round() as i64
    }
}

/// Note length in microseconds: `round(len * step_dur)`.
pub fn note_len_micros(len: f32, step_dur: u64) -> u64 {
    (len as f64 * step_dur as f64).round() as u64
}

/// A MIDI message scheduled for a specific lane at a specific monotonic time.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledEvent {
    pub at_micros: u64,
    pub lane: usize,
    pub msg: MidiMessage,
}

/// Delegate to the canonical implementation in `crate::link`.
fn step_from_beat(beat: f64) -> usize {
    crate::link::step_from_beat(beat)
}

/// A melodic note currently sounding on a lane, tracked for slide/legato and stop.
#[derive(Clone, Debug)]
struct ActiveNote {
    note: u8,
    channel: u8,
    /// Scheduled NoteOff time. `None` means "held" (a following slide note suppressed
    /// the release until the next NoteOn schedules it).
    off_at: Option<u64>,
}

/// Stateful step sequencer. Owns the `Set`, a monotonic step clock, a per-melodic
/// lane active-note tracker, and a time-ordered queue of pending events.
pub struct Sequencer {
    set: Set,
    playing: bool,
    /// Monotonic time at which step 0 began (the play origin).
    origin_micros: u64,
    /// The next step index that has not yet been materialized into the queue.
    next_step: usize,
    /// Fix #5 — accumulated schedule.
    /// `last_step_at` is the absolute time of the most recently fired step, or
    /// `None` before the first step fires. The NEXT step is due at
    /// `last_step_at + current_step_dur`; when `None`, step 0 is due at `origin_micros`.
    /// Storing the fire time (not the next-due time) means a bpm change between
    /// ticks shifts only future intervals without touching already-fired steps.
    last_step_at: Option<u64>,
    /// Time-ordered (ascending `at_micros`) pending events.
    queue: Vec<ScheduledEvent>,
    /// One active melodic note per lane (index parallels `set.lanes`).
    active: Vec<Option<ActiveNote>>,
    /// Externally-set current step (used by `sync_to_beat` / `current_step`).
    current: usize,
    /// Deterministic xorshift64 PRNG state for per-step probability rolls.
    rng: u64,
    /// Authoritative registry of every currently-sounding note with its owner.
    /// Updated at emit time: NoteOn inserts, NoteOff removes by (channel, note).
    pub sounding: Vec<SoundingNote>,
    /// M3 launch queue — one slot per lane (index parallels `set.lanes`). A queued
    /// `(Pattern, Quant, Option<LaunchState>)` is applied at the next matching global
    /// boundary, exactly once. Replacing a queued launch overwrites the slot (no stacking).
    ///
    /// M6: the optional `LaunchState` carries the per-lane mute/solo/transpose/octave to
    /// apply AT the launch instant (scene recall). `None` = leave the lane's current
    /// performance state untouched (the M3 single-pattern launch). Queuing every lane with
    /// the SAME `Quant` makes `apply_due_launches` fire them all on ONE boundary step.
    queued: Vec<Option<(Pattern, Quant, Option<LaunchState>)>>,
    /// Absolute step at which each lane last (re)started its local clock. 0 for a lane
    /// that has never been launched, so `(step - 0) % len` is byte-identical to today's
    /// `step % len`. A launch sets this to the boundary step, restarting the lane at local 0.
    launch_offset: Vec<usize>,
    /// Scratch: lanes that launched during the most recent `tick`. Drained by
    /// `take_launched` so the engine can emit one `Launched` event per lane.
    just_launched: Vec<usize>,
    /// Per-route CC cache: suppresses redundant CC resends within a play session.
    /// Key: `(port_stable_key, channel, cc_number)`. Value: last sent value.
    /// Cleared on Stop, panic, route change, and Set-swap (the latter replaces the
    /// whole `Sequencer`, so the new instance starts with an empty map).
    cc_cache: HashMap<(String, u8, u8), u8>,
    /// Latched fill-active flag. Read by the trig-condition gate for Fill/NotFill
    /// conditions. Separate from ToggleFill (a pattern transform) — this is a
    /// momentary performance state toggled live via SetFillActive.
    fill_active: bool,
    /// M10 T4: when true (external MIDI clock followed), `tick()`'s internal
    /// `step_dur`-based advance loop is SUPPRESSED — steps advance ONLY via
    /// `advance_clock_step` (called once per 6 external ticks). `tick()` still runs its
    /// inaudible-note release pass and flushes due NoteOffs (note-safety/output), so
    /// disabling the advance loop never strands a note. This guarantees no-double-advance:
    /// while clock-driven, exactly ONE path moves `current`.
    clock_driven: bool,
}

/// Default PRNG seed (a fixed nonzero constant so playback is reproducible).
const DEFAULT_SEED: u64 = 0x2545F4914F6CDD1D;

impl Sequencer {
    pub fn new(set: Set) -> Sequencer {
        let n = set.lanes.len();
        Sequencer {
            set,
            playing: false,
            origin_micros: 0,
            next_step: 0,
            last_step_at: None,
            queue: Vec::new(),
            active: vec![None; n],
            current: 0,
            rng: DEFAULT_SEED,
            sounding: Vec::new(),
            queued: vec![None; n],
            launch_offset: vec![0; n],
            just_launched: Vec::new(),
            cc_cache: HashMap::new(),
            fill_active: false,
            clock_driven: false,
        }
    }

    /// Reseed the per-step probability PRNG (tests use this for determinism).
    /// A zero seed is mapped to the default to keep xorshift64 nondegenerate.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = if seed == 0 { DEFAULT_SEED } else { seed };
    }

    /// Next xorshift64 value mapped into [0.0, 1.0).
    fn next_unit(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        // Use the top 24 bits for a uniform float in [0, 1).
        ((x >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// Roll the PRNG against `prob`. `prob >= 1.0` always fires (no roll consumed);
    /// `prob <= 0.0` never fires (no roll consumed). Otherwise consume one roll and
    /// fire iff `roll < prob`.
    fn rolls_fire(&mut self, prob: f32) -> bool {
        if prob >= 1.0 {
            return true;
        }
        if prob <= 0.0 {
            return false;
        }
        self.next_unit() < prob
    }

    /// Route ALL NoteOn/NoteOff emission through this single helper.
    /// Updates the sounding registry then forwards to `sink.send` unchanged.
    /// - NoteOn: inserts into registry (replaces any existing (ch,note) — retrigger).
    /// - NoteOff: removes matching (ch,note) from registry.
    /// - Other messages: passed through unmodified, registry unchanged.
    fn emit(&mut self, msg: MidiMessage, lane: usize, at_micros: u64, sink: &mut dyn MidiSink) {
        match msg {
            MidiMessage::NoteOn { channel, note, .. } => {
                // Retrigger: replace any prior entry for (channel, note).
                self.sounding
                    .retain(|s| !(s.channel == channel && s.note == note));
                self.sounding.push(SoundingNote {
                    channel,
                    note,
                    lane,
                    domain: NoteDomain::Playback,
                });
            }
            MidiMessage::NoteOff { channel, note } => {
                self.sounding
                    .retain(|s| !(s.channel == channel && s.note == note));
            }
            _ => {}
        }
        // Per-lane routing: deliver to the EMITTING lane's port, so two lanes sharing a MIDI
        // channel on different ports stay independent (PortFanoutSink::send_lane).
        sink.send_lane(msg, lane, at_micros);
    }

    /// Release every currently-sounding note, then send CC123 + CC120 per distinct
    /// channel, and clear the registry (and the legato `active` tracker).
    /// This is the P4 fix: call BEFORE clearing `queue` so queued NoteOffs for
    /// already-flushed NoteOns don't get dropped silently.
    pub fn release_all(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // NoteOff for every sounding note.
        let sounding = std::mem::take(&mut self.sounding);
        for s in &sounding {
            sink.send_lane(
                MidiMessage::NoteOff {
                    channel: s.channel,
                    note: s.note,
                },
                s.lane,
                at_micros,
            );
        }
        // CC123 (All Notes Off) + CC120 (All Sound Off) on each lane's ROUTE channel, routed
        // to that lane's port. Per-lane (NOT deduped by channel): two lanes sharing a channel
        // on DIFFERENT ports must each be swept; if they share a port the extra send is an
        // idempotent all-notes-off, so it is harmless.
        for (lane_idx, lane) in self.set.lanes.iter().enumerate() {
            let ch = lane.route_channel();
            sink.send_lane(
                MidiMessage::ControlChange {
                    channel: ch,
                    controller: 123,
                    value: 0,
                },
                lane_idx,
                at_micros,
            );
            sink.send_lane(
                MidiMessage::ControlChange {
                    channel: ch,
                    controller: 120,
                    value: 0,
                },
                lane_idx,
                at_micros,
            );
        }
        // Clear the legato active tracker too.
        for slot in self.active.iter_mut() {
            *slot = None;
        }
        // sounding was already cleared by take().
    }

    /// Release only notes belonging to domain `d`.
    pub fn release_domain(&mut self, d: NoteDomain, at_micros: u64, sink: &mut dyn MidiSink) {
        let mut remaining = Vec::with_capacity(self.sounding.len());
        for s in std::mem::take(&mut self.sounding) {
            if s.domain == d {
                sink.send_lane(
                    MidiMessage::NoteOff {
                        channel: s.channel,
                        note: s.note,
                    },
                    s.lane,
                    at_micros,
                );
            } else {
                remaining.push(s);
            }
        }
        self.sounding = remaining;
    }

    /// Release only notes on the given lane indices. Used by route/disconnect (Task 5).
    pub fn release_lanes(&mut self, lanes: &[usize], at_micros: u64, sink: &mut dyn MidiSink) {
        let mut remaining = Vec::with_capacity(self.sounding.len());
        for s in std::mem::take(&mut self.sounding) {
            if lanes.contains(&s.lane) {
                sink.send_lane(
                    MidiMessage::NoteOff {
                        channel: s.channel,
                        note: s.note,
                    },
                    s.lane,
                    at_micros,
                );
                // Also clear the legato active slot for this lane.
                if let Some(slot) = self.active.get_mut(s.lane) {
                    *slot = None;
                }
            } else {
                remaining.push(s);
            }
        }
        self.sounding = remaining;
    }

    /// Number of currently-sounding notes across all domains. Test hook.
    pub fn sounding_count(&self) -> usize {
        self.sounding.len()
    }

    /// Release a single (channel, note) from the sounding registry, sending a NoteOff.
    /// No-op if the note is not currently sounding. Used by `set_voice_mute`.
    fn release_note(
        sounding: &mut Vec<SoundingNote>,
        channel: u8,
        note: u8,
        lane: usize,
        at: u64,
        sink: &mut dyn MidiSink,
    ) {
        let before = sounding.len();
        sounding.retain(|s| !(s.channel == channel && s.note == note));
        if sounding.len() < before {
            sink.send_lane(MidiMessage::NoteOff { channel, note }, lane, at);
        }
    }

    /// Latch or unlatch a per-voice mute on `lane`. When muting (`on=true`), any
    /// currently-sounding instance of `note` on that lane is immediately released.
    /// Unmuting (`on=false`) is silent — future steps will fire normally.
    pub fn set_voice_mute(
        &mut self,
        lane: usize,
        note: u8,
        on: bool,
        at: u64,
        sink: &mut dyn MidiSink,
    ) {
        if let Some(l) = self.set.lanes.get_mut(lane) {
            if on {
                if !l.muted_voices.contains(&note) {
                    l.muted_voices.push(note);
                }
            } else {
                l.muted_voices.retain(|&n| n != note);
            }
        }
        if on {
            // Release the note if it is currently sounding on this lane.
            let channel = self
                .set
                .lanes
                .get(lane)
                .map(|l| l.route_channel())
                .unwrap_or(0);
            Self::release_note(&mut self.sounding, channel, note, lane, at, sink);
        }
    }

    pub fn play(&mut self, at_micros: u64) {
        self.playing = true;
        self.origin_micros = at_micros;
        self.current = 0;
        self.next_step = 0;
        // Fix #5: reset the accumulated clock. `None` signals that no step has
        // fired yet; the tick loop treats this as "step 0 is due at origin_micros".
        self.last_step_at = None;
        self.queue.clear();
        self.sounding.clear();
        for a in self.active.iter_mut() {
            *a = None;
        }
        // M3: a fresh play restarts every lane's local clock at global 0 (offset 0)
        // and discards any pending launches from a previous run.
        for off in self.launch_offset.iter_mut() {
            *off = 0;
        }
        for q in self.queued.iter_mut() {
            *q = None;
        }
        self.just_launched.clear();
        // Step 0 (and all other steps) are materialized by tick() once
        // now_micros >= step_start. This avoids double-emit when the first
        // tick lands exactly on the origin (step_start == now_micros).
    }

    /// M10 T4: resume playback at the CURRENT position without rewinding (MIDI Continue).
    /// Unlike `play`, this does NOT reset `current`/`next_step` or clear lane state — it only
    /// marks the sequencer playing again and re-anchors the accumulated clock to `at_micros`
    /// so a subsequent internal-timing tick schedules from here (no catch-up burst). No-op if
    /// already playing.
    pub fn resume(&mut self, at_micros: u64) {
        if self.playing {
            return;
        }
        self.playing = true;
        self.last_step_at = Some(at_micros);
    }

    /// Halt the sequencer, releasing every sounding note (including drums whose
    /// NoteOn was flushed but NoteOff is still queued — P4 fix), then halting.
    /// `release_all` is called BEFORE `queue.clear()` so no queued NoteOff drops.
    pub fn stop(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // P4 fix: release via the authoritative sounding registry first,
        // then clear the queue (so a flushed drum NoteOn + queued NoteOff
        // releases cleanly instead of hanging).
        self.release_all(at_micros, sink);
        self.playing = false;
        self.queue.clear();
        // M3: stopping discards any pending launches (they were relative to this run).
        for q in self.queued.iter_mut() {
            *q = None;
        }
        // Clear CC cache so the next play re-sends all CC locks unconditionally.
        self.cc_cache.clear();
    }

    /// All-notes-off / all-sound-off live recovery. Releases every sounding note via
    /// the authoritative registry (all domains), sends CC 123 + CC 120 per distinct
    /// lane channel, and clears the registry. Does NOT change `playing`.
    pub fn panic(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // release_all handles NoteOff for every sounding note + CC123/120 per channel
        // + clears active[]. playing is intentionally left unchanged.
        self.release_all(at_micros, sink);
        // Note: `playing` is intentionally left unchanged.
        // Clear CC cache: a panic resets hardware controller state, so the cache must
        // not suppress a re-send of any CC on the next played step.
        self.cc_cache.clear();
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The ABSOLUTE 16th-step counter since play (monotonic, never wrapped).
    pub fn current_step(&self) -> usize {
        self.current
    }

    /// Lane `idx`'s LOCAL step within its own pattern length. A length of 0 is treated
    /// as 1 so each lane wraps independently (polymeter) without a divide-by-zero.
    ///
    /// M3: the lane's local clock is relative to `launch_offset[idx]` — the absolute
    /// step at which it last (re)started. For a never-launched lane the offset is 0, so
    /// `(current - 0) % len` reduces exactly to the pre-M3 `current % len`.
    pub fn lane_step(&self, idx: usize) -> usize {
        let len = self
            .set
            .lanes
            .get(idx)
            .map(|l| l.pattern.length.max(1))
            .unwrap_or(1);
        let off = self.launch_offset.get(idx).copied().unwrap_or(0);
        self.current.wrapping_sub(off) % len
    }

    /// M3: queue `pattern` to launch on lane `lane` at the next `q` boundary. Replaces
    /// any existing queued launch for that lane (no stacking). No-op if `lane` is out of range.
    /// Leaves the lane's mute/solo/transpose/octave unchanged at the launch (state `None`).
    pub fn queue_launch(&mut self, lane: usize, pattern: Pattern, q: Quant) {
        if let Some(slot) = self.queued.get_mut(lane) {
            *slot = Some((pattern, q, None));
        }
    }

    /// M6: like `queue_launch`, but ALSO applies `state` (mute/solo/transpose/octave) to the
    /// lane AT the launch instant — used by scene recall so every lane's performance state
    /// switches on the SAME boundary as its pattern, not before. Queue every scene lane with
    /// the SAME `q` and they all fire together on one boundary (see `apply_due_launches`).
    pub fn queue_launch_with_state(
        &mut self,
        lane: usize,
        pattern: Pattern,
        q: Quant,
        state: LaunchState,
    ) {
        if let Some(slot) = self.queued.get_mut(lane) {
            *slot = Some((pattern, q, Some(state)));
        }
    }

    /// M3: clear any queued launch on `lane` (no-op if none / out of range).
    pub fn cancel_launch(&mut self, lane: usize) {
        if let Some(slot) = self.queued.get_mut(lane) {
            *slot = None;
        }
    }

    /// M3: drain and return the lanes that launched during the most recent `tick`.
    /// The engine maps each to an `EngineEvent::Launched`.
    pub fn take_launched(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.just_launched)
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        self.set.bpm = bpm;
    }

    pub fn set_swing(&mut self, swing: f32) {
        self.set.swing = swing;
    }

    pub fn update_lane(&mut self, idx: usize, lane: Lane) {
        if idx < self.set.lanes.len() {
            self.set.lanes[idx] = lane;
        }
    }

    /// Replace a lane's MIDI route (`None` = derive from profile). No-op when out of
    /// bounds. The caller is responsible for releasing the lane's sounding notes BEFORE
    /// the channel changes (so NoteOffs go out on the old channel).
    pub fn set_lane_route(&mut self, lane: usize, route: Option<crate::pattern::model::LaneRoute>) {
        if let Some(l) = self.set.lanes.get_mut(lane) {
            l.route = route;
        }
        // Clear CC cache: the port/channel mapping may have changed, so cached values
        // keyed on the old route could suppress a needed CC on the new route.
        self.cc_cache.clear();
    }

    /// Read accessor for a lane by index (the Sequencer owns `set: Set`).
    pub fn lane(&self, idx: usize) -> Option<&Lane> {
        self.set.lanes.get(idx)
    }

    /// Read accessor for all lanes (the Sequencer owns `set: Set`). Used by the real
    /// engine loop to recompute the route plan after a `SetRoute`.
    pub fn lanes(&self) -> &[Lane] {
        &self.set.lanes
    }

    /// Drive the internal clock to `now_micros`, emitting due events to `sink`.
    /// Returns the step index if a new step was materialized this call.
    pub fn tick(&mut self, now_micros: u64, sink: &mut dyn MidiSink) -> Option<usize> {
        if !self.playing {
            return None;
        }

        // Fix #7 — release held notes on lanes that became inaudible since the
        // last tick (muted or soloed-out). At most 1ms latency is acceptable.
        let any_solo = self.set.lanes.iter().any(|l| l.solo);
        for lane_idx in 0..self.set.lanes.len() {
            if !self.lane_audible(lane_idx, any_solo) {
                if let Some(held) = self.active[lane_idx].take() {
                    self.emit(
                        MidiMessage::NoteOff {
                            channel: held.channel,
                            note: held.note,
                        },
                        lane_idx,
                        now_micros,
                        sink,
                    );
                }
            }
        }

        let mut advanced: Option<usize> = None;

        // M10 T4 — when following an external MIDI clock, step advance is driven ONLY by
        // `advance_clock_step` (one step per 6 incoming ticks), NOT by this internal
        // `step_dur` accumulator. Skip the advance loop entirely so the internal-timing
        // path can never also advance the same step (no double-advance). The inaudible
        // release pass above and `flush_due` below still run — disabling advance does not
        // strand a note.
        if self.clock_driven {
            self.flush_due(now_micros, sink);
            return None;
        }

        // Fix #5 — accumulated schedule: the next step is due at
        // `last_step_at + current_dur` (or at `origin_micros` for step 0).
        // Recomputing `dur` from the CURRENT bpm on every iteration means a
        // mid-play tempo change shifts only future intervals — already-fired
        // steps' timestamps are never retroactively moved.
        loop {
            let dur = step_dur_micros(self.set.bpm);
            // Compute when the upcoming step is due.
            let step_due = match self.last_step_at {
                None => self.origin_micros, // step 0 due at play origin
                Some(prev) => prev + dur,   // subsequent steps
            };
            if step_due > now_micros {
                break;
            }
            let step = self.next_step;
            // M3: apply any queued launches whose boundary matches THIS step, BEFORE
            // materializing it. For each such lane: release its sounding/held notes at
            // the step's fire time (no hang), swap in the queued pattern, set the lane's
            // launch_offset to this step (so its local clock restarts at 0 here), clear
            // the queue slot (exactly-once), and record it for take_launched(). The
            // subsequent materialize_step_at then emits the NEW pattern's local-0 step
            // for that lane. Other lanes are untouched.
            self.apply_due_launches(step, step_due, sink);
            // Pass the actual fire time into materialize so event timestamps
            // reflect the accumulated position, not origin + step * dur.
            self.materialize_step_at(step, dur, step_due);
            self.current = step;
            advanced = Some(step);
            self.next_step += 1;
            // Record this step's fire time so the NEXT step's due time can be
            // computed from it (with whatever bpm is current at that moment).
            self.last_step_at = Some(step_due);
        }

        // Flush all queued events with at_micros <= now.
        self.flush_due(now_micros, sink);
        advanced
    }

    /// M10 T4: enable/disable external-clock-driven advance. When enabled, `tick()`'s
    /// internal `step_dur` advance loop is suppressed and steps advance only via
    /// `advance_clock_step`. Toggling this does NOT touch playing/position — the caller
    /// (engine) owns transport (play/stop) and source exclusivity.
    pub fn set_clock_driven(&mut self, on: bool) {
        self.clock_driven = on;
    }

    /// M10 T4: advance exactly ONE step, driven by an external MIDI clock (called once per
    /// 6 incoming ticks). No-op when stopped. Materializes the next step at fire time
    /// `now_micros` using `bpm` for gate/swing duration math, then flushes due events.
    ///
    /// Reuses the SAME emission path as the internal `tick()` loop (`apply_due_launches`
    /// then `materialize_step_at`), so notes/CC/launches behave identically — the only
    /// difference is what triggers the advance. Returns the materialized step index.
    ///
    /// `last_step_at` is kept consistent (set to `now_micros`) so that if the source later
    /// switches back to internal timing, the next internal step is scheduled from here
    /// rather than back-filling a burst.
    pub fn advance_clock_step(
        &mut self,
        now_micros: u64,
        bpm: f64,
        sink: &mut dyn MidiSink,
    ) -> Option<usize> {
        if !self.playing {
            return None;
        }
        let dur = step_dur_micros(bpm);
        let step = self.next_step;
        // Apply any queued launches whose boundary matches this step BEFORE materializing it
        // (identical ordering to the internal tick loop).
        self.apply_due_launches(step, now_micros, sink);
        self.materialize_step_at(step, dur, now_micros);
        self.current = step;
        self.next_step += 1;
        self.last_step_at = Some(now_micros);
        // Flush any NoteOffs (gate ends) that are already due at this fire time.
        self.flush_due(now_micros, sink);
        Some(step)
    }

    /// Link mode: place the sequencer at musical `beat` (16th = beat*4) at `bpm`.
    ///
    /// Fix #1 — idempotent sync: `next_step` is only advanced forward, never
    /// reset to a step that has already been materialized. Repeated calls with
    /// the same (or non-advancing) beat therefore have no effect on the queue —
    /// each absolute step's NoteOns are emitted at most once. Only when the beat
    /// advances to a new step does `tick` materialize it.
    ///
    /// A BACKWARD jump (`new_step < next_step`, e.g. a Link loop or rewind) is
    /// intentionally IGNORED: the step sequencer does not rewind — it only ever
    /// moves forward — so we never re-materialize a step already emitted.
    ///
    /// Forward-jump re-anchoring (#1/#5 interaction): when Link advances by more
    /// than one step across a sync gap (or jumps the beat), we RE-ANCHOR
    /// `last_step_at` to the scheduled fire time of `new_step - 1`. Without this,
    /// the next `tick` would see a stale `last_step_at` far in the past and
    /// greedily materialize a catch-up burst of every skipped step (ghost notes).
    /// Re-anchoring makes the next tick materialize ONLY `new_step` at its
    /// correct time, skipping the intervening steps without emitting them.
    pub fn sync_to_beat(&mut self, beat: f64, bpm: f64, now_micros: u64) {
        self.set.bpm = bpm;
        let new_step = step_from_beat(beat);
        self.current = new_step;
        // Only move next_step forward; never re-materialize already-emitted steps
        // and never rewind on a backward jump.
        if new_step > self.next_step {
            self.next_step = new_step;
            // Re-anchor the accumulated clock so the next tick fires ONLY `new_step`
            // (no back-fill of the skipped steps).
            //
            // H5: anchor from the current `now` and the beat's fractional step phase
            // rather than `origin_micros + (new_step-1)*dur`. On a mid-session Link
            // join `origin_micros == now` (we just called `play`) while the session
            // beat B > 0, so the origin-based formula placed the anchor ~B*4*dur in
            // the FUTURE and no step ever materialized. `new_step` begins at
            // `now - frac*dur`; anchoring the previous step one `dur` earlier makes
            // `step_due = last_step_at + dur = now - frac*dur <= now`, so `new_step`
            // fires at its correct phase. When `origin` already equals beat-0 time
            // (an ordinary forward jump during play) this yields exactly the same
            // value as before, so that path is unchanged.
            let dur = step_dur_micros(self.set.bpm);
            let frac = (beat * 4.0 - new_step as f64).clamp(0.0, 1.0);
            let into_step = (frac * dur as f64).round() as u64;
            self.last_step_at = Some(now_micros.saturating_sub(into_step).saturating_sub(dur));
        }
    }

    // --- internals -------------------------------------------------------

    /// M3: apply every lane whose queued launch matches absolute `step`'s boundary,
    /// at fire time `at_micros`. Releases the launching lane's sounding/held notes,
    /// swaps in the queued pattern, restarts its local clock at this step, clears the
    /// queue slot (exactly-once), and records it for `take_launched`. Called BEFORE the
    /// step is materialized so the new pattern's local-0 step fires this same tick.
    fn apply_due_launches(&mut self, step: usize, at_micros: u64, sink: &mut dyn MidiSink) {
        // Collect due lanes first so we can take ownership of each pattern without
        // holding a borrow of `self.queued` across the &mut self release/update calls.
        let mut due: Vec<(usize, Pattern, Option<LaunchState>)> = Vec::new();
        for (lane, slot) in self.queued.iter_mut().enumerate() {
            let fire = matches!(slot, Some((_, q, _)) if is_boundary(step, *q));
            if fire {
                // take() leaves None → exactly-once (the slot is cleared on apply).
                if let Some((pattern, _, state)) = slot.take() {
                    due.push((lane, pattern, state));
                }
            }
        }
        for (lane, pattern, state) in due {
            // Release this lane's sounding + held notes so a swap can't hang a note.
            self.release_lanes(&[lane], at_micros, sink);
            if let Some(l) = self.set.lanes.get_mut(lane) {
                l.pattern = pattern;
                // M6: apply the scene's per-lane performance state AT the launch instant
                // (not before). mute/solo affect audibility from this step; transpose/octave
                // shift the new pattern's pitches. tick()'s Fix #7 pass releases any held
                // note on a lane that becomes inaudible, so muting here never hangs a note.
                if let Some(s) = state {
                    l.mute = s.mute;
                    l.solo = s.solo;
                    l.transpose = s.transpose;
                    l.octave = s.octave;
                }
            }
            if let Some(off) = self.launch_offset.get_mut(lane) {
                *off = step;
            }
            self.just_launched.push(lane);
        }
    }

    /// Materialize step `step` at the given absolute fire time `step_at`.
    /// `dur` is the step duration at this step's tempo (for gate/swing calculations).
    fn materialize_step_at(&mut self, step: usize, dur: u64, step_at: u64) {
        // Fix #5: step_at is the accumulated fire time passed from tick(), not
        // recomputed from origin + step * dur, so tempo changes don't shift
        // already-queued steps.
        let step_start = step_at;

        let any_solo = self.set.lanes.iter().any(|l| l.solo);

        for lane_idx in 0..self.set.lanes.len() {
            if !self.lane_audible(lane_idx, any_solo) {
                continue;
            }

            // Per-lane clock division gate (M8 T7): a lane with clock_div=Some(N)
            // where N >= 2 only materializes when (step - launch_offset) % N == 0,
            // anchored to the lane's launch offset exactly as local_step_for is.
            // clock_div=None or Some(1) passes through every step (unchanged behavior).
            let div = self.set.lanes[lane_idx].clock_div.unwrap_or(1).max(1) as usize;
            let off = self.launch_offset.get(lane_idx).copied().unwrap_or(0);
            let steps_since_launch = step.wrapping_sub(off);
            if div >= 2 && steps_since_launch % div != 0 {
                continue;
            }

            // When clock_div=N, the lane's local step must advance once per N global
            // steps.  We convert the global step into a "lane-time step" by dividing
            // the elapsed steps by N before passing to the materializer.  This keeps
            // local_step_for / loop_index_for correct without touching those helpers:
            //   effective_step = off + steps_since_launch / div
            // so that (effective_step - off) % count = (steps_since_launch / div) % count.
            let effective_step = if div >= 2 {
                off.wrapping_add(steps_since_launch / div)
            } else {
                step
            };

            // Per-lane swing override (M8 T7): use lane.swing if set, else global.
            // Computed per-lane so two lanes in the same Set can swing differently.
            let lane_swing = self.set.lanes[lane_idx].swing.unwrap_or(self.set.swing);
            // Swing is keyed on the global step index so even/odd alternation stays
            // consistent with the global grid regardless of division.
            let swung =
                (step_start as i64 + swing_offset_micros(step, lane_swing, dur)).max(0) as u64;

            let kind_is_drums =
                matches!(self.set.lanes[lane_idx].pattern.data, PatternData::Drums(_));
            if kind_is_drums {
                self.materialize_drum_step(lane_idx, effective_step, swung, dur);
            } else {
                self.materialize_melodic_step(lane_idx, effective_step, swung, dur);
            }
        }
    }

    fn lane_audible(&self, lane_idx: usize, any_solo: bool) -> bool {
        let lane = &self.set.lanes[lane_idx];
        if lane.mute {
            return false;
        }
        if any_solo {
            lane.solo
        } else {
            true
        }
    }

    /// M3: lane `lane_idx`'s LOCAL index for absolute `step`, relative to its
    /// `launch_offset`. For a never-launched lane (offset 0) this is exactly `step % count`,
    /// so pre-M3 polymeter/timing behavior is preserved bit-for-bit. `count` is the caller's
    /// already-clamped pattern length (`>= 1`), so the modulo never divides by zero.
    fn local_step_for(&self, lane_idx: usize, step: usize, count: usize) -> usize {
        let off = self.launch_offset.get(lane_idx).copied().unwrap_or(0);
        step.wrapping_sub(off) % count
    }

    /// Number of full pattern cycles the lane has completed since its launch.
    /// Mirrors `local_step_for` but integer-divides instead of modulo.
    fn loop_index_for(&self, lane_idx: usize, step: usize, count: usize) -> u64 {
        let off = self.launch_offset.get(lane_idx).copied().unwrap_or(0);
        (step.wrapping_sub(off) / count) as u64
    }

    /// Set the latched fill-active performance flag (read by Fill/NotFill trig conditions).
    pub fn set_fill_active(&mut self, on: bool) {
        self.fill_active = on;
    }

    fn materialize_drum_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = self.local_step_for(lane_idx, step, count);
        let loop_index = self.loop_index_for(lane_idx, step, count);
        let is_first = loop_index == 0;
        // Emit on the route channel (route override else profile) so the channel the
        // scheduler emits on matches the channel the route plan keys on (no mis-route).
        let channel = lane.route_channel();
        let port_stable_key = lane.effective_route().port.stable_key;
        let gate_fraction = lane.profile.drum_gate_fraction;
        let cc_locks: Vec<CcLock> = lane.pattern.step_cc(local).to_vec();
        let hits = match &lane.pattern.data {
            PatternData::Drums(steps) => steps.get(local).cloned().unwrap_or_default(),
            PatternData::Melodic(_) => Vec::new(),
        };
        // Track the earliest NoteOn time across all firing hits so CC lands just before.
        let mut earliest_on_at: Option<u64> = None;
        for hit in hits {
            // Per-voice mute: skip this hit entirely when the voice is silenced.
            if self.set.lanes[lane_idx].is_voice_muted(hit.note) {
                continue;
            }
            // Trig condition gate runs before the probability roll.
            if !crate::pattern::trig::trig_fires(&hit.cond, loop_index, self.fill_active, is_first)
            {
                continue;
            }
            // Probability is rolled per hit; a failed roll skips the whole hit.
            if !self.rolls_fire(hit.prob) {
                continue;
            }
            // Ratchet: R evenly-spaced NoteOn/NoteOff pairs across the step.
            let r = hit.ratchet.max(1) as u64;
            let sub = dur / r;
            let gate = note_len_micros(gate_fraction, sub);
            // Microtiming: shift the whole group (all ratchet sub-events) by hit.micro,
            // clamped to ±(dur/2) so a hit can never cross into a neighbouring step.
            // Absolute time is also clamped to 0 (can't schedule before epoch).
            let micro = (hit.micro as i64).clamp(-(dur as i64 / 2), dur as i64 / 2);
            let base_on = (swung as i64 + micro).max(0) as u64;
            // Record the earliest NoteOn for CC placement.
            earliest_on_at = Some(match earliest_on_at {
                Some(prev) => prev.min(base_on),
                None => base_on,
            });
            for i in 0..r {
                let on_at = base_on + i * sub;
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: on_at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOn {
                            channel,
                            note: hit.note,
                            vel: hit.vel,
                        },
                    },
                );
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: on_at + gate,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel,
                            note: hit.note,
                        },
                    },
                );
            }
        }
        // Emit CC locks just before the earliest NoteOn, but only when at least one
        // hit fired (CC-only steps with no note do not send CC).
        if let Some(on_at) = earliest_on_at {
            if !cc_locks.is_empty() {
                Self::enqueue_cc_locks(
                    &mut self.queue,
                    &mut self.cc_cache,
                    &cc_locks,
                    lane_idx,
                    channel,
                    &port_stable_key,
                    on_at,
                );
            }
        }
    }

    fn materialize_melodic_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        use crate::devices::profiles::{melodic_velocity, resolve_melodic_pitch};

        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = self.local_step_for(lane_idx, step, count);
        let loop_index = self.loop_index_for(lane_idx, step, count);
        let is_first = loop_index == 0;
        // Emit on the route channel (route override else profile) so emission and the
        // route plan agree on the channel (no mis-route / dropped notes).
        let channel = lane.route_channel();
        let port_stable_key = lane.effective_route().port.stable_key;
        let root = lane.profile.root_note;
        let transpose = lane.transpose;
        let octave = lane.octave;
        let cc_locks: Vec<CcLock> = lane.pattern.step_cc(local).to_vec();

        // M5b Task 3: read the step's full note Vec. 0 notes = rest, 1 = mono
        // (today's behavior, byte-identical), >=2 = chord (poly).
        let step_notes: Vec<MelodicNote> = match &lane.pattern.data {
            PatternData::Melodic(steps) => steps.get(local).map(|s| s.to_vec()).unwrap_or_default(),
            PatternData::Drums(_) => Vec::new(),
        };

        // Chord (>=2 notes): emit each note independently, sharing the step timing.
        // SLIDE is a monophonic 303/SH-101 concept (legato tie to the next step's
        // single note); a chord is NOT slid — each chord note gets its own NoteOff
        // at its own gate. This keeps mono playback exactly as today and gives sane
        // chord behavior. The mono lane (<=1 note) never reaches this branch.
        if step_notes.len() >= 2 {
            self.materialize_chord_step(
                lane_idx,
                &step_notes,
                (channel, &port_stable_key, &cc_locks),
                (loop_index, is_first),
                swung,
                dur,
            );
            return;
        }

        // 0 or 1 note: the original single-note path below is preserved byte-for-byte
        // (`first()` of a <=1-element Vec is the lone note or None == rest).
        let note = match step_notes.into_iter().next() {
            Some(n) => n,
            None => return, // rest: nothing to emit, prior active note keeps its NoteOff.
        };

        // Trig condition gate runs before the probability roll.
        if !crate::pattern::trig::trig_fires(&note.cond, loop_index, self.fill_active, is_first) {
            // This step is the intended legato target of a prior slid note (slide
            // lookahead reads the pattern's slide flag, not the runtime trig outcome).
            // Since the target won't fire, release the held note here or it hangs.
            self.release_held_slide(lane_idx, swung);
            return;
        }
        // Probability is rolled once per note; a failed roll skips the entire step
        // (no NoteOn/NoteOff). A prior note with a scheduled NoteOff keeps its release;
        // a prior note held open for a slide into this step must be released here.
        if !self.rolls_fire(note.prob) {
            self.release_held_slide(lane_idx, swung);
            return;
        }

        let pitch = resolve_melodic_pitch(root, note.semi, transpose, octave);
        let vel = melodic_velocity(note.vel);
        // Microtiming: shift NoteOn (and all NoteOffs/ratchets) by note.micro,
        // clamped to ±(dur/2) so the note can't reorder into a neighbouring step.
        let micro = (note.micro as i64).clamp(-(dur as i64 / 2), dur as i64 / 2);
        let on_at = (swung as i64 + micro).max(0) as u64;

        // Emit CC locks just before the NoteOn (this step fires at least one note).
        if !cc_locks.is_empty() {
            Self::enqueue_cc_locks(
                &mut self.queue,
                &mut self.cc_cache,
                &cc_locks,
                lane_idx,
                channel,
                &port_stable_key,
                on_at,
            );
        }

        // Ratchet: R evenly-spaced retriggers across the step. Slide governs legato into
        // the FIRST retrigger only; the remaining pairs are independent gated hits whose
        // gate = (step_dur/R) * min(note.len, 1.0).
        let r = note.ratchet.max(1) as u64;
        let sub = dur / r;
        let ratchet_gate = note_len_micros(note.len.min(1.0), sub);

        // If a previous note is still active on this lane, release it on the first
        // NoteOn (legato overlap) when it was held for slide; otherwise its NoteOff is
        // already queued and we leave it.
        if let Some(prev) = self.active[lane_idx].take() {
            if prev.off_at.is_none() {
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: on_at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel: prev.channel,
                            note: prev.note,
                        },
                    },
                );
            }
        }

        // Emit the first retrigger's NoteOn.
        Self::enqueue(
            &mut self.queue,
            ScheduledEvent {
                at_micros: on_at,
                lane: lane_idx,
                msg: MidiMessage::NoteOn {
                    channel,
                    note: pitch,
                    vel,
                },
            },
        );

        if r > 1 {
            // Ratchet group: the first NoteOff is scheduled immediately (ratchets are
            // independent), then the remaining R-1 pairs follow at sub-step offsets.
            Self::enqueue(
                &mut self.queue,
                ScheduledEvent {
                    at_micros: on_at + ratchet_gate,
                    lane: lane_idx,
                    msg: MidiMessage::NoteOff {
                        channel,
                        note: pitch,
                    },
                },
            );
            for i in 1..r {
                let at = on_at + i * sub;
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOn {
                            channel,
                            note: pitch,
                            vel,
                        },
                    },
                );
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: at + ratchet_gate,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel,
                            note: pitch,
                        },
                    },
                );
            }
            // The last retrigger's NoteOff is already queued; track it for stop().
            let last_off = on_at + (r - 1) * sub + ratchet_gate;
            self.active[lane_idx] = Some(ActiveNote {
                note: pitch,
                channel,
                off_at: Some(last_off),
            });
            return;
        }

        // ratchet == 1: original single-hit behavior with slide lookahead.
        let off_at = on_at + note_len_micros(note.len, dur);
        let next_slides = self.next_played_note_slides(lane_idx, step, count);
        if next_slides {
            // Hold: do not schedule a NoteOff yet. The next note's materialization
            // will release it just after its own NoteOn.
            self.active[lane_idx] = Some(ActiveNote {
                note: pitch,
                channel,
                off_at: None,
            });
        } else {
            Self::enqueue(
                &mut self.queue,
                ScheduledEvent {
                    at_micros: off_at,
                    lane: lane_idx,
                    msg: MidiMessage::NoteOff {
                        channel,
                        note: pitch,
                    },
                },
            );
            self.active[lane_idx] = Some(ActiveNote {
                note: pitch,
                channel,
                off_at: Some(off_at),
            });
        }
    }

    /// Release a note the previous step left held open for a slide (`off_at == None`)
    /// when its intended legato target step is skipped at runtime (failed trig
    /// condition or probability roll). Without this the held note would never receive
    /// a NoteOff and would hang. A prior note that already has a scheduled NoteOff
    /// (`off_at == Some`) is left untouched — its release still stands.
    fn release_held_slide(&mut self, lane_idx: usize, at: u64) {
        if let Some(prev) = self.active[lane_idx].take() {
            if prev.off_at.is_none() {
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel: prev.channel,
                            note: prev.note,
                        },
                    },
                );
            } else {
                self.active[lane_idx] = Some(prev);
            }
        }
    }

    /// Scan forward from `step+1` to the next non-rest melodic step on this lane and
    /// report whether it has `slide=true`. Bounded to one pattern length so a fully
    /// rested remainder terminates the scan.
    fn next_played_note_slides(&self, lane_idx: usize, step: usize, count: usize) -> bool {
        // Walk forward from the lane's LOCAL position (offset-relative). For an
        // un-launched lane (offset 0) the base equals `step % count`, so this is
        // identical to the pre-M3 `(step + offset) % count`.
        let base = self.local_step_for(lane_idx, step, count);
        if let PatternData::Melodic(steps) = &self.set.lanes[lane_idx].pattern.data {
            for offset in 1..=count {
                let local = (base + offset) % count;
                // Mono lookahead: read the step's FIRST note's slide flag (a non-empty
                // step is a played note; an empty step is a rest, so skip it).
                if let Some(n) = steps.get(local).and_then(|s| s.first()) {
                    return n.slide;
                }
            }
        }
        false
    }

    /// M5b Task 3: emit a CHORD step (>=2 notes) on a poly lane. Each note fires its
    /// own NoteOn at the shared step time and a scheduled NoteOff at its own gate,
    /// honoring per-note velocity / length / probability / ratchet. SLIDE is NOT
    /// applied to chords (it is a monophonic legato tie); the per-note NoteOffs are
    /// scheduled unconditionally, so no chord note is ever held open by slide.
    ///
    /// REGISTRY/RELEASE (M1 invariant): every NoteOn/NoteOff routes through `emit()`
    /// at flush time, which registers/deregisters by (channel, note) in the authoritative
    /// `sounding` registry. Distinct chord notes (distinct note numbers) are tracked
    /// independently, so `release_all` / `release_domain` / `release_lanes` / `release_note`
    /// release ALL of them — no hung notes on stop/panic/mute/solo/set-load/disconnect.
    /// The single-note `active[lane_idx]` legato slot is cleared here: a chord has no
    /// single legato anchor, and release is owned entirely by the registry.
    fn materialize_chord_step(
        &mut self,
        lane_idx: usize,
        notes: &[MelodicNote],
        route_cc: (u8, &str, &[CcLock]),
        loop_ctx: (u64, bool),
        swung: u64,
        dur: u64,
    ) {
        let (channel, port_stable_key, cc_locks) = route_cc;
        let (loop_index, is_first) = loop_ctx;
        use crate::devices::profiles::{melodic_velocity, resolve_melodic_pitch};

        let lane = &self.set.lanes[lane_idx];
        let root = lane.profile.root_note;
        let transpose = lane.transpose;
        let octave = lane.octave;

        // Release any prior slide-held mono note on this lane at the chord's step
        // onset (swung, not shifted) — the legato release belongs to the step boundary,
        // not to any individual note's microtiming offset.
        if let Some(prev) = self.active[lane_idx].take() {
            if prev.off_at.is_none() {
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: swung,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel: prev.channel,
                            note: prev.note,
                        },
                    },
                );
            }
        }
        // No active note tracked for a chord; the registry owns release.
        self.active[lane_idx] = None;

        let mut earliest_on_at: Option<u64> = None;
        for note in notes {
            // Trig condition gate runs before the probability roll.
            if !crate::pattern::trig::trig_fires(&note.cond, loop_index, self.fill_active, is_first)
            {
                continue;
            }
            // Probability is rolled per note (a failed roll skips just that note).
            if !self.rolls_fire(note.prob) {
                continue;
            }
            let pitch = resolve_melodic_pitch(root, note.semi, transpose, octave);
            let vel = melodic_velocity(note.vel);
            // Per-note microtiming: each chord note shifts independently, clamped to
            // ±(dur/2) so it can't reorder into a neighbouring step.
            let micro = (note.micro as i64).clamp(-(dur as i64 / 2), dur as i64 / 2);
            let on_at = (swung as i64 + micro).max(0) as u64;
            // Track earliest NoteOn across all firing chord notes for CC placement.
            earliest_on_at = Some(match earliest_on_at {
                Some(prev) => prev.min(on_at),
                None => on_at,
            });
            // Ratchet: R evenly-spaced gated retriggers across the step, per note.
            let r = note.ratchet.max(1) as u64;
            let sub = dur / r;
            if r > 1 {
                let ratchet_gate = note_len_micros(note.len.min(1.0), sub);
                for i in 0..r {
                    let at = on_at + i * sub;
                    Self::enqueue(
                        &mut self.queue,
                        ScheduledEvent {
                            at_micros: at,
                            lane: lane_idx,
                            msg: MidiMessage::NoteOn {
                                channel,
                                note: pitch,
                                vel,
                            },
                        },
                    );
                    Self::enqueue(
                        &mut self.queue,
                        ScheduledEvent {
                            at_micros: at + ratchet_gate,
                            lane: lane_idx,
                            msg: MidiMessage::NoteOff {
                                channel,
                                note: pitch,
                            },
                        },
                    );
                }
            } else {
                // Single hit: NoteOn now, NoteOff at the note's gate (no slide hold).
                let off_at = on_at + note_len_micros(note.len, dur);
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: on_at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOn {
                            channel,
                            note: pitch,
                            vel,
                        },
                    },
                );
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: off_at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff {
                            channel,
                            note: pitch,
                        },
                    },
                );
            }
        }
        // Emit CC locks just before the earliest NoteOn, only when at least one chord
        // note fired (CC-only steps with no note do not send CC).
        if let Some(on_at) = earliest_on_at {
            if !cc_locks.is_empty() {
                Self::enqueue_cc_locks(
                    &mut self.queue,
                    &mut self.cc_cache,
                    cc_locks,
                    lane_idx,
                    channel,
                    port_stable_key,
                    on_at,
                );
            }
        }
    }

    /// Enqueue CC locks for a step that fires at least one note.
    ///
    /// For each `CcLock` in `locks`, checks the per-route cache keyed by
    /// `(port_stable_key, channel, cc_number)`. If the cached value equals the new
    /// value, the CC is suppressed (no-op). Otherwise the CC is enqueued at
    /// `earliest_on_at.saturating_sub(1)` (just before the NoteOn) and the cache is
    /// updated. CC events do not touch the note-ownership registry.
    fn enqueue_cc_locks(
        queue: &mut Vec<ScheduledEvent>,
        cc_cache: &mut HashMap<(String, u8, u8), u8>,
        locks: &[CcLock],
        lane_idx: usize,
        channel: u8,
        port_stable_key: &str,
        earliest_on_at: u64,
    ) {
        let cc_at = earliest_on_at.saturating_sub(1);
        for lock in locks {
            let key = (port_stable_key.to_string(), channel, lock.cc);
            if cc_cache.get(&key) == Some(&lock.val) {
                continue; // suppress redundant resend
            }
            cc_cache.insert(key, lock.val);
            Self::enqueue(
                queue,
                ScheduledEvent {
                    at_micros: cc_at,
                    lane: lane_idx,
                    msg: MidiMessage::ControlChange {
                        channel,
                        controller: lock.cc,
                        value: lock.val,
                    },
                },
            );
        }
    }

    /// Insert keeping `queue` sorted ascending by `at_micros` (stable for equal times).
    fn enqueue(queue: &mut Vec<ScheduledEvent>, ev: ScheduledEvent) {
        let pos = queue
            .iter()
            .position(|e| e.at_micros > ev.at_micros)
            .unwrap_or(queue.len());
        queue.insert(pos, ev);
    }

    /// Send and remove every queued event whose time is <= now, in time order.
    /// Routes through `emit()` so the sounding registry is updated on every flush.
    fn flush_due(&mut self, now_micros: u64, sink: &mut dyn MidiSink) {
        let i = 0;
        while i < self.queue.len() {
            if self.queue[i].at_micros <= now_micros {
                let ev = self.queue.remove(i);
                self.emit(ev.msg, ev.lane, ev.at_micros, sink);
            } else {
                // queue is time-ordered, so the first future event ends the scan.
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_dur_at_120_bpm_is_125000_micros() {
        // 60_000_000 / (120 * 4) = 125_000
        assert_eq!(step_dur_micros(120.0), 125_000);
    }

    #[test]
    fn step_dur_at_60_bpm_is_250000_micros() {
        assert_eq!(step_dur_micros(60.0), 250_000);
    }

    #[test]
    fn swing_even_steps_have_zero_offset() {
        let dur = step_dur_micros(120.0);
        assert_eq!(swing_offset_micros(0, 0.6, dur), 0);
        assert_eq!(swing_offset_micros(2, 0.6, dur), 0);
        assert_eq!(swing_offset_micros(4, 0.6, dur), 0);
    }

    #[test]
    fn swing_half_is_zero_on_odd_steps() {
        let dur = step_dur_micros(120.0);
        assert_eq!(swing_offset_micros(1, 0.5, dur), 0);
        assert_eq!(swing_offset_micros(3, 0.5, dur), 0);
    }

    #[test]
    fn swing_above_half_delays_odd_steps() {
        let dur = step_dur_micros(120.0); // 125_000
                                          // (0.6 - 0.5) * 125_000 * 2 = 25_000
        assert_eq!(swing_offset_micros(1, 0.6, dur), 25_000);
        assert_eq!(swing_offset_micros(3, 0.6, dur), 25_000);
        assert!(swing_offset_micros(1, 0.6, dur) > 0);
    }

    #[test]
    fn note_len_micros_rounds_fractional_steps() {
        // 0.5 * 125_000 = 62_500
        assert_eq!(note_len_micros(0.5, 125_000), 62_500);
        // 1.5 * 125_000 = 187_500
        assert_eq!(note_len_micros(1.5, 125_000), 187_500);
        // rounding: 0.1 * 125_000 = 12_500
        assert_eq!(note_len_micros(0.1, 125_000), 12_500);
    }

    #[test]
    fn ppqn_is_24() {
        assert_eq!(PPQN, 24);
    }

    #[test]
    fn step_dur_clamps_zero_and_negative_bpm_to_finite() {
        // bpm <= 0 must not yield 0 or u64::MAX (both hang the tick loop).
        let at_min = step_dur_micros(MIN_BPM);
        assert_eq!(step_dur_micros(0.0), at_min);
        assert_eq!(step_dur_micros(-120.0), at_min);
        assert!(step_dur_micros(0.0) > 0 && step_dur_micros(0.0) < u64::MAX);
    }

    #[test]
    fn step_dur_clamps_absurdly_high_bpm() {
        assert_eq!(step_dur_micros(100_000.0), step_dur_micros(MAX_BPM));
    }

    #[test]
    fn step_dur_unchanged_in_normal_range() {
        assert_eq!(step_dur_micros(120.0), 125_000); // regression: normal path intact
    }

    #[test]
    fn scheduled_event_holds_time_lane_msg() {
        let ev = ScheduledEvent {
            at_micros: 1000,
            lane: 2,
            msg: MidiMessage::Clock,
        };
        assert_eq!(ev.at_micros, 1000);
        assert_eq!(ev.lane, 2);
        assert_eq!(ev.msg, MidiMessage::Clock);
    }
}

#[cfg(test)]
mod sequencer_tests {
    use super::*;
    use crate::devices::profiles::{S1, T8_BASS, T8_DRUMS};
    use crate::midi::ports::RecordingSink;
    use crate::midi::MidiMessage;
    use crate::pattern::model::{
        DrumHit, Lane, MelodicNote, MelodicStep, Pattern, PatternData, Set,
    };

    // --- helpers ---------------------------------------------------------

    fn drum_lane_four_on_floor() -> Lane {
        // kick (note 36) on steps 0,4,8,12; 16 steps.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        for &s in &[0usize, 4, 8, 12] {
            steps[s].push(DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "four".to_string(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    // Accepts the legacy `Option<MelodicNote>` per-step shape (None = rest, Some = mono
    // note) and maps it to the new `MelodicStep` Vec form, so the existing literal call
    // sites stay unchanged and assert the SAME mono behavior.
    fn melodic_lane(notes: Vec<Option<MelodicNote>>, profile_bass: bool) -> Lane {
        let len = notes.len();
        let steps: Vec<MelodicStep> = notes
            .into_iter()
            .map(|n| MelodicStep::from(n.into_iter().collect::<Vec<_>>()))
            .collect();
        Lane {
            profile: if profile_bass { T8_BASS } else { S1 },
            pattern: Pattern {
                name: "mel".to_string(),
                desc: String::new(),
                length: len,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    fn set_with(lanes: Vec<Lane>) -> Set {
        Set {
            name: "test".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
            id: crate::persist::Id::nil(),
            scenes: Vec::new(),
            chains: Vec::new(),
            clock_in_port: None,
        }
    }

    /// Advance a sequencer from 0 to `total` µs in `tick` steps, collecting events.
    fn run(seq: &mut Sequencer, sink: &mut RecordingSink, total: u64, tick: u64) {
        let mut now = 0u64;
        while now <= total {
            seq.tick(now, sink);
            now += tick;
        }
    }

    // --- (a) four-on-floor drum timing ----------------------------------

    #[test]
    fn four_on_floor_kick_on_steps_0_4_8_12_with_gap() {
        let dur = step_dur_micros(120.0); // 125_000
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // one bar = 16 steps * 125_000 = 2_000_000 µs. Stop before step 16
        // (which is bar 2 beat 1) — use 16*dur - 1 so the boundary tick at
        // exactly 16*dur is not reached (the fixed scheduler is inclusive).
        run(&mut seq, &mut sink, 16 * dur - 1, 1_000);

        // collect kick NoteOn times.
        let ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0, 4 * dur, 8 * dur, 12 * dur]);

        // each NoteOn has a matching NoteOff drum_gate_fraction*step_dur later.
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 9,
                    note: 36,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            offs,
            vec![gate, 4 * dur + gate, 8 * dur + gate, 12 * dur + gate]
        );
    }

    // --- (b) melodic pitch + velocity -----------------------------------

    #[test]
    fn melodic_note_emits_resolved_pitch_and_velocity() {
        let dur = step_dur_micros(120.0);
        // single note at step 0: semi +7, vel mult 1.0, no slide, len 1.0.
        let notes = vec![
            Some(MelodicNote {
                semi: 7,
                vel: 1.0,
                slide: false,
                len: 1.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }),
            None,
            None,
            None,
        ];
        let mut seq = Sequencer::new(set_with(vec![melodic_lane(notes, true)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, 4 * dur, 1_000);

        // root 45 + semi 7 + transpose 0 + 12*0 = 52; vel = round(1.0*100)=100.
        assert!(sink.events.iter().any(|(t, m)| *t == 0
            && *m
                == MidiMessage::NoteOn {
                    channel: 1,
                    note: 52,
                    vel: 100
                }));
        // NoteOff at len 1.0 * step_dur.
        let off_at = note_len_micros(1.0, dur);
        assert!(sink.events.iter().any(|(t, m)| *t == off_at
            && *m
                == MidiMessage::NoteOff {
                    channel: 1,
                    note: 52
                }));
    }

    // --- (c) slide overlap ----------------------------------------------

    #[test]
    fn slide_note_on_precedes_prior_note_off() {
        let dur = step_dur_micros(120.0);
        // step 0: note A (len 1.0, no slide). step 1: note B with slide=true.
        // The slide on B must hold A until *after* B's NoteOn (legato overlap),
        // so A's NoteOff time > B's NoteOn time.
        let notes = vec![
            Some(MelodicNote {
                semi: 0,
                vel: 1.0,
                slide: false,
                len: 1.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }),
            Some(MelodicNote {
                semi: 5,
                vel: 1.0,
                slide: true,
                len: 1.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }),
            None,
            None,
        ];
        let mut seq = Sequencer::new(set_with(vec![melodic_lane(notes, true)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, 4 * dur, 1_000);

        let note_a = 45u8; // root 45 + semi 0
        let note_b = 50u8; // root 45 + semi 5
        let a_off = sink
            .events
            .iter()
            .find(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 1,
                    note: note_a,
                }
            })
            .map(|(t, _)| *t)
            .expect("A must have a NoteOff");
        let b_on = sink
            .events
            .iter()
            .find(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 1,
                    note: note_b,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .expect("B must have a NoteOn");
        // legato: A is released only after B sounds.
        assert!(
            a_off >= b_on,
            "expected A off ({}) >= B on ({})",
            a_off,
            b_on
        );
    }

    // --- (d) mute silences a lane ---------------------------------------

    #[test]
    fn muted_lane_emits_nothing() {
        let dur = step_dur_micros(120.0);
        let mut lane = drum_lane_four_on_floor();
        lane.mute = true;
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, 16 * dur, 1_000);
        let note_events = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. }))
            .count();
        assert_eq!(note_events, 0);
    }

    // --- (e) solo silences others ---------------------------------------

    #[test]
    fn solo_lane_silences_non_soloed() {
        let dur = step_dur_micros(120.0);
        let drums = drum_lane_four_on_floor(); // not soloed
        let mut bass = melodic_lane(
            vec![
                Some(MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 1.0,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }),
                None,
                None,
                None,
            ],
            true,
        );
        bass.solo = true;
        let mut seq = Sequencer::new(set_with(vec![drums, bass]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, 4 * dur, 1_000);

        // drum lane (channel 9) must be silent under solo.
        let drum_events = sink
            .events
            .iter()
            .filter(|(_, m)| {
                matches!(
                    m,
                    MidiMessage::NoteOn { channel: 9, .. }
                        | MidiMessage::NoteOff { channel: 9, .. }
                )
            })
            .count();
        assert_eq!(drum_events, 0);
        // bass (channel 1) must still sound.
        let bass_on = sink
            .events
            .iter()
            .any(|(_, m)| matches!(m, MidiMessage::NoteOn { channel: 1, .. }));
        assert!(bass_on);
    }

    #[test]
    fn stop_releases_active_notes_and_halts() {
        let dur = step_dur_micros(120.0);
        // a long note still sounding when we stop.
        let notes = vec![Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 4.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        })];
        let mut seq = Sequencer::new(set_with(vec![melodic_lane(notes, true)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // emit the NoteOn
        assert!(seq.is_playing());
        seq.stop(1_000, &mut sink);
        assert!(!seq.is_playing());
        // there is a NoteOff for the still-active note at/after stop time.
        assert!(sink.events.iter().any(|(t, m)| *t >= 1_000
            && *m
                == MidiMessage::NoteOff {
                    channel: 1,
                    note: 45
                }));
        let _ = dur;
    }

    #[test]
    fn sync_to_beat_sets_step_and_bpm() {
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        seq.sync_to_beat(2.5, 140.0, 0); // step = floor(2.5*4) = 10
        assert_eq!(seq.current_step(), 10);
    }

    #[test]
    fn lane_accessor_returns_lane_by_index() {
        let seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let lane = seq.lane(0).expect("lane 0 exists");
        assert_eq!(lane.profile.id, "t8-drums");
        assert!(seq.lane(1).is_none());
    }

    // --- (h) polymeter: lanes of different lengths wrap independently -----

    /// A drum lane of `length` steps with a kick (note 36) on LOCAL step 0 only.
    fn drum_lane_hit_on_step0(length: usize) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); length];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: format!("len{length}"),
                desc: String::new(),
                length,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn lane_step_returns_local_step_per_lane_length() {
        // Three lanes of lengths 12 / 16 / 7. lane_step wraps each by its own length.
        let seq = {
            let mut s = Sequencer::new(set_with(vec![
                drum_lane_hit_on_step0(12),
                drum_lane_hit_on_step0(16),
                drum_lane_hit_on_step0(7),
            ]));
            s.sync_to_beat(5.0, 120.0, 0); // abs step = step_from_beat(5.0) = 20
            s
        };
        assert_eq!(seq.current_step(), 20); // ABSOLUTE step is unwrapped
        assert_eq!(seq.lane_step(0), 20 % 12); // 8
        assert_eq!(seq.lane_step(1), 20 % 16); // 4
        assert_eq!(seq.lane_step(2), 20 % 7); //  6
    }

    #[test]
    fn polymeter_lanes_wrap_independently_over_48_steps() {
        let dur = step_dur_micros(120.0);
        // Lengths 12 / 16 / 7; each fires note 36 at its OWN local step 0.
        let mut seq = Sequencer::new(set_with(vec![
            drum_lane_hit_on_step0(12),
            drum_lane_hit_on_step0(16),
            drum_lane_hit_on_step0(7),
        ]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run 48 absolute steps (a hair past so step 47 materializes).
        run(&mut seq, &mut sink, 48 * dur, 1_000);

        // Lane 0 (channel 9, len 12) fires at abs {0,12,24,36}.
        // Lane 2 (channel 9, len 7) fires at abs {0,7,14,21,28,35,42}.
        // All lanes share channel 9 / note 36, so collect the absolute step indices.
        let fired_abs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t / dur)
            .collect();
        // lane 0 onsets present.
        for s in [0u64, 12, 24, 36] {
            assert!(
                fired_abs.contains(&s),
                "lane0 onset {s} missing in {fired_abs:?}"
            );
        }
        // lane 2 onsets present (independent 7-step wrap).
        for s in [0u64, 7, 14, 21, 28, 35, 42] {
            assert!(
                fired_abs.contains(&s),
                "lane2 onset {s} missing in {fired_abs:?}"
            );
        }
        // lane 1 (16) onsets at 0,16,32 also present.
        for s in [0u64, 16, 32] {
            assert!(
                fired_abs.contains(&s),
                "lane1 onset {s} missing in {fired_abs:?}"
            );
        }
    }

    // --- (i) panic: all-notes-off without stopping transport -------------

    #[test]
    fn panic_emits_cc123_cc120_per_channel_and_keeps_playing() {
        // Two lanes on distinct channels (drums ch9, bass ch1).
        let drums = drum_lane_four_on_floor();
        let bass = melodic_lane(
            vec![
                Some(MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 4.0,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }),
                None,
                None,
                None,
            ],
            true,
        );
        let mut seq = Sequencer::new(set_with(vec![drums, bass]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // emit step-0 events (bass NoteOn becomes active)
        assert!(seq.is_playing());

        seq.panic(10_000, &mut sink);

        // CC 123 (All Notes Off) on each distinct channel.
        for ch in [9u8, 1u8] {
            assert!(
                sink.events.iter().any(|(_, m)| *m
                    == MidiMessage::ControlChange {
                        channel: ch,
                        controller: 123,
                        value: 0
                    }),
                "expected CC123 on channel {ch}"
            );
            // CC 120 (All Sound Off) on each distinct channel.
            assert!(
                sink.events.iter().any(|(_, m)| *m
                    == MidiMessage::ControlChange {
                        channel: ch,
                        controller: 120,
                        value: 0
                    }),
                "expected CC120 on channel {ch}"
            );
        }
        // Panic must NOT stop transport.
        assert!(seq.is_playing(), "panic must leave transport playing");
    }

    // --- route channel: release_all sweeps the emitted channel ----------

    /// release_all/panic must send CC123/120 on the ROUTE channel (the channel notes
    /// are emitted on), not the profile channel, when a lane has a route override.
    #[test]
    fn release_all_sweeps_route_channels() {
        use crate::pattern::model::{LaneRoute, PortRef};
        // Bass lane (profile channel 1) with a route forcing channel 7.
        let mut lane = melodic_lane(
            vec![Some(MelodicNote {
                semi: 0,
                vel: 1.0,
                slide: false,
                len: 4.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            })],
            true,
        );
        let profile_ch = lane.profile.channel; // 1
        lane.route = Some(LaneRoute {
            port: PortRef {
                stable_key: "X".to_string(),
                name: "X".to_string(),
            },
            channel: 7,
            clock_out: true,
        });
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // NoteOn fires on channel 7

        // The note must have sounded on the route channel, not the profile channel.
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { channel: 7, .. })),
            "NoteOn must be on the route channel 7"
        );

        seq.panic(10_000, &mut sink);

        // CC123 + CC120 on the ROUTE channel 7.
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: 7,
                    controller: 123,
                    value: 0,
                }),
            "expected CC123 on the route channel 7"
        );
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: 7,
                    controller: 120,
                    value: 0,
                }),
            "expected CC120 on the route channel 7"
        );
        // And NOT on the (now unused) profile channel.
        assert!(
            !sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: profile_ch,
                    controller: 123,
                    value: 0,
                }),
            "must not sweep the profile channel {profile_ch} when a route overrides it"
        );
    }

    // --- (f) probability -------------------------------------------------

    /// A single-step drum lane with the kick hit on step 0 only, prob `p`, ratchet `r`.
    fn drum_lane_one_hit(prob: f32, ratchet: u8) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob,
            ratchet,
            micro: 0,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "one".to_string(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    fn kick_on_count(sink: &RecordingSink) -> usize {
        sink.events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .count()
    }

    #[test]
    fn prob_one_always_emits_and_prob_zero_never_emits() {
        let dur = step_dur_micros(120.0);
        // prob = 1.0 → always fires.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_one_hit(1.0, 1)]));
        seq.set_seed(12345);
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur, 1_000);
        assert_eq!(kick_on_count(&sink), 1);

        // prob = 0.0 → never fires.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_one_hit(0.0, 1)]));
        seq.set_seed(12345);
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur, 1_000);
        assert_eq!(kick_on_count(&sink), 0);
    }

    #[test]
    fn seeded_prob_half_fires_a_stable_step_set() {
        let dur = step_dur_micros(120.0);
        // 16 steps, kick on EVERY step, each prob = 0.5. With a fixed seed the fired
        // set is deterministic — pin it so a PRNG change is caught.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        for step in steps.iter_mut() {
            step.push(DrumHit {
                note: 36,
                vel: 100,
                prob: 0.5,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "row".to_string(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        let run_once = |seed: u64| -> Vec<u64> {
            let mut seq = Sequencer::new(set_with(vec![lane.clone()]));
            seq.set_seed(seed);
            let mut sink = RecordingSink::new();
            seq.play(0);
            run(&mut seq, &mut sink, 16 * dur, 1_000);
            sink.events
                .iter()
                .filter(|(_, m)| {
                    *m == MidiMessage::NoteOn {
                        channel: 9,
                        note: 36,
                        vel: 100,
                    }
                })
                .map(|(t, _)| *t / dur)
                .collect()
        };
        let fired = run_once(0xABCDEF);
        // Stable across identical seed.
        assert_eq!(fired, run_once(0xABCDEF));
        // Roughly half fire (not all, not none) — guards against a degenerate PRNG.
        assert!(!fired.is_empty() && fired.len() < 16);
    }

    // --- (g) ratcheting --------------------------------------------------

    #[test]
    fn ratchet_three_emits_three_noteons_at_sub_offsets() {
        let dur = step_dur_micros(120.0);
        let sub = dur / 3;
        let mut seq = Sequencer::new(set_with(vec![drum_lane_one_hit(1.0, 3)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur, 1_000);

        let ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0, sub, 2 * sub]);

        // Each NoteOn has a NoteOff a (sub * drum_gate_fraction) gate later.
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, sub);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 9,
                    note: 36,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(offs, vec![gate, sub + gate, 2 * sub + gate]);
    }

    // --- (j) coarse-tick boundary regression --------------------------------

    /// Drive the sequencer with ticks landing EXACTLY on step boundaries and assert:
    /// 1. Step 0 is emitted exactly once after play(t) + tick(t).
    /// 2. Each subsequent step is materialized on the tick whose time == step_start
    ///    (i.e. not deferred to a later tick).
    #[test]
    fn step_at_exact_tick_boundary_is_not_deferred() {
        let dur = step_dur_micros(120.0); // 125_000
                                          // Four-on-floor kick at steps 0, 4, 8, 12 — gives us clear NoteOn timestamps.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let mut sink = RecordingSink::new();
        let origin = 1_000_000u64; // non-zero origin to test general case

        seq.play(origin);

        // Tick exactly at each step boundary for the first 5 steps (0..=4),
        // which covers steps 0 and 4 — both have kick hits in four-on-floor.
        for i in 0..=4usize {
            let boundary = origin + i as u64 * dur;
            seq.tick(boundary, &mut sink);
        }

        // Collect kick NoteOn timestamps.
        let ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();

        // Step 0 (at origin) must appear exactly once — not zero (deferred) and not twice (double-emit).
        let step0_count = ons.iter().filter(|&&t| t == origin).count();
        assert_eq!(
            step0_count, 1,
            "step 0 must be emitted exactly once, got {step0_count}"
        );

        // Steps 0 and 4 fall on exact boundaries (steps 1,2,3 have no kick hit).
        // Step 0 NoteOn must be at origin, step 4 NoteOn at origin + 4*dur.
        assert!(
            ons.contains(&origin),
            "step 0 NoteOn at boundary {origin} was deferred; ons={ons:?}"
        );
        assert!(
            ons.contains(&(origin + 4 * dur)),
            "step 4 NoteOn at boundary {} was deferred; ons={ons:?}",
            origin + 4 * dur
        );
    }

    #[test]
    fn ratchet_one_matches_single_hit_baseline() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_one_hit(1.0, 1)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur, 1_000);
        let ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0]);
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 9,
                    note: 36,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(offs, vec![gate]);
    }

    // =========================================================================
    // Regression tests for fixes #1, #5, #7
    // =========================================================================

    // --- Fix #5: accumulated step scheduling (no tempo-change distortion) ----

    /// A drum lane with a kick on every step, so we can measure inter-step gaps.
    fn drum_lane_every_step(length: usize) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); length];
        for step in steps.iter_mut() {
            step.push(DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "every".to_string(),
                desc: String::new(),
                length,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn tempo_change_only_affects_future_step_intervals() {
        // Play at 120 bpm → step_dur = 125_000 µs.
        // Tick through steps 0 and 1, then change to 60 bpm → step_dur = 250_000 µs.
        // The gap between step 1 and step 2 should be 250_000 µs (new tempo),
        // not a burst of catch-up notes (old distortion: gap would be near 0)
        // or a huge pause (retroactive recalculation).
        let dur_120 = step_dur_micros(120.0); // 125_000
        let dur_60 = step_dur_micros(60.0); // 250_000

        let mut seq = Sequencer::new(set_with(vec![drum_lane_every_step(16)]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Tick through step 0 (t=0) and step 1 (t=125_000).
        seq.tick(0, &mut sink);
        seq.tick(dur_120, &mut sink);

        // Capture timestamps so far to assert they are unchanged later.
        let early_ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            early_ons,
            vec![0, dur_120],
            "steps 0 and 1 must fire at 120 bpm intervals"
        );

        // Change tempo mid-play.
        seq.set_bpm(60.0);

        // Tick through a window that covers step 2 at the NEW duration.
        // Step 2 should fire at: step1_at + dur_60 = 125_000 + 250_000 = 375_000.
        // (Old buggy code: origin + 2 * dur_60 = 0 + 500_000 — a huge pause.)
        let step2_expected = dur_120 + dur_60; // 375_000
        seq.tick(step2_expected, &mut sink);

        let all_ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();

        // Step 2 must be at the accumulated position (125_000 + 250_000).
        assert!(
            all_ons.contains(&step2_expected),
            "step 2 must fire at accumulated position {step2_expected}, got {all_ons:?}"
        );

        // Earlier steps' timestamps must be unchanged.
        assert_eq!(
            all_ons[0], 0,
            "step 0 timestamp must not be retroactively changed"
        );
        assert_eq!(
            all_ons[1], dur_120,
            "step 1 timestamp must not be retroactively changed"
        );

        // The gap between step 1 and step 2 equals the NEW step duration.
        assert_eq!(
            all_ons[2] - all_ons[1],
            dur_60,
            "gap after tempo change must equal new step_dur"
        );
    }

    // --- Fix #1: idempotent Link sync (no repeated step emission) -----------

    #[test]
    fn sync_to_beat_repeated_calls_emit_step_exactly_once() {
        // A lane with a kick on step 0 (beat 0.0 → step 0).
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Repeatedly call sync_to_beat with the SAME beat, interleaved with tick.
        // Step 0 should appear exactly once.
        for _ in 0..5 {
            seq.sync_to_beat(0.0, 120.0, 0);
            seq.tick(0, &mut sink);
        }

        let kick_ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();

        assert_eq!(
            kick_ons.len(),
            1,
            "step 0 NoteOn must be emitted exactly once, got {} times: {:?}",
            kick_ons.len(),
            kick_ons
        );

        // Now advance the beat to step 4 (beat 1.0 → step 4 has a kick in four-on-floor).
        // But since we need to let tick fire it, we provide a time past step 4.
        // Advance next_step_at by ticking normally for a few more steps then sync.
        seq.sync_to_beat(1.0, 120.0, 4 * dur); // step 4
        seq.tick(4 * dur, &mut sink);

        let kick_ons2: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note: 36,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();

        // After advancing to beat 1.0 (step 4) and ticking, step 4 kick should appear.
        assert_eq!(
            kick_ons2.len(),
            2,
            "advancing beat to step 4 must emit step 4 kick; total NoteOns: {:?}",
            kick_ons2
        );
    }

    // --- Fix #7: release held note when lane becomes muted / soloed-out -----

    /// Build a lane with a slide note at step 0 so the sequencer holds the note
    /// (off_at = None → no scheduled NoteOff until the next step fires).
    fn melodic_lane_slide_held() -> Lane {
        // step 0: note semi=0, slide=true (so sequencer holds it).
        // step 1: rest — slide lookahead won't find a following note, but slide
        // is on step 0 itself so the active note is held until step 1 materializes.
        // We want the note HELD after step 0 fires.
        let notes = vec![
            Some(MelodicNote {
                semi: 0,
                vel: 1.0,
                slide: true,
                len: 1.0,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }),
            None,
            None,
            None,
        ];
        let steps: Vec<MelodicStep> = notes
            .into_iter()
            .map(|n| MelodicStep::from(n.into_iter().collect::<Vec<_>>()))
            .collect();
        Lane {
            profile: T8_BASS,
            pattern: Pattern {
                name: "slide".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn mute_releases_held_slide_note_on_next_tick() {
        let dur = step_dur_micros(120.0);
        // Lane 0: slide held note. Lane 1: drum (to give us a non-muted companion).
        let mut seq = Sequencer::new(set_with(vec![
            melodic_lane_slide_held(),
            drum_lane_four_on_floor(),
        ]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Tick step 0 — melodic NoteOn fires and note is held (slide).
        seq.tick(0, &mut sink);

        // Confirm the NoteOn fired.
        let note_pitch = 45u8; // T8_BASS root 45 + semi 0 = 45
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::NoteOn {
                    channel: 1,
                    note: note_pitch,
                    vel: 100
                }),
            "NoteOn for held note must have fired"
        );

        // No NoteOff yet (it's slide-held).
        let noteoff_before = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 1,
                    note: note_pitch,
                }
            })
            .count();
        assert_eq!(noteoff_before, 0, "held note must not have a NoteOff yet");

        // Mute the melodic lane.
        let mut muted = melodic_lane_slide_held();
        muted.mute = true;
        seq.update_lane(0, muted);

        // Tick — Fix #7 must release the held note.
        let mute_tick_time = dur / 2; // some time before step 1
        seq.tick(mute_tick_time, &mut sink);

        let noteoff_after = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 1,
                    note: note_pitch,
                }
            })
            .count();
        assert_eq!(
            noteoff_after, 1,
            "muting a lane must release its held slide note via NoteOff"
        );
    }

    #[test]
    fn solo_other_lane_releases_held_slide_note_on_next_tick() {
        let dur = step_dur_micros(120.0);
        // Lane 0: melodic (slide held). Lane 1: drums (will be soloed).
        let mut seq = Sequencer::new(set_with(vec![
            melodic_lane_slide_held(),
            drum_lane_four_on_floor(),
        ]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Fire step 0 — melodic note is held.
        seq.tick(0, &mut sink);

        let note_pitch = 45u8;
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::NoteOn {
                    channel: 1,
                    note: note_pitch,
                    vel: 100
                }),
            "NoteOn for held note must have fired"
        );

        // Solo the DRUM lane (lane 1), leaving melodic lane (lane 0) silenced.
        let mut soloed_drums = drum_lane_four_on_floor();
        soloed_drums.solo = true;
        seq.update_lane(1, soloed_drums);

        // Tick — Fix #7 must release the melodic lane's held note.
        let solo_tick_time = dur / 2;
        seq.tick(solo_tick_time, &mut sink);

        let noteoff_count = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 1,
                    note: note_pitch,
                }
            })
            .count();
        assert_eq!(
            noteoff_count, 1,
            "soloing another lane must release the silenced lane's held slide note"
        );
    }

    // --- Fix #1/#5 interaction: forward Link jump must not back-fill steps ----

    /// A 4-step drum lane with a DISTINCT kick note per step so we can tell which
    /// steps fired. Step 0 → note 36, step 1 → 37, step 2 → 38, step 3 → 39.
    fn drum_lane_distinct_per_step() -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        for (i, step) in steps.iter_mut().enumerate() {
            step.push(DrumHit {
                note: 36 + i as u8,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "distinct".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn forward_link_jump_does_not_backfill_skipped_steps() {
        // Ghost-step regression: a forward Link jump from step 0 to step 4 must
        // NOT emit the notes for the skipped steps 1, 2, 3. The pattern has a
        // distinct note on every step so a back-filled catch-up burst is visible.
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_distinct_per_step()]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Link reports beat 0.0 → step 0; tick fires step 0 (note 36).
        seq.sync_to_beat(0.0, 120.0, 0);
        seq.tick(0, &mut sink);

        // Link jumps forward to beat 1.0 → step 4 (skipping 1, 2, 3).
        // step 4 wraps to local step 0 in the 4-step pattern → note 36 again.
        seq.sync_to_beat(1.0, 120.0, 4 * dur);
        seq.tick(4 * dur, &mut sink);

        let fired_notes: Vec<u8> = sink
            .events
            .iter()
            .filter_map(|(_, m)| match m {
                MidiMessage::NoteOn { note, .. } => Some(*note),
                _ => None,
            })
            .collect();

        // Steps 1, 2, 3 (notes 37, 38, 39) must NEVER have fired.
        for ghost in [37u8, 38, 39] {
            assert!(
                !fired_notes.contains(&ghost),
                "skipped step's note {ghost} was ghost-emitted; fired={fired_notes:?}"
            );
        }
        // Step 0 fired (initial) and step 4 fired (wraps to note 36) → exactly two 36s.
        let note36_count = fired_notes.iter().filter(|&&n| n == 36).count();
        assert_eq!(
            note36_count, 2,
            "expected step 0 + step 4 (both note 36), got {fired_notes:?}"
        );
    }

    #[test]
    fn forward_step_by_step_link_sync_advances_normally() {
        // Guard against over-skipping: a one-step-at-a-time forward sync must
        // still fire each step's distinct note in order.
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_distinct_per_step()]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // beat for step k is k/4. Drive steps 0..=3 one at a time.
        for step in 0..4u64 {
            let beat = step as f64 / 4.0;
            seq.sync_to_beat(beat, 120.0, step * dur);
            seq.tick(step * dur, &mut sink);
        }

        let fired_notes: Vec<u8> = sink
            .events
            .iter()
            .filter_map(|(_, m)| match m {
                MidiMessage::NoteOn { note, .. } => Some(*note),
                _ => None,
            })
            .collect();
        // Every step's note must appear exactly once, in order.
        assert_eq!(
            fired_notes,
            vec![36, 37, 38, 39],
            "step-by-step must fire all steps"
        );
    }

    #[test]
    fn zero_bpm_set_does_not_hang_tick() {
        let mut set = set_with(vec![drum_lane_four_on_floor()]);
        set.bpm = 0.0;
        let mut seq = Sequencer::new(set);
        let mut sink = RecordingSink::new();
        seq.play(0);
        // If unclamped this loops forever; clamped it advances a bounded number of steps.
        seq.tick(1_000_000, &mut sink); // must return
        assert!(seq.current_step() < 1000, "step count must be bounded");
    }

    // =========================================================================
    // Task 2: Active-note registry tests
    // =========================================================================

    // Helper: inject a sounding note with a chosen domain directly into the
    // registry (test-only — simulates a note that was emitted before the test
    // body but whose domain we want to control).
    fn inject_sounding(
        seq: &mut Sequencer,
        channel: u8,
        note: u8,
        lane: usize,
        domain: NoteDomain,
    ) {
        // Remove any existing entry for (channel, note) then push a fresh one.
        seq.sounding
            .retain(|s| !(s.channel == channel && s.note == note));
        seq.sounding.push(SoundingNote {
            channel,
            note,
            lane,
            domain,
        });
    }

    /// P4 regression: stop() must release a drum note whose NoteOn was already
    /// flushed to the sink but whose NoteOff is still in the queue.
    #[test]
    fn stop_releases_flushed_drum_with_queued_noteoff() {
        let dur = step_dur_micros(120.0);
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        // One drum lane: kick on step 0.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Tick exactly at t=0: the NoteOn is materialized AND flushed to sink
        // (at_micros=0 <= now=0). The NoteOff is queued at t=gate (future).
        seq.tick(0, &mut sink);

        // Confirm NoteOn flushed, NoteOff still pending (not yet in sink).
        let noteoff_before = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 9,
                    note: 36,
                }
            })
            .count();
        assert_eq!(
            noteoff_before, 0,
            "NoteOff must still be queued, not flushed yet"
        );
        assert_eq!(
            seq.sounding_count(),
            1,
            "kick must be in the sounding registry"
        );

        // Stop before the NoteOff is due. P4 bug: without registry, stop() only
        // released `active` (melodic tracking), not the drum note — it would hang.
        seq.stop(1, &mut sink);

        let noteoff_after = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 9,
                    note: 36,
                }
            })
            .count();
        assert_eq!(
            noteoff_after, 1,
            "stop() must release the still-sounding drum note (P4 fix)"
        );
        assert_eq!(
            seq.sounding_count(),
            0,
            "registry must be empty after stop()"
        );
        assert!(!seq.is_playing());
        let _ = gate;
    }

    /// release_domain releases only notes in its domain, leaving others sounding.
    #[test]
    fn release_domain_only_releases_its_domain() {
        let mut seq = Sequencer::new(set_with(vec![drum_lane_four_on_floor()]));
        let mut sink = RecordingSink::new();

        // Inject two notes in different domains.
        inject_sounding(&mut seq, 1, 60, 0, NoteDomain::Playback);
        inject_sounding(&mut seq, 2, 62, 0, NoteDomain::Audition);
        assert_eq!(seq.sounding_count(), 2);

        // Release only Audition.
        seq.release_domain(NoteDomain::Audition, 1000, &mut sink);

        assert_eq!(seq.sounding_count(), 1, "Playback note must remain");
        // The Audition note (ch2, note 62) got a NoteOff.
        assert!(sink.events.iter().any(|(_, m)| *m
            == MidiMessage::NoteOff {
                channel: 2,
                note: 62
            }));
        // The Playback note (ch1, note 60) must NOT have a NoteOff.
        assert!(!sink.events.iter().any(|(_, m)| *m
            == MidiMessage::NoteOff {
                channel: 1,
                note: 60
            }));
    }

    /// release_lanes releases only notes on the named lanes.
    #[test]
    fn release_lanes_releases_only_named_lanes() {
        let mut seq = Sequencer::new(set_with(vec![
            drum_lane_four_on_floor(), // lane 0
            drum_lane_four_on_floor(), // lane 1
        ]));
        let mut sink = RecordingSink::new();

        // Inject one note per lane.
        inject_sounding(&mut seq, 9, 36, 0, NoteDomain::Playback);
        inject_sounding(&mut seq, 9, 38, 1, NoteDomain::Playback);
        assert_eq!(seq.sounding_count(), 2);

        // Release only lane 0.
        seq.release_lanes(&[0], 1000, &mut sink);

        assert_eq!(seq.sounding_count(), 1, "lane 1 note must remain");
        assert!(sink.events.iter().any(|(_, m)| *m
            == MidiMessage::NoteOff {
                channel: 9,
                note: 36
            }));
        assert!(!sink.events.iter().any(|(_, m)| *m
            == MidiMessage::NoteOff {
                channel: 9,
                note: 38
            }));
    }

    /// panic() emits CC123+CC120 per channel, NoteOff for every sounding note,
    /// clears the registry, and leaves transport playing.
    #[test]
    fn panic_emits_cc123_120_and_clears_registry() {
        let drums = drum_lane_four_on_floor();
        let bass = melodic_lane(
            vec![
                Some(MelodicNote {
                    semi: 0,
                    vel: 1.0,
                    slide: false,
                    len: 4.0,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }),
                None,
                None,
                None,
            ],
            true,
        );
        let mut seq = Sequencer::new(set_with(vec![drums, bass]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // bass NoteOn emitted → enters registry

        // Inject an extra Audition note to confirm panic() clears ALL domains.
        inject_sounding(&mut seq, 3, 72, 0, NoteDomain::Audition);

        let count_before = seq.sounding_count();
        assert!(count_before >= 2, "bass + audition note must be sounding");

        seq.panic(10_000, &mut sink);

        assert_eq!(
            seq.sounding_count(),
            0,
            "panic() must clear the entire registry"
        );
        assert!(seq.is_playing(), "panic() must leave transport playing");
        // CC123 + CC120 on each distinct lane channel.
        for ch in [9u8, 1u8] {
            assert!(sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: ch,
                    controller: 123,
                    value: 0
                }));
            assert!(sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: ch,
                    controller: 120,
                    value: 0
                }));
        }
    }

    #[test]
    fn backward_sync_to_beat_is_a_noop() {
        // A backward Link jump (loop/rewind) must not rewind the sequencer or
        // re-emit an already-played step.
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_distinct_per_step()]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // Advance to step 2 (beat 0.5) and fire it.
        seq.sync_to_beat(0.5, 120.0, 2 * dur); // step 2
        seq.tick(2 * dur, &mut sink);
        assert_eq!(seq.current_step(), 2);

        let before = sink.events.len();

        // Backward jump to step 0 (beat 0.0). next_step must NOT rewind.
        seq.sync_to_beat(0.0, 120.0, 2 * dur);
        // current is allowed to reflect the reported beat, but next_step must stay
        // ahead so no step is re-materialized.
        seq.tick(2 * dur, &mut sink);

        // No new events emitted by the backward jump + tick.
        assert_eq!(
            sink.events.len(),
            before,
            "backward sync_to_beat must be a no-op (no re-emitted steps)"
        );
    }

    // =========================================================================
    // M3 Task 1: quantized per-lane launch queue + boundary launch
    // =========================================================================

    /// A 16-step drum lane firing the given kick `note` on LOCAL step 0 only.
    fn drum_lane_step0_note(note: u8) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: format!("k{note}"),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// The pattern half of `drum_lane_step0_note` (for queuing).
    fn drum_pattern_step0_note(note: u8) -> Pattern {
        drum_lane_step0_note(note).pattern
    }

    fn kick_note_on_abs_steps(sink: &RecordingSink, note: u8, dur: u64) -> Vec<u64> {
        sink.events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 9,
                    note,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t / dur)
            .collect()
    }

    #[test]
    fn is_boundary_matches_bar_and_beat_grids() {
        // NextBar: multiples of 16. NextBeat: multiples of 4. Both true at 0.
        assert!(is_boundary(0, Quant::NextBar));
        assert!(is_boundary(16, Quant::NextBar));
        assert!(is_boundary(32, Quant::NextBar));
        assert!(!is_boundary(4, Quant::NextBar));
        assert!(!is_boundary(15, Quant::NextBar));

        assert!(is_boundary(0, Quant::NextBeat));
        assert!(is_boundary(4, Quant::NextBeat));
        assert!(is_boundary(8, Quant::NextBeat));
        assert!(!is_boundary(3, Quant::NextBeat));
        assert!(!is_boundary(5, Quant::NextBeat));
    }

    #[test]
    fn queued_pattern_does_not_launch_before_boundary() {
        let dur = step_dur_micros(120.0);
        // Lane 0 plays kick note 36 on local step 0. Queue a pattern with note 50
        // (NextBar). Before step 16, the new pattern (note 50) must never fire.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run up to (but not reaching) step 16: stop strictly before 16*dur.
        // Queue at step 3 (NextBar).
        let mut now = 0u64;
        while now < 16 * dur {
            if now == 3 * dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }

        let new_pattern_ons = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 50, .. }))
            .count();
        assert_eq!(
            new_pattern_ons, 0,
            "queued pattern must not fire before the bar boundary"
        );
        // The original pattern (note 36) DID fire at step 0.
        assert!(
            sink.events
                .iter()
                .any(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. })),
            "original pattern must still play before the boundary"
        );
    }

    #[test]
    fn queued_pattern_launches_at_bar_and_starts_local_0() {
        let dur = step_dur_micros(120.0);
        // Original: note 36 on local 0. Queue note 50 (NextBar) at step 2.
        // At GLOBAL step 16 the new pattern's LOCAL step 0 must fire (note 50 at abs 16).
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut now = 0u64;
        while now <= 17 * dur {
            if now == 2 * dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }

        // New pattern (note 50) fires at absolute step 16 (its local step 0).
        let new_ons = kick_note_on_abs_steps(&sink, 50, dur);
        assert!(
            new_ons.contains(&16),
            "new pattern's local-0 hit must fire at global step 16; got {new_ons:?}"
        );
        // The launch is recorded for the engine to confirm (just_launched accumulates
        // across ticks until drained), and the offset aligned local-0 to the bar.
        let launched = seq.take_launched();
        assert!(
            launched.contains(&0),
            "lane 0 must be reported as launched; got {launched:?}"
        );
    }

    #[test]
    fn next_beat_launches_at_next_multiple_of_4() {
        let dur = step_dur_micros(120.0);
        // Queue at step 1 with NextBeat → must launch at step 4 (next multiple of 4),
        // NOT before, and the new pattern's local-0 fires at absolute step 4.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut now = 0u64;
        while now <= 5 * dur {
            if now == dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBeat);
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        let new_ons = kick_note_on_abs_steps(&sink, 50, dur);
        assert!(
            new_ons.contains(&4),
            "NextBeat launch must fire the new pattern's local-0 at step 4; got {new_ons:?}"
        );
        // Must not have launched earlier (steps 2 or 3 are not beat boundaries).
        assert!(
            !new_ons.iter().any(|&s| s < 4),
            "new pattern must not fire before step 4; got {new_ons:?}"
        );
    }

    #[test]
    fn launch_is_exactly_once() {
        let dur = step_dur_micros(120.0);
        // Queue note 50 (NextBar) at step 2; run two full bars. The new pattern's
        // local-0 fires at abs 16 (launch) and then at abs 32 (its OWN wrap, len 16),
        // but it must NOT re-launch (re-apply the queue) at step 32. We detect a
        // double application by the launch_offset only being set once: the new
        // pattern wraps with len 16 so its local-0 falls on abs {16, 32}. A re-launch
        // would reset the offset to 32 and ALSO re-release — observed via take_launched.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut launched_events: Vec<usize> = Vec::new();
        let mut now = 0u64;
        while now <= 33 * dur {
            if now == 2 * dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
            }
            seq.tick(now, &mut sink);
            launched_events.extend(seq.take_launched());
            now += 1_000;
        }
        // The lane launched exactly once (at the first boundary), never re-applied.
        assert_eq!(
            launched_events.len(),
            1,
            "launch must happen exactly once; got {launched_events:?}"
        );
        // Sanity: the new pattern still wraps normally afterward (abs 16 and 32 both fire).
        let new_ons = kick_note_on_abs_steps(&sink, 50, dur);
        assert!(new_ons.contains(&16) && new_ons.contains(&32));
    }

    #[test]
    fn cancel_launch_prevents_it() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut now = 0u64;
        while now <= 17 * dur {
            if now == 2 * dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
            }
            if now == 5 * dur {
                seq.cancel_launch(0);
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // Cancelled before the boundary → new pattern never fires.
        let new_ons = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 50, .. }))
            .count();
        assert_eq!(new_ons, 0, "cancelled launch must not fire");
        // Original pattern keeps playing (note 36 fires at abs 0 and 16).
        let orig = kick_note_on_abs_steps(&sink, 36, dur);
        assert!(
            orig.contains(&0) && orig.contains(&16),
            "original pattern must keep playing after cancel; got {orig:?}"
        );
    }

    #[test]
    fn other_lanes_unaffected_by_a_lane_launch() {
        let dur = step_dur_micros(120.0);
        // Lane 0: note 36 local0. Lane 1: note 40 local0 (independent). Launch lane 0.
        // Lane 1 must keep firing note 40 at abs {0,16,32} regardless.
        let mut seq = Sequencer::new(set_with(vec![
            drum_lane_step0_note(36),
            drum_lane_step0_note(40),
        ]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut now = 0u64;
        while now <= 17 * dur {
            if now == 2 * dur {
                seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // Lane 1 (note 40) fired at its own local-0 positions abs 0 and 16, untouched
        // by lane 0's launch and offset.
        let lane1 = kick_note_on_abs_steps(&sink, 40, dur);
        assert!(
            lane1.contains(&0) && lane1.contains(&16),
            "untouched lane must keep its original local-0 schedule; got {lane1:?}"
        );
        // Lane 0's launch took effect (note 50 at abs 16).
        let lane0_new = kick_note_on_abs_steps(&sink, 50, dur);
        assert!(lane0_new.contains(&16), "launching lane DID launch");
    }

    #[test]
    fn launch_releases_held_slide_note() {
        let dur = step_dur_micros(120.0);
        // Lane 0: a slide-held melodic note on step 0 (held, off_at=None). Queue a
        // drum pattern (NextBar). At the boundary the held note must be released
        // (NoteOff for note 45 on channel 1) before the new pattern materializes.
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_slide_held()]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Tick step 0 so the held note sounds.
        seq.tick(0, &mut sink);
        let held_pitch = 45u8;
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::NoteOn {
                    channel: 1,
                    note: held_pitch,
                    vel: 100
                }),
            "held slide note must have sounded"
        );
        // No NoteOff yet (slide-held).
        assert_eq!(
            sink.events
                .iter()
                .filter(|(_, m)| *m
                    == MidiMessage::NoteOff {
                        channel: 1,
                        note: held_pitch
                    })
                .count(),
            0,
            "held note must not be released before launch"
        );
        // Queue a replacement pattern (a 4-step drum lane) at NextBar.
        seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);
        // Run to the bar boundary (step 16).
        let mut now = 1_000u64;
        while now <= 16 * dur + dur {
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // The held note got a NoteOff at/after the boundary (release on launch).
        assert!(
            sink.events.iter().any(|(at, m)| *at >= 16 * dur
                && *m
                    == MidiMessage::NoteOff {
                        channel: 1,
                        note: held_pitch
                    }),
            "launch must release the lane's held slide note at the boundary; got {:?}",
            sink.events
        );
    }

    #[test]
    fn launch_under_link_fires_at_boundary() {
        let dur = step_dur_micros(120.0);
        // Drive the absolute step counter via sync_to_beat (Link), not manual interval
        // accumulation. Queue note 50 (NextBar) while at an early beat; advance the
        // beat across step 16 and confirm the new pattern's local-0 fires there.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);

        // beat 0.0 → step 0; fire it.
        seq.sync_to_beat(0.0, 120.0, 0);
        seq.tick(0, &mut sink);

        // Queue at step 0..boundary.
        seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);

        // Link advances to beat 4.0 → step 16 (the bar boundary). tick materializes it.
        seq.sync_to_beat(4.0, 120.0, 16 * dur); // step_from_beat(4.0) = 16
        seq.tick(16 * dur, &mut sink);

        let new_ons = kick_note_on_abs_steps(&sink, 50, dur);
        assert!(
            new_ons.contains(&16),
            "under Link, the queued pattern must launch at global step 16; got {new_ons:?}"
        );
        // Exactly-once: take_launched reports lane 0 launched.
        // (Drained inside tick via the engine; here we drain manually.)
        let launched = seq.take_launched();
        assert!(
            launched.contains(&0),
            "take_launched must report lane 0 launched; got {launched:?}"
        );
    }

    // ── M6: all-lane scene launch on one boundary (queue_launch_with_state) ──

    /// Queuing every lane with the SAME quant fires them ALL on ONE boundary step:
    /// none switch before it, and each new pattern's local-0 fires together at step 16.
    #[test]
    fn scene_all_lanes_launch_on_one_boundary() {
        let dur = step_dur_micros(120.0);
        // Two drum lanes: lane 0 plays 36, lane 1 plays 40 (each on local step 0).
        let mut seq = Sequencer::new(set_with(vec![
            drum_lane_step0_note(36),
            drum_lane_step0_note(40),
        ]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let st = |mute| LaunchState {
            mute,
            solo: false,
            transpose: 0,
            octave: 0,
        };
        let mut now = 0u64;
        while now <= 17 * dur {
            if now == 3 * dur {
                // Queue BOTH lanes for NextBar at the same moment (note 50 and 51).
                seq.queue_launch_with_state(
                    0,
                    drum_pattern_step0_note(50),
                    Quant::NextBar,
                    st(false),
                );
                seq.queue_launch_with_state(
                    1,
                    drum_pattern_step0_note(51),
                    Quant::NextBar,
                    st(false),
                );
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // Neither new pattern fired before the bar boundary (step 16).
        let early = sink
            .events
            .iter()
            .filter(|(at, m)| {
                *at < 16 * dur && matches!(m, MidiMessage::NoteOn { note: 50 | 51, .. })
            })
            .count();
        assert_eq!(early, 0, "no recalled lane may switch before the boundary");
        // BOTH launched together at absolute step 16 (their shared boundary).
        let l0 = kick_note_on_abs_steps(&sink, 50, dur);
        let l1 = kick_note_on_abs_steps(&sink, 51, dur);
        assert!(l0.contains(&16), "lane 0 launches at step 16; got {l0:?}");
        assert!(l1.contains(&16), "lane 1 launches at step 16; got {l1:?}");
    }

    /// The per-lane LaunchState (mute/solo/transpose/octave) is applied AT the launch
    /// instant: a lane queued with `mute=true` goes silent exactly from the boundary on.
    #[test]
    fn scene_applies_mute_at_launch_instant() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_step0_note(36)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Queue at step 2 (NextBar) with mute=true, AFTER step 0's original note sounds.
        let mut now = 0u64;
        while now <= 33 * dur {
            if now == 2 * dur {
                seq.queue_launch_with_state(
                    0,
                    drum_pattern_step0_note(50),
                    Quant::NextBar,
                    LaunchState {
                        mute: true,
                        solo: false,
                        transpose: 0,
                        octave: 0,
                    },
                );
            }
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // BEFORE the boundary the original pattern (note 36) sounded at step 0.
        let orig = kick_note_on_abs_steps(&sink, 36, dur);
        assert!(
            orig.contains(&0),
            "original sounded before launch; got {orig:?}"
        );
        // AFTER the launch (mute=true applied at the instant) the NEW pattern is silent —
        // note 50 never fires (it would at abs 16/32 if audible).
        let new_ons = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 50, .. }))
            .count();
        assert_eq!(
            new_ons, 0,
            "muted-at-launch lane must be silent after the boundary"
        );
    }

    /// NOTE-OWNERSHIP: an all-lane recall releases each lane's previously-sounding note
    /// at the boundary (no hung notes). After the launch + new pattern, the sounding
    /// registry holds only the new pattern's notes — the old ones were released.
    #[test]
    fn scene_recall_does_not_hang_notes() {
        let dur = step_dur_micros(120.0);
        // Lane 0: a slide-HELD melodic note (off_at=None) — the worst case for hangs.
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_slide_held()]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // held note (pitch 45, ch1) sounds, no NoteOff yet
        assert_eq!(seq.sounding_count(), 1, "held note is sounding");
        // Recall: queue a drum pattern (note 50) with default state on NextBar.
        seq.queue_launch_with_state(
            0,
            drum_pattern_step0_note(50),
            Quant::NextBar,
            LaunchState {
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
            },
        );
        let mut now = 1_000u64;
        while now <= 16 * dur + dur {
            seq.tick(now, &mut sink);
            now += 1_000;
        }
        // The held melodic note got a NoteOff at/after the boundary (released on relaunch).
        assert!(
            sink.events.iter().any(|(at, m)| *at >= 16 * dur
                && *m
                    == MidiMessage::NoteOff {
                        channel: 1,
                        note: 45
                    }),
            "recall must release the held note at the boundary (no hang)"
        );
        // No leftover melodic note: the registry only holds the drum hit's brief note, and
        // after its gate the lane has at most the new pattern's notes — never pitch 45.
        assert!(
            !seq.sounding.iter().any(|s| s.note == 45),
            "the old held note must not remain in the sounding registry"
        );
    }

    // ── Per-drum-voice mute tests (§2.6) ────────────────────────────────────

    /// Build a drum lane with two voices: kick (36) on step 0, hat (42) on step 0.
    fn drum_lane_kick_and_hat() -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        steps[0].push(DrumHit {
            note: 42,
            vel: 80,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "kh".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// A lane with kick (36) muted: note 36 must not fire, hat (42) must fire.
    #[test]
    fn muted_drum_voice_is_silent() {
        let mut lane = drum_lane_kick_and_hat();
        lane.muted_voices = vec![36];
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        let dur = step_dur_micros(120.0);
        run(&mut seq, &mut sink, dur * 4, 1000);

        let has_kick_on = sink
            .events
            .iter()
            .any(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }));
        let has_hat_on = sink
            .events
            .iter()
            .any(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 42, .. }));
        assert!(!has_kick_on, "muted kick (36) must not produce a NoteOn");
        assert!(has_hat_on, "unmuted hat (42) must still fire");
    }

    /// Empty muted_voices → event stream identical to baseline (regression guard).
    #[test]
    fn unmuted_voices_unchanged() {
        // Baseline: kick lane, no mutes.
        let mut seq_base = Sequencer::new(set_with(vec![drum_lane_kick_and_hat()]));
        let mut sink_base = RecordingSink::new();
        seq_base.play(0);
        let dur = step_dur_micros(120.0);
        run(&mut seq_base, &mut sink_base, dur * 4, 1000);

        // Same lane with explicit empty muted_voices.
        let mut lane = drum_lane_kick_and_hat();
        lane.muted_voices = Vec::new();
        let mut seq_mut = Sequencer::new(set_with(vec![lane]));
        let mut sink_mut = RecordingSink::new();
        seq_mut.play(0);
        run(&mut seq_mut, &mut sink_mut, dur * 4, 1000);

        assert_eq!(
            sink_base.events, sink_mut.events,
            "empty muted_voices must produce byte-identical event stream"
        );
    }

    /// set_voice_mute(on=true) releases a currently-sounding note and removes it from registry.
    #[test]
    fn set_voice_mute_releases_sounding_note() {
        let mut seq = Sequencer::new(set_with(vec![drum_lane_kick_and_hat()]));
        let mut sink = RecordingSink::new();
        // Manually inject a sounding note for lane 0, note 36 on channel 9.
        inject_sounding(&mut seq, 9, 36, 0, NoteDomain::Playback);
        assert_eq!(seq.sounding_count(), 1);

        // Mute note 36 on lane 0 at t=1000.
        seq.set_voice_mute(0, 36, true, 1000, &mut sink);

        // Registry must no longer contain note 36.
        assert_eq!(
            seq.sounding_count(),
            0,
            "sounding registry must be empty after voice mute"
        );
        // A NoteOff must have been emitted.
        let has_note_off = sink.events.iter().any(|(at, m)| {
            *at == 1000
                && matches!(
                    m,
                    MidiMessage::NoteOff {
                        channel: 9,
                        note: 36
                    }
                )
        });
        assert!(
            has_note_off,
            "set_voice_mute must emit NoteOff for sounding note; got {:?}",
            sink.events
        );
        // The note must be in muted_voices now.
        assert!(seq.set.lanes[0].muted_voices.contains(&36));
    }

    /// set_voice_mute(on=false) removes note from muted_voices, no NoteOff emitted.
    #[test]
    fn set_voice_mute_unmute_removes_from_list() {
        let mut lane = drum_lane_kick_and_hat();
        lane.muted_voices = vec![36];
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();

        seq.set_voice_mute(0, 36, false, 0, &mut sink);

        assert!(
            !seq.set.lanes[0].muted_voices.contains(&36),
            "note must be removed from muted_voices"
        );
        assert!(sink.events.is_empty(), "unmute must not emit any MIDI");
    }

    // --- M5b Task 3: polyphonic (chord) melodic steps -------------------

    /// Build an S1 (poly) melodic lane from explicit per-step note Vecs, so a step
    /// can hold a CHORD (>1 note). This bypasses the edit-layer mono guard (Task 2)
    /// by constructing the model directly — the scheduler must play whatever the
    /// model holds.
    fn poly_lane_from_steps(steps: Vec<Vec<MelodicNote>>) -> Lane {
        let len = steps.len();
        let steps: Vec<MelodicStep> = steps.into_iter().map(MelodicStep::from).collect();
        Lane {
            profile: S1,
            pattern: Pattern {
                name: "poly".to_string(),
                desc: String::new(),
                length: len,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    fn plain_note(semi: i8) -> MelodicNote {
        MelodicNote {
            semi,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }
    }

    /// A 3-note chord step on a poly lane emits 3 NoteOns at the step time and
    /// 3 matching NoteOffs at/after the gate — one per chord note.
    #[test]
    fn poly_step_emits_all_notes() {
        let dur = step_dur_micros(120.0);
        // step 0: chord of semis 0, 4, 7 (S1 root 48 → notes 48, 52, 55).
        let chord = vec![plain_note(0), plain_note(4), plain_note(7)];
        let lane = poly_lane_from_steps(vec![chord, Vec::new(), Vec::new(), Vec::new()]);
        let root = lane.profile.root_note;
        let expected: Vec<u8> = [0i8, 4, 7]
            .iter()
            .map(|&s| (root as i8 + s) as u8)
            .collect();

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, 4 * dur, 1_000);

        for &note in &expected {
            // NoteOn at step time (0) on the S1 channel.
            assert!(
                sink.events.iter().any(|(t, m)| *t == 0
                    && *m
                        == MidiMessage::NoteOn {
                            channel: 0,
                            note,
                            vel: 100
                        }),
                "chord note {note} must have a NoteOn at t=0; got {:?}",
                sink.events
            );
            // matching NoteOff at/after the gate (len 1.0 * dur).
            let off_at = note_len_micros(1.0, dur);
            assert!(
                sink.events
                    .iter()
                    .any(|(t, m)| *t >= off_at && *m == MidiMessage::NoteOff { channel: 0, note }),
                "chord note {note} must have a NoteOff at/after gate; got {:?}",
                sink.events
            );
        }
    }

    /// A 1-note step on a (poly-capable) lane plays IDENTICALLY to the pre-T3
    /// single-note path, including slide/legato to the next 1-note step. This is
    /// the mono regression guard: replicates `slide_note_on_precedes_prior_note_off`
    /// but built via the direct (poly) model path to prove the single-note branch
    /// is byte-identical.
    #[test]
    fn mono_step_unchanged() {
        let dur = step_dur_micros(120.0);
        // step 0: note A (semi 0, no slide). step 1: note B (semi 5, slide=true).
        let mut a = plain_note(0);
        a.slide = false;
        let mut b = plain_note(5);
        b.slide = true;
        let lane = poly_lane_from_steps(vec![vec![a], vec![b], Vec::new(), Vec::new()]);
        let root = lane.profile.root_note; // S1 root
        let note_a = root; // semi 0
        let note_b = (root as i8 + 5) as u8; // semi 5

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run steps 0..=3 only (stop before the pattern loops back to step 0 at
        // 4*dur), so note A fires exactly once at step 0.
        run(&mut seq, &mut sink, 3 * dur, 1_000);

        // Exactly one NoteOn + one NoteOff per note (single-note path, no extras).
        let count = |target: &MidiMessage| sink.events.iter().filter(|(_, m)| m == target).count();
        assert_eq!(
            count(&MidiMessage::NoteOn {
                channel: 0,
                note: note_a,
                vel: 100
            }),
            1,
            "A must fire exactly one NoteOn"
        );
        assert_eq!(
            count(&MidiMessage::NoteOff {
                channel: 0,
                note: note_a
            }),
            1,
            "A must have exactly one NoteOff"
        );

        // Slide/legato: A is released only after B sounds.
        let a_off = sink
            .events
            .iter()
            .find(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: 0,
                    note: note_a,
                }
            })
            .map(|(t, _)| *t)
            .expect("A must have a NoteOff");
        let b_on = sink
            .events
            .iter()
            .find(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: 0,
                    note: note_b,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .expect("B must have a NoteOn");
        assert!(a_off >= b_on, "legato: A off ({a_off}) >= B on ({b_on})");
    }

    /// After a 3-note chord NoteOn, the stop/release path emits a NoteOff for ALL
    /// 3 chord notes — zero hung notes. Covers `release_all` (via `stop`) plus the
    /// per-lane `release_lanes` stop path.
    #[test]
    fn release_all_clears_chord_notes() {
        // Long chord (len 4.0) still sounding when we stop.
        let chord = vec![
            {
                let mut n = plain_note(0);
                n.len = 4.0;
                n
            },
            {
                let mut n = plain_note(4);
                n.len = 4.0;
                n
            },
            {
                let mut n = plain_note(7);
                n.len = 4.0;
                n
            },
        ];
        let lane = poly_lane_from_steps(vec![chord, Vec::new(), Vec::new(), Vec::new()]);
        let root = lane.profile.root_note;
        let expected: Vec<u8> = [0i8, 4, 7]
            .iter()
            .map(|&s| (root as i8 + s) as u8)
            .collect();

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // emit the 3 chord NoteOns

        // All 3 are sounding (none hung / un-tracked).
        assert_eq!(
            seq.sounding.len(),
            3,
            "all 3 chord notes must be registered"
        );

        seq.stop(1_000, &mut sink);
        assert!(!seq.is_playing());

        for &note in &expected {
            assert!(
                sink.events
                    .iter()
                    .any(|(t, m)| *t >= 1_000 && *m == MidiMessage::NoteOff { channel: 0, note }),
                "stop must release chord note {note}; got {:?}",
                sink.events
            );
        }
        assert!(seq.sounding.is_empty(), "registry must be empty after stop");
    }

    /// `release_lanes` (the per-lane stop path used by launch/replace) must also
    /// release every chord note on the targeted lane — no hang.
    #[test]
    fn release_lanes_clears_chord_notes() {
        let chord = vec![plain_note(0), plain_note(4), plain_note(7)];
        let lane = poly_lane_from_steps(vec![chord, Vec::new(), Vec::new(), Vec::new()]);
        let root = lane.profile.root_note;
        let expected: Vec<u8> = [0i8, 4, 7]
            .iter()
            .map(|&s| (root as i8 + s) as u8)
            .collect();

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink);
        assert_eq!(seq.sounding.len(), 3);

        seq.release_lanes(&[0], 2_000, &mut sink);
        for &note in &expected {
            assert!(
                sink.events
                    .iter()
                    .any(|(t, m)| *t >= 2_000 && *m == MidiMessage::NoteOff { channel: 0, note }),
                "release_lanes must release chord note {note}; got {:?}",
                sink.events
            );
        }
        assert!(
            seq.sounding.iter().all(|s| s.lane != 0),
            "lane 0 chord notes must be cleared from the registry"
        );
    }

    // --- M5b hardening: mono-slide → chord-onset release -------------------

    /// When a slide-held mono note is followed by a chord step, the held note
    /// must receive a NoteOff exactly at the chord's onset (it is NOT carried
    /// into the chord), and all 3 chord NoteOns must fire at that same time.
    #[test]
    fn mono_slide_note_released_at_chord_onset() {
        let dur = step_dur_micros(120.0);
        // S1: channel 0, root_note 45.
        // step 0: single note, semi 0, slide=true  → held into next step.
        // step 1: 3-note chord, semis 0/4/7        → onset at `dur`.
        let slid = MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: true,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        };
        let chord = vec![plain_note(0), plain_note(4), plain_note(7)];
        let lane = poly_lane_from_steps(vec![vec![slid], chord, Vec::new(), Vec::new()]);
        let root = lane.profile.root_note; // 45
        let ch = lane.profile.channel; // 0
        let slid_pitch = root; // semi 0 → note 45
        let chord_pitches: Vec<u8> = [0i8, 4, 7]
            .iter()
            .map(|&s| (root as i8 + s) as u8)
            .collect();

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run through step 0 and step 1 (stop before step 2 = 2*dur).
        run(&mut seq, &mut sink, 2 * dur - 1, 1_000);

        // The slid note's NoteOff must appear at exactly the chord's onset (dur).
        let slid_off_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: ch,
                    note: slid_pitch,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert!(
            slid_off_times.contains(&dur),
            "slid mono note must be released at chord onset (t={}); NoteOffs at {:?}",
            dur,
            slid_off_times
        );

        // All 3 chord NoteOns must fire at the chord's onset (dur).
        for &note in &chord_pitches {
            assert!(
                sink.events.iter().any(|(t, m)| *t == dur
                    && *m
                        == MidiMessage::NoteOn {
                            channel: ch,
                            note,
                            vel: 100
                        }),
                "chord note {} must have NoteOn at t={}; events={:?}",
                note,
                dur,
                sink.events
            );
        }
    }

    // --- M5b hardening: chord + ratchet ------------------------------------

    /// A chord step where one note carries ratchet=2 must retrigger that note
    /// twice within the step while the non-ratcheted companion fires once.
    /// Counts: ratcheted note → 2 NoteOns + 2 NoteOffs; plain note → 1+1.
    #[test]
    fn chord_step_honors_per_note_ratchet() {
        let dur = step_dur_micros(120.0);
        // S1: channel 0, root_note 45.
        // note A: semi 0, ratchet 2, len 0.5  → 2 retriggers within the step.
        //   len=0.5 keeps ratchet_gate = 0.5 * sub well inside the step window
        //   so both NoteOffs flush before dur (avoids the boundary at dur exactly).
        // note B: semi 4, ratchet 1  → 1 hit (baseline).
        let note_a = MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 2,
            micro: 0,
            cond: TrigCond::Always,
        };
        let note_b = MelodicNote {
            semi: 4,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        };
        let lane = poly_lane_from_steps(vec![
            vec![note_a, note_b],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ]);
        let root = lane.profile.root_note; // 45
        let ch = lane.profile.channel; // 0
        let pitch_a = root; // semi 0 → 45
        let pitch_b = (root as i8 + 4) as u8; // semi 4 → 49

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run through step 0 and into step 1 (which is a rest) so that all
        // NoteOffs scheduled at or before `dur` are flushed. Step 1 emits
        // nothing for pitch_a or pitch_b, so the counts remain exact.
        run(&mut seq, &mut sink, dur + 1_000, 1_000);

        // note A (ratchet 2): exactly 2 NoteOns and 2 NoteOffs.
        let a_ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: ch,
                    note: pitch_a,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        let a_offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: ch,
                    note: pitch_a,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            a_ons.len(),
            2,
            "ratchet-2 note must fire 2 NoteOns; got {:?}",
            a_ons
        );
        assert_eq!(
            a_offs.len(),
            2,
            "ratchet-2 note must fire 2 NoteOffs; got {:?}",
            a_offs
        );

        // The two ratchet hits are evenly spaced: sub = dur / 2.
        let sub = dur / 2;
        assert_eq!(
            a_ons,
            vec![0, sub],
            "ratchet NoteOns must be at t=0 and t=sub"
        );

        // Each ratchet NoteOff follows its NoteOn by gate = note.len.min(1.0) * sub.
        // note_a.len = 0.5, so gate = 0.5 * sub = 31_250 µs.
        let ratchet_gate = note_len_micros(0.5_f32.min(1.0), sub);
        assert_eq!(
            a_offs,
            vec![ratchet_gate, sub + ratchet_gate],
            "ratchet NoteOffs must follow each NoteOn by one ratchet gate"
        );

        // note B (ratchet 1): exactly 1 NoteOn + 1 NoteOff.
        let b_ons = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: ch,
                    note: pitch_b,
                    vel: 100,
                }
            })
            .count();
        let b_offs = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOff {
                    channel: ch,
                    note: pitch_b,
                }
            })
            .count();
        assert_eq!(b_ons, 1, "plain chord note must fire exactly 1 NoteOn");
        assert_eq!(b_offs, 1, "plain chord note must fire exactly 1 NoteOff");
    }

    // ── M8 Task 4: signed microtiming ────────────────────────────────────────

    /// Helper: a one-step drum lane with a single hit at step 0, with `micro` set.
    fn drum_lane_micro(note: u8, micro: i16) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 1];
        steps[0].push(DrumHit {
            note,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "micro_drum".to_string(),
                desc: String::new(),
                length: 1,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// Helper: a one-step drum lane with ratchet=R and `micro` set.
    fn drum_lane_micro_ratchet(note: u8, micro: i16, ratchet: u8) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 1];
        steps[0].push(DrumHit {
            note,
            vel: 100,
            prob: 1.0,
            ratchet,
            micro,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "micro_ratchet_drum".to_string(),
                desc: String::new(),
                length: 1,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// Helper: a one-step melodic lane with `micro` set.
    fn melodic_lane_micro(semi: i8, micro: i16) -> Lane {
        let note = MelodicNote {
            semi,
            vel: 1.0,
            prob: 1.0,
            ratchet: 1,
            len: 0.5,
            slide: false,
            micro,
            cond: TrigCond::Always,
        };
        melodic_lane(vec![Some(note)], false)
    }

    /// Helper: a one-step melodic lane with ratchet and `micro`.
    fn melodic_lane_micro_ratchet(semi: i8, micro: i16, ratchet: u8) -> Lane {
        let note = MelodicNote {
            semi,
            vel: 1.0,
            prob: 1.0,
            ratchet,
            len: 0.5,
            slide: false,
            micro,
            cond: TrigCond::Always,
        };
        melodic_lane(vec![Some(note)], false)
    }

    // ── drum microtiming ────────────────────────────────────────────────────

    #[test]
    fn drum_micro_positive_shifts_noteon_and_noteoff() {
        let dur = step_dur_micros(120.0); // 125_000
        let micro_val: i16 = 1_000; // +1 ms
        let mut seq = Sequencer::new(set_with(vec![drum_lane_micro(36, micro_val)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run just under 1 step so only step 0 fires (not its repeat at t=dur).
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![micro_val as u64],
            "NoteOn must shift by +micro"
        );

        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let off_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOff { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            off_times,
            vec![micro_val as u64 + gate],
            "NoteOff must shift by the same +micro"
        );
    }

    #[test]
    fn drum_micro_negative_shifts_noteon_and_noteoff() {
        let dur = step_dur_micros(120.0); // 125_000
        let micro_val: i16 = -2_000; // -2 ms
        let mut seq = Sequencer::new(set_with(vec![drum_lane_micro(36, micro_val)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // step 0 is at swung=0; with micro=-2000 the on_at would be negative → clamped to 0
        // But step 0 is at absolute 0, so negative micro on step 0 clamps to 0.
        // Use step 1 offset: run TWO steps and check step-1 timing.
        // Actually the lane is 1 step long so step 0 repeats at t=dur, t=2*dur, …
        // At repeat on_at = dur + micro_val (which is positive for small micro).
        run(&mut seq, &mut sink, 2 * dur - 1, 500);

        // Second firing (step repetition at t=dur) should be at dur + micro_val.
        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        // First hit at t=0: swung=0, micro=-2000 → clamped to 0 (can't go negative).
        // Second hit at t=dur: swung=dur, micro=-2000 → dur - 2000.
        assert_eq!(
            on_times,
            vec![0, dur - 2_000],
            "second NoteOn must shift by -micro; first is clamped at 0"
        );
    }

    #[test]
    fn drum_micro_zero_is_unchanged() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_micro(36, 0)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(on_times, vec![0], "micro=0 must not change timing");
    }

    #[test]
    fn drum_micro_clamped_beyond_half_step() {
        // At 600 BPM: dur = 25_000; half = 12_500. i16::MAX = 32_767 > 12_500 → clamped.
        let bpm = 600.0_f64;
        let fast_dur = step_dur_micros(bpm); // 25_000
        let half = fast_dur / 2; // 12_500
        let mut set = set_with(vec![drum_lane_micro(36, i16::MAX)]);
        set.bpm = bpm;
        let mut seq = Sequencer::new(set);
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, fast_dur + 1, 200);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![half],
            "micro beyond +½ step must clamp to +½ step"
        );
    }

    #[test]
    fn drum_ratchet_all_sub_events_shift_by_micro() {
        let dur = step_dur_micros(120.0); // 125_000
        let micro_val: i16 = 5_000;
        let ratchet = 2u8;
        let sub = dur / ratchet as u64; // 62_500
        let mut seq = Sequencer::new(set_with(vec![drum_lane_micro_ratchet(
            36, micro_val, ratchet,
        )]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        let expected_on = vec![
            micro_val as u64,       // sub-hit 0: 0 + micro
            sub + micro_val as u64, // sub-hit 1: sub + micro
        ];
        assert_eq!(
            on_times, expected_on,
            "all ratchet NoteOns must shift by micro"
        );

        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, sub);
        let off_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOff { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();
        let expected_off = vec![micro_val as u64 + gate, sub + micro_val as u64 + gate];
        assert_eq!(
            off_times, expected_off,
            "all ratchet NoteOffs must shift by micro"
        );
    }

    // ── melodic microtiming ─────────────────────────────────────────────────

    #[test]
    fn melodic_micro_positive_shifts_noteon_and_noteoff() {
        let dur = step_dur_micros(120.0);
        let micro_val: i16 = 3_000;
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_micro(0, micro_val)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![micro_val as u64],
            "melodic NoteOn must shift by +micro"
        );

        let off_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOff { .. }))
            .map(|(t, _)| *t)
            .collect();
        // len=0.5 → off = on_at + note_len_micros(0.5, dur)
        let expected_off = micro_val as u64 + note_len_micros(0.5, dur);
        assert_eq!(
            off_times,
            vec![expected_off],
            "melodic NoteOff must shift by same +micro"
        );
    }

    #[test]
    fn melodic_micro_zero_is_unchanged() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_micro(0, 0)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(on_times, vec![0], "micro=0 must not change melodic timing");
    }

    #[test]
    fn melodic_micro_clamped_beyond_half_step() {
        // At 600 BPM dur=25_000; half=12_500; i16::MAX=32_767 → clamped to 12_500.
        let bpm = 600.0_f64;
        let fast_dur = step_dur_micros(bpm);
        let half = fast_dur / 2;
        let mut set = set_with(vec![melodic_lane_micro(0, i16::MAX)]);
        set.bpm = bpm;
        let mut seq = Sequencer::new(set);
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, fast_dur + 1, 200);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![half],
            "melodic micro beyond +½ step must clamp"
        );
    }

    #[test]
    fn melodic_ratchet_all_sub_events_shift_by_micro() {
        let dur = step_dur_micros(120.0);
        let micro_val: i16 = 4_000;
        let ratchet = 2u8;
        let sub = dur / ratchet as u64;
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_micro_ratchet(
            0, micro_val, ratchet,
        )]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();
        let expected_on = vec![micro_val as u64, sub + micro_val as u64];
        assert_eq!(
            on_times, expected_on,
            "all melodic ratchet NoteOns must shift by micro"
        );
    }

    // ── chord microtiming ────────────────────────────────────────────────────

    fn chord_lane_micro(micro_a: i16, micro_b: i16) -> Lane {
        // Two-note chord: semis 0 and 4, each with their own micro.
        let note_a = MelodicNote {
            semi: 0,
            vel: 1.0,
            prob: 1.0,
            ratchet: 1,
            len: 0.5,
            slide: false,
            micro: micro_a,
            cond: TrigCond::Always,
        };
        let note_b = MelodicNote {
            semi: 4,
            vel: 1.0,
            prob: 1.0,
            ratchet: 1,
            len: 0.5,
            slide: false,
            micro: micro_b,
            cond: TrigCond::Always,
        };
        let steps = vec![crate::pattern::model::MelodicStep::from(vec![
            note_a, note_b,
        ])];
        Lane {
            profile: S1,
            pattern: Pattern {
                name: "chord_micro".to_string(),
                desc: String::new(),
                length: 1,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    #[test]
    fn chord_micro_per_note_shifts_independently() {
        use crate::devices::profiles::resolve_melodic_pitch;
        let dur = step_dur_micros(120.0);
        let micro_a: i16 = 2_000;
        let micro_b: i16 = -1_000;
        let mut seq = Sequencer::new(set_with(vec![chord_lane_micro(micro_a, micro_b)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let root = S1.root_note;
        let pitch_a = resolve_melodic_pitch(root, 0, 0, 0);
        let pitch_b = resolve_melodic_pitch(root, 4, 0, 0);
        let ch = S1.channel;

        let on_a: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: ch,
                    note: pitch_a,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();
        let on_b: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                *m == MidiMessage::NoteOn {
                    channel: ch,
                    note: pitch_b,
                    vel: 100,
                }
            })
            .map(|(t, _)| *t)
            .collect();

        assert_eq!(
            on_a,
            vec![micro_a as u64],
            "chord note A must shift by micro_a"
        );
        // micro_b = -1000; swung=0 → on_at = (0 - 1000).max(0) = 0 (clamped)
        assert_eq!(
            on_b,
            vec![0],
            "chord note B with negative micro at t=0 must clamp to 0"
        );
    }

    #[test]
    fn chord_micro_zero_unchanged() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![chord_lane_micro(0, 0)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();
        // Both notes at t=0
        assert_eq!(on_times, vec![0, 0], "chord micro=0 must not shift timing");
    }

    // ── M8 Task 5: per-step CC locks ─────────────────────────────────────────

    use crate::pattern::model::CcLock;

    /// Build a drum lane whose step 0 has a single kick hit AND a CC74=64 lock.
    fn drum_lane_cc(cc: u8, val: u8) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 1];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let pattern = Pattern {
            name: "cc_drum".to_string(),
            desc: String::new(),
            length: 1,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: vec![vec![CcLock { cc, val }]],
        };
        // make sure step_cc is consistent
        assert_eq!(pattern.step_cc(0), &[CcLock { cc, val }]);
        Lane {
            profile: T8_DRUMS,
            pattern,
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// Build a melodic lane whose step 0 has a note AND a CC74=64 lock.
    fn melodic_lane_cc(cc: u8, val: u8) -> Lane {
        let note = MelodicNote {
            semi: 0,
            vel: 1.0,
            prob: 1.0,
            ratchet: 1,
            len: 0.5,
            slide: false,
            micro: 0,
            cond: TrigCond::Always,
        };
        let steps = vec![MelodicStep::from(vec![note])];
        Lane {
            profile: S1,
            pattern: Pattern {
                name: "cc_mel".to_string(),
                desc: String::new(),
                length: 1,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: vec![vec![CcLock { cc, val }]],
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// CC is enqueued just before the step's NoteOn (at_micros < NoteOn time or == 0).
    #[test]
    fn cc_lock_enqueued_before_noteon_drum() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_cc(74, 64)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let cc_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::ControlChange { controller: 74, .. }))
            .map(|(t, _)| *t)
            .collect();
        let note_on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 36, .. }))
            .map(|(t, _)| *t)
            .collect();

        assert_eq!(cc_times.len(), 1, "exactly one CC74 should be sent");
        assert_eq!(note_on_times.len(), 1, "exactly one NoteOn for kick");
        // CC must arrive at or before the NoteOn (saturating_sub(1) at t=0 stays 0).
        assert!(cc_times[0] <= note_on_times[0], "CC must be <= NoteOn time");
    }

    /// CC is enqueued just before the step's NoteOn for a melodic lane.
    #[test]
    fn cc_lock_enqueued_before_noteon_melodic() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![melodic_lane_cc(74, 64)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);

        let cc_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::ControlChange { controller: 74, .. }))
            .map(|(t, _)| *t)
            .collect();
        let note_on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { .. }))
            .map(|(t, _)| *t)
            .collect();

        assert_eq!(cc_times.len(), 1, "exactly one CC74 should be sent");
        assert_eq!(note_on_times.len(), 1, "exactly one NoteOn");
        assert!(cc_times[0] <= note_on_times[0], "CC must be <= NoteOn time");
    }

    /// Identical consecutive CC value to the same route is suppressed (only one send).
    #[test]
    fn cc_lock_identical_value_suppressed_on_repeat() {
        let dur = step_dur_micros(120.0);
        // 2-step pattern: both steps have CC74=64 and a kick.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 2];
        for s in &[0usize, 1] {
            steps[*s].push(DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "cc_repeat".to_string(),
                desc: String::new(),
                length: 2,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                // Both steps: CC74=64
                cc: vec![
                    vec![CcLock { cc: 74, val: 64 }],
                    vec![CcLock { cc: 74, val: 64 }],
                ],
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run for 2 full steps.
        run(&mut seq, &mut sink, dur * 2 + 1, 500);

        let cc_events: Vec<_> = sink
            .events
            .iter()
            .filter(|(_, m)| {
                matches!(
                    m,
                    MidiMessage::ControlChange {
                        controller: 74,
                        value: 64,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            cc_events.len(),
            1,
            "identical consecutive CC value must be suppressed — only 1 send expected, got {}: {:?}",
            cc_events.len(),
            cc_events
        );
    }

    /// A value change on the same CC/route re-sends.
    #[test]
    fn cc_lock_value_change_resends() {
        let dur = step_dur_micros(120.0);
        // 2-step pattern: step 0 CC74=64, step 1 CC74=80.
        // Run for exactly 2 steps (0 and 1) — stop before step 2 (the loop-back).
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 2];
        for s in &[0usize, 1] {
            steps[*s].push(DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            });
        }
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "cc_change".to_string(),
                desc: String::new(),
                length: 2,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: vec![
                    vec![CcLock { cc: 74, val: 64 }],
                    vec![CcLock { cc: 74, val: 80 }],
                ],
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run just past step 1 (2*dur - 1) so both steps fire but the loop-back step 2 does not.
        run(&mut seq, &mut sink, dur * 2 - 1, 500);

        let cc_vals: Vec<u8> = sink
            .events
            .iter()
            .filter_map(|(_, m)| {
                if let MidiMessage::ControlChange {
                    controller: 74,
                    value,
                    ..
                } = m
                {
                    Some(*value)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            cc_vals,
            vec![64, 80],
            "both CC values must be sent when they differ"
        );
    }

    /// After Stop the cache clears, so the next play re-sends all CC locks.
    #[test]
    fn cc_lock_cache_cleared_on_stop_resends_on_next_play() {
        let dur = step_dur_micros(120.0);
        let mut seq = Sequencer::new(set_with(vec![drum_lane_cc(74, 64)]));
        let mut sink = RecordingSink::new();

        // First play: CC74=64 is sent once.
        seq.play(0);
        run(&mut seq, &mut sink, dur - 1, 500);
        let first_count = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::ControlChange { controller: 74, .. }))
            .count();
        assert_eq!(first_count, 1, "first play must send CC74 once");

        // Stop clears the cache.
        seq.stop(dur, &mut sink);
        sink.events.clear();

        // Second play: CC74=64 must be re-sent (cache was cleared).
        seq.play(dur * 2);
        run(&mut seq, &mut sink, dur * 2 + dur - 1, 500);
        let second_count = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::ControlChange { controller: 74, .. }))
            .count();
        assert_eq!(second_count, 1, "second play after stop must re-send CC74");
    }

    /// CC does not perturb the note-ownership registry — sounding count is unaffected.
    #[test]
    fn cc_lock_does_not_affect_note_registry() {
        // step_dur_micros(120.0) = 125_000. Kick gate = 0.1 * 125_000 = 12_500.
        let mut seq = Sequencer::new(set_with(vec![drum_lane_cc(74, 64)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run to t=50_000: the kick NoteOff (at 12_500) is flushed,
        // but step 1 (at t=125_000) has not fired yet.
        // During this window the sounding registry must be empty (CC never registers).
        run(&mut seq, &mut sink, 50_000, 500);

        assert_eq!(
            seq.sounding_count(),
            0,
            "CC must not add anything to the sounding registry"
        );
    }

    // --- trig-condition gate tests (T6) ----------------------------------

    fn drum_lane_single_hit_cond(cond: TrigCond) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "cond".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    fn melodic_lane_single_note_cond(cond: TrigCond) -> Lane {
        let note = MelodicNote {
            semi: 0,
            vel: 1.0,
            len: 0.5,
            slide: false,
            ratchet: 1,
            micro: 0,
            prob: 1.0,
            cond,
        };
        let steps: Vec<MelodicStep> = vec![
            MelodicStep::from(vec![note]),
            MelodicStep::from(vec![]),
            MelodicStep::from(vec![]),
            MelodicStep::from(vec![]),
        ];
        Lane {
            profile: S1,
            pattern: Pattern {
                name: "cond_mel".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    fn count_note_ons_for(sink: &RecordingSink, note: u8) -> usize {
        sink.events
            .iter()
            .filter(|(_, msg)| matches!(msg, MidiMessage::NoteOn { note: n, .. } if *n == note))
            .count()
    }

    fn count_any_note_ons(sink: &RecordingSink) -> usize {
        sink.events
            .iter()
            .filter(|(_, msg)| matches!(msg, MidiMessage::NoteOn { .. }))
            .count()
    }

    /// Run sequencer for exactly `loops` full loops of a 4-step pattern at 120 BPM.
    /// Stops just before step 0 of the next loop to avoid materializing it.
    fn run_loops(mut seq: Sequencer, loops: usize) -> (Sequencer, RecordingSink) {
        let dur = step_dur_micros(120.0); // 125_000 µs per step
                                          // End midway through the last step of loop N-1, never reaching step 0 of loop N.
        let total = dur * 4 * (loops as u64) - dur / 2;
        let mut sink = RecordingSink::new();
        seq.play(0);
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 4;
        }
        (seq, sink)
    }

    #[test]
    fn trig_gate_always_fires_every_loop() {
        let lane = drum_lane_single_hit_cond(TrigCond::Always);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            4,
            "Always must fire every loop"
        );
    }

    #[test]
    fn trig_gate_ratio_1_2_fires_every_other_loop() {
        let lane = drum_lane_single_hit_cond(TrigCond::Ratio { x: 1, y: 2 });
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        // x=1,y=2: fires when loop_index%2+1==1, i.e. loop_index%2==0 → loops 0,2
        assert_eq!(
            count_note_ons_for(&sink, 60),
            2,
            "Ratio 1:2 must fire on even loops only"
        );
    }

    #[test]
    fn trig_gate_first_fires_only_on_loop_0() {
        let lane = drum_lane_single_hit_cond(TrigCond::First);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            1,
            "First must fire only on loop 0"
        );
    }

    #[test]
    fn trig_gate_fill_fires_only_when_fill_active() {
        let lane = drum_lane_single_hit_cond(TrigCond::Fill);
        let mut seq = Sequencer::new(set_with(vec![lane]));
        seq.set_fill_active(true);
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            4,
            "Fill must fire all loops when fill_active=true"
        );
    }

    #[test]
    fn trig_gate_fill_suppressed_when_fill_inactive() {
        let lane = drum_lane_single_hit_cond(TrigCond::Fill);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            0,
            "Fill must not fire when fill_active=false"
        );
    }

    #[test]
    fn trig_gate_not_fill_fires_when_fill_inactive() {
        let lane = drum_lane_single_hit_cond(TrigCond::NotFill);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            4,
            "NotFill must fire all loops when fill_active=false"
        );
    }

    #[test]
    fn trig_gate_runs_before_prob_check() {
        // First cond + prob=1.0: gate blocks loops 1-3, so exactly 1 NoteOn across 4 loops.
        let lane = drum_lane_single_hit_cond(TrigCond::First);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            1,
            "Gate must block even when prob=1.0"
        );
    }

    #[test]
    fn trig_gate_melodic_ratio_1_2_fires_every_other_loop() {
        let lane = melodic_lane_single_note_cond(TrigCond::Ratio { x: 1, y: 2 });
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_any_note_ons(&sink),
            2,
            "Melodic: Ratio 1:2 must fire every other loop"
        );
    }

    /// Build a melodic lane whose step 0 is a *chord* (≥2 notes), so the
    /// scheduler routes through `materialize_chord_step`.  Both notes carry
    /// the supplied condition.
    fn chord_lane_two_notes_cond(cond: TrigCond) -> Lane {
        let make_note = |semi: i8| MelodicNote {
            semi,
            vel: 1.0,
            len: 0.5,
            slide: false,
            ratchet: 1,
            micro: 0,
            prob: 1.0,
            cond: cond.clone(),
        };
        // Two notes → len() >= 2 → materialize_chord_step path.
        let steps: Vec<MelodicStep> = vec![
            MelodicStep::from(vec![make_note(0), make_note(4)]),
            MelodicStep::from(vec![]),
            MelodicStep::from(vec![]),
            MelodicStep::from(vec![]),
        ];
        Lane {
            profile: S1,
            pattern: Pattern {
                name: "cond_chord".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Melodic(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        }
    }

    /// Chord path (`materialize_chord_step`) honours the trig-condition gate.
    /// Step 0 has 2 notes with Ratio{1,2} → fires on loops 0 and 2 only.
    /// Each firing loop emits 2 NoteOns (one per chord note), giving 4 total.
    #[test]
    fn trig_gate_chord_ratio_fires_every_other_loop() {
        let lane = chord_lane_two_notes_cond(TrigCond::Ratio { x: 1, y: 2 });
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        // 2 notes × 2 firing loops (0, 2) = 4 NoteOns.
        assert_eq!(
            count_any_note_ons(&sink),
            4,
            "Chord path: Ratio 1:2 must fire on loops 0 and 2 only (2 notes × 2 loops = 4)"
        );
    }

    /// `TrigCond::NotFirst` must suppress the step on loop 0 and allow it on
    /// all subsequent loops.  Tested via the drum path for simplicity.
    #[test]
    fn trig_gate_not_first_skips_loop_0_fires_rest() {
        let lane = drum_lane_single_hit_cond(TrigCond::NotFirst);
        let seq = Sequencer::new(set_with(vec![lane]));
        let (_, sink) = run_loops(seq, 4);
        // Loops 1, 2, 3 fire; loop 0 is suppressed.
        assert_eq!(
            count_note_ons_for(&sink, 60),
            3,
            "NotFirst must suppress loop 0 and fire on loops 1-3"
        );
    }

    // ── M8 Task 7: per-lane swing override + clock division ───────────────

    /// Helper: 4-step drum lane with a hit on step 0.
    fn drum_lane_single_hit_swing_div(swing: Option<f32>, clock_div: Option<u8>) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "swing_div".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing,
            clock_div,
        }
    }

    /// A lane with per-lane swing=0.7 produces a DIFFERENT odd-step offset than
    /// the global swing=0.5.  We run two lanes in the same Set: lane 0 uses global
    /// (swing=None), lane 1 overrides to 0.7.  Both have a hit only on step 1 (odd),
    /// so we can compare their NoteOn times.
    ///
    /// Global swing 0.5 → offset = 0 µs on odd steps (straight).
    /// Per-lane swing 0.7 → offset = (0.7-0.5)*125_000*2 = 50_000 µs on odd steps.
    #[test]
    fn per_lane_swing_override_changes_odd_step_timing() {
        let dur = step_dur_micros(120.0); // 125_000 µs

        // Lane 0: hit on step 1 only; uses global swing (0.5 → offset 0).
        let mut steps0: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps0[1].push(DrumHit {
            note: 61,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let lane0 = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "global_sw".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps0),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None, // use global 0.5 → straight
            clock_div: None,
        };

        // Lane 1: same hit on step 1; per-lane swing 0.7 → 50_000 µs offset.
        let mut steps1: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps1[1].push(DrumHit {
            note: 62,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let lane1 = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "lane_sw".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps1),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: Some(0.7), // per-lane override
            clock_div: None,
        };

        // Global swing = 0.5 (straight).
        let set = Set {
            name: "test".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes: vec![lane0, lane1],
            id: crate::persist::Id::nil(),
            scenes: Vec::new(),
            chains: Vec::new(),
            clock_in_port: None,
        };
        let mut seq = Sequencer::new(set);
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run for exactly one bar (4 steps) — stop just before step 4.
        let total = 4 * dur - dur / 4;
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 8;
        }

        // Collect NoteOn times per note.
        let time_for = |note: u8| -> Option<u64> {
            sink.events
                .iter()
                .find(|(_, m)| matches!(m, MidiMessage::NoteOn { note: n, .. } if *n == note))
                .map(|(t, _)| *t)
        };

        let t_global = time_for(61).expect("lane0 (note 61) must have fired");
        let t_per = time_for(62).expect("lane1 (note 62) must have fired");

        // Global swing=0.5 on odd step 1: step_start=dur, offset=0 → fires at dur.
        assert_eq!(
            t_global, dur,
            "global-swing lane must fire straight at step 1"
        );
        // Per-lane swing=0.7: offset = (0.7-0.5)*dur*2 = 50_000 µs.
        let expected_offset = ((0.7_f64 - 0.5) * dur as f64 * 2.0).round() as u64;
        assert_eq!(
            t_per,
            dur + expected_offset,
            "per-lane swing=0.7 must delay odd step by {expected_offset} µs"
        );
        // The two lanes must fire at DIFFERENT times.
        assert_ne!(
            t_global, t_per,
            "per-lane swing must differ from global swing"
        );
    }

    /// A lane with clock_div=None or Some(1) fires every global step (unchanged behavior).
    #[test]
    fn clock_div_none_and_one_fire_every_step() {
        // 4-step pattern, hit on step 0 — runs 4 loops → 4 fires.
        let lane_none = drum_lane_single_hit_swing_div(None, None);
        let seq = Sequencer::new(set_with(vec![lane_none]));
        let (_, sink) = run_loops(seq, 4);
        assert_eq!(
            count_note_ons_for(&sink, 60),
            4,
            "clock_div=None must fire every loop"
        );

        let lane_one = drum_lane_single_hit_swing_div(None, Some(1));
        let seq2 = Sequencer::new(set_with(vec![lane_one]));
        let (_, sink2) = run_loops(seq2, 4);
        assert_eq!(
            count_note_ons_for(&sink2, 60),
            4,
            "clock_div=Some(1) must fire every loop"
        );
    }

    /// A lane with clock_div=Some(2) fires its local step only on every other global
    /// step.  Pattern has 2 steps: hit on local step 0, rest on local step 1.
    /// Global steps: 0,1,2,3,4,5,6,7 (2 loops of a 4-step global tick = 8 global steps
    /// if pattern=2 with div=2: local advances on even globals 0,2,4,6 → 4 advances,
    /// each pair consumes one local cycle).
    ///
    /// Simpler: run 8 global steps with a 2-step pattern and clock_div=2.
    /// Local fires at global 0,2,4,6 → local step 0 at globals 0,4; local step 1 at globals 2,6.
    /// Hit is only on local step 0 → 2 NoteOns expected.
    #[test]
    fn clock_div_2_halves_lane_step_rate() {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 2];
        steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        // steps[1] is empty (rest)
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "div2".to_string(),
                desc: String::new(),
                length: 2,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: Some(2),
        };

        let dur = step_dur_micros(120.0); // 125_000 µs
        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run 8 global steps (just before step 8).
        let total = 8 * dur - dur / 4;
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 8;
        }

        // Local step 0 fires at global steps 0 and 4 → 2 NoteOns.
        assert_eq!(
            count_note_ons_for(&sink, 60),
            2,
            "clock_div=2 with 2-step pattern: local step 0 fires at globals 0,4 → 2 hits"
        );

        // Verify NoteOn times: global step 0 and global step 4.
        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 60, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![0, 4 * dur],
            "NoteOns must be at global steps 0 and 4"
        );
    }

    /// clock_div=4 advances the lane once every 4 global steps (quarter-time).
    /// 4-step pattern, hit on step 0 only.  Run 16 global steps → local step 0
    /// fires at global steps 0,4,8,12 → 4 NoteOns? No: div=4, 4-step pattern.
    /// Local advances at globals 0,4,8,12 → local 0,1,2,3 → hit only at local 0
    /// → 1 NoteOn at global 0.  Run 16 globals: local cycles at 0,4,8,12 (local
    /// 0→3 one cycle), then 16 triggers local 0 again… but let's stay 16 steps.
    /// Globals 0,4,8,12: local 0,1,2,3 → 1 hit on local 0 → 1 NoteOn.
    #[test]
    fn clock_div_4_quarter_time() {
        let dur = step_dur_micros(120.0);

        // 4-step pattern, hit on step 0 only.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "div4".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: Some(4),
        };

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run 16 global steps (just before step 16).
        let total = 16 * dur - dur / 4;
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 8;
        }

        // div=4, 4-step pattern: local advances at globals 0,4,8,12.
        // local 0→hit at global 0; local 1→no hit at global 4; etc. → 1 NoteOn.
        assert_eq!(
            count_note_ons_for(&sink, 60),
            1,
            "clock_div=4 with 4-step pattern: only local step 0 fires in 16 global steps"
        );
        let on_times: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 60, .. }))
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(
            on_times,
            vec![0],
            "clock_div=4 first fire must be at global step 0"
        );
    }

    /// Polymeter coexistence: a divided lane (clock_div=2, 2-step pattern) alongside
    /// a normal lane (4-step).  The normal lane fires on every global step; the divided
    /// lane fires half as often.  Neither should interfere with the other.
    #[test]
    fn clock_div_coexists_with_normal_lane_polymeter() {
        let dur = step_dur_micros(120.0);

        // Normal 4-step lane, hit on step 0 (note 61).
        let mut normal_steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        normal_steps[0].push(DrumHit {
            note: 61,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let normal_lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "normal".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(normal_steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: None,
        };

        // Divided 2-step lane, hit on local step 0 (note 60), clock_div=2.
        let mut div_steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 2];
        div_steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let div_lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "div".to_string(),
                desc: String::new(),
                length: 2,
                data: PatternData::Drums(div_steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: Some(2),
        };

        let mut seq = Sequencer::new(set_with(vec![normal_lane, div_lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run 8 global steps.
        let total = 8 * dur - dur / 4;
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 8;
        }

        // Normal lane (note 61): hits at local step 0 → globals 0,4 → 2 NoteOns.
        assert_eq!(
            count_note_ons_for(&sink, 61),
            2,
            "normal lane must fire at globals 0,4 (step 0 of 4-step pattern)"
        );
        // Divided lane (note 60): local step 0 at globals 0,4 → 2 NoteOns.
        assert_eq!(
            count_note_ons_for(&sink, 60),
            2,
            "divided lane (div=2) local step 0 fires at globals 0,4 → 2 hits"
        );
    }

    /// No hung notes: a clock_div=2 lane that skips odd global steps must not
    /// leave dangling NoteOff events (each NoteOn has a matching NoteOff).
    #[test]
    fn clock_div_no_hung_notes() {
        let dur = step_dur_micros(120.0);

        // 2-step pattern, hit on step 0, clock_div=2.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 2];
        steps[0].push(DrumHit {
            note: 60,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "hung".to_string(),
                desc: String::new(),
                length: 2,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
            swing: None,
            clock_div: Some(2),
        };

        let mut seq = Sequencer::new(set_with(vec![lane]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        // Run 8 global steps + a little extra for NoteOffs to land.
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let total = 8 * dur + gate + dur;
        let mut t = 0u64;
        while t <= total {
            seq.tick(t, &mut sink);
            t += dur / 8;
        }

        let n_ons = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { note: 60, .. }))
            .count();
        let n_offs = sink
            .events
            .iter()
            .filter(|(_, m)| matches!(m, MidiMessage::NoteOff { note: 60, .. }))
            .count();
        assert!(n_ons > 0, "must have some NoteOns");
        assert_eq!(
            n_ons, n_offs,
            "every NoteOn must have a matching NoteOff (no hung notes)"
        );
    }
}
