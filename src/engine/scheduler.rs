//! Step sequencer: pure timing math (this task) plus the stateful `Sequencer`
//! (Task 9). All time is `u64` microseconds on a monotonic timeline.

use crate::midi::ports::MidiSink;
use crate::midi::MidiMessage;
use crate::pattern::model::{Lane, Pattern, PatternData, Set};

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
    /// `(Pattern, Quant)` is applied at the next matching global boundary, exactly once.
    /// Replacing a queued launch overwrites the slot (no stacking).
    queued: Vec<Option<(Pattern, Quant)>>,
    /// Absolute step at which each lane last (re)started its local clock. 0 for a lane
    /// that has never been launched, so `(step - 0) % len` is byte-identical to today's
    /// `step % len`. A launch sets this to the boundary step, restarting the lane at local 0.
    launch_offset: Vec<usize>,
    /// Scratch: lanes that launched during the most recent `tick`. Drained by
    /// `take_launched` so the engine can emit one `Launched` event per lane.
    just_launched: Vec<usize>,
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
        sink.send(msg, at_micros);
    }

    /// Release every currently-sounding note, then send CC123 + CC120 per distinct
    /// channel, and clear the registry (and the legato `active` tracker).
    /// This is the P4 fix: call BEFORE clearing `queue` so queued NoteOffs for
    /// already-flushed NoteOns don't get dropped silently.
    pub fn release_all(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // NoteOff for every sounding note.
        let sounding = std::mem::take(&mut self.sounding);
        for s in &sounding {
            sink.send(
                MidiMessage::NoteOff {
                    channel: s.channel,
                    note: s.note,
                },
                at_micros,
            );
        }
        // CC123 + CC120 per distinct ROUTE channel in the set (the channel notes are
        // actually emitted on), so the sweep clears the same channels that sound.
        let mut sent: Vec<u8> = Vec::new();
        for lane in &self.set.lanes {
            let ch = lane.route_channel();
            if sent.contains(&ch) {
                continue;
            }
            sent.push(ch);
            sink.send(
                MidiMessage::ControlChange {
                    channel: ch,
                    controller: 123,
                    value: 0,
                },
                at_micros,
            );
            sink.send(
                MidiMessage::ControlChange {
                    channel: ch,
                    controller: 120,
                    value: 0,
                },
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
                sink.send(
                    MidiMessage::NoteOff {
                        channel: s.channel,
                        note: s.note,
                    },
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
                sink.send(
                    MidiMessage::NoteOff {
                        channel: s.channel,
                        note: s.note,
                    },
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
        at: u64,
        sink: &mut dyn MidiSink,
    ) {
        let before = sounding.len();
        sounding.retain(|s| !(s.channel == channel && s.note == note));
        if sounding.len() < before {
            sink.send(MidiMessage::NoteOff { channel, note }, at);
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
            Self::release_note(&mut self.sounding, channel, note, at, sink);
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
    }

    /// All-notes-off / all-sound-off live recovery. Releases every sounding note via
    /// the authoritative registry (all domains), sends CC 123 + CC 120 per distinct
    /// lane channel, and clears the registry. Does NOT change `playing`.
    pub fn panic(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // release_all handles NoteOff for every sounding note + CC123/120 per channel
        // + clears active[]. playing is intentionally left unchanged.
        self.release_all(at_micros, sink);
        // Note: `playing` is intentionally left unchanged.
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
    pub fn queue_launch(&mut self, lane: usize, pattern: Pattern, q: Quant) {
        if let Some(slot) = self.queued.get_mut(lane) {
            *slot = Some((pattern, q));
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
    pub fn sync_to_beat(&mut self, beat: f64, bpm: f64) {
        self.set.bpm = bpm;
        let new_step = step_from_beat(beat);
        self.current = new_step;
        // Only move next_step forward; never re-materialize already-emitted steps
        // and never rewind on a backward jump.
        if new_step > self.next_step {
            self.next_step = new_step;
            // Re-anchor the accumulated clock to (new_step - 1)'s fire time so the
            // next tick fires only new_step (no back-fill of the skipped steps).
            // `step_from_beat(beat) >= 1` here since new_step > next_step >= 0, so
            // `new_step - 1` does not underflow; we still saturate defensively.
            let dur = step_dur_micros(self.set.bpm);
            let prev_step = new_step.saturating_sub(1) as u64;
            self.last_step_at = Some(self.origin_micros + prev_step * dur);
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
        let mut due: Vec<(usize, Pattern)> = Vec::new();
        for (lane, slot) in self.queued.iter_mut().enumerate() {
            let fire = matches!(slot, Some((_, q)) if is_boundary(step, *q));
            if fire {
                // take() leaves None → exactly-once (the slot is cleared on apply).
                if let Some((pattern, _)) = slot.take() {
                    due.push((lane, pattern));
                }
            }
        }
        for (lane, pattern) in due {
            // Release this lane's sounding + held notes so a swap can't hang a note.
            self.release_lanes(&[lane], at_micros, sink);
            if let Some(l) = self.set.lanes.get_mut(lane) {
                l.pattern = pattern;
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
        let swung =
            (step_start as i64 + swing_offset_micros(step, self.set.swing, dur)).max(0) as u64;

        let any_solo = self.set.lanes.iter().any(|l| l.solo);

        for lane_idx in 0..self.set.lanes.len() {
            if !self.lane_audible(lane_idx, any_solo) {
                continue;
            }
            let kind_is_drums =
                matches!(self.set.lanes[lane_idx].pattern.data, PatternData::Drums(_));
            if kind_is_drums {
                self.materialize_drum_step(lane_idx, step, swung, dur);
            } else {
                self.materialize_melodic_step(lane_idx, step, swung, dur);
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

    fn materialize_drum_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = self.local_step_for(lane_idx, step, count);
        // Emit on the route channel (route override else profile) so the channel the
        // scheduler emits on matches the channel the route plan keys on (no mis-route).
        let channel = lane.route_channel();
        let gate_fraction = lane.profile.drum_gate_fraction;
        let hits = match &lane.pattern.data {
            PatternData::Drums(steps) => steps.get(local).cloned().unwrap_or_default(),
            PatternData::Melodic(_) => Vec::new(),
        };
        for hit in hits {
            // Per-voice mute: skip this hit entirely when the voice is silenced.
            if self.set.lanes[lane_idx].is_voice_muted(hit.note) {
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
            for i in 0..r {
                let on_at = swung + i * sub;
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
    }

    fn materialize_melodic_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        use crate::devices::profiles::{melodic_velocity, resolve_melodic_pitch};

        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = self.local_step_for(lane_idx, step, count);
        // Emit on the route channel (route override else profile) so emission and the
        // route plan agree on the channel (no mis-route / dropped notes).
        let channel = lane.route_channel();
        let root = lane.profile.root_note;
        let transpose = lane.transpose;
        let octave = lane.octave;

        // Mono behavior (M5b Task 1): a step is now a Vec; play its FIRST note. Multi-note
        // (poly) playback is Task 3 — here we preserve today's single-note emission exactly.
        let note = match &lane.pattern.data {
            PatternData::Melodic(steps) => steps.get(local).and_then(|s| s.first()).cloned(),
            PatternData::Drums(_) => None,
        };
        let note = match note {
            Some(n) => n,
            None => return, // rest: nothing to emit, prior active note keeps its NoteOff.
        };

        // Probability is rolled once per note; a failed roll skips the entire step
        // (no NoteOn/NoteOff, and the prior active note keeps its scheduled release).
        if !self.rolls_fire(note.prob) {
            return;
        }

        let pitch = resolve_melodic_pitch(root, note.semi, transpose, octave);
        let vel = melodic_velocity(note.vel);
        let on_at = swung;

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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
        }
    }

    fn set_with(lanes: Vec<Lane>) -> Set {
        Set {
            name: "test".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
            id: crate::persist::Id::nil(),
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
            }),
            Some(MelodicNote {
                semi: 5,
                vel: 1.0,
                slide: true,
                len: 1.0,
                prob: 1.0,
                ratchet: 1,
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
        seq.sync_to_beat(2.5, 140.0); // step = floor(2.5*4) = 10
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
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: format!("len{length}"),
                desc: String::new(),
                length,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            s.sync_to_beat(5.0, 120.0); // abs step = step_from_beat(5.0) = 20
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
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "one".to_string(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            seq.sync_to_beat(0.0, 120.0);
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
        seq.sync_to_beat(1.0, 120.0); // step 4
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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
        seq.sync_to_beat(0.0, 120.0);
        seq.tick(0, &mut sink);

        // Link jumps forward to beat 1.0 → step 4 (skipping 1, 2, 3).
        // step 4 wraps to local step 0 in the 4-step pattern → note 36 again.
        seq.sync_to_beat(1.0, 120.0);
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
            seq.sync_to_beat(beat, 120.0);
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
        seq.sync_to_beat(0.5, 120.0); // step 2
        seq.tick(2 * dur, &mut sink);
        assert_eq!(seq.current_step(), 2);

        let before = sink.events.len();

        // Backward jump to step 0 (beat 0.0). next_step must NOT rewind.
        seq.sync_to_beat(0.0, 120.0);
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
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: format!("k{note}"),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
        seq.sync_to_beat(0.0, 120.0);
        seq.tick(0, &mut sink);

        // Queue at step 0..boundary.
        seq.queue_launch(0, drum_pattern_step0_note(50), Quant::NextBar);

        // Link advances to beat 4.0 → step 16 (the bar boundary). tick materializes it.
        seq.sync_to_beat(4.0, 120.0); // step_from_beat(4.0) = 16
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

    // ── Per-drum-voice mute tests (§2.6) ────────────────────────────────────

    /// Build a drum lane with two voices: kick (36) on step 0, hat (42) on step 0.
    fn drum_lane_kick_and_hat() -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 4];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        });
        steps[0].push(DrumHit {
            note: 42,
            vel: 80,
            prob: 1.0,
            ratchet: 1,
        });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "kh".to_string(),
                desc: String::new(),
                length: 4,
                data: PatternData::Drums(steps),
                id: crate::persist::Id::nil(),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: crate::music::scale::Scale::Chromatic,
            root: None,
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
}
