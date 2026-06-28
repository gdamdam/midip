//! Step sequencer: pure timing math (this task) plus the stateful `Sequencer`
//! (Task 9). All time is `u64` microseconds on a monotonic timeline.

use crate::midi::ports::MidiSink;
use crate::midi::MidiMessage;
use crate::pattern::model::{Lane, PatternData, Set};

/// Pulses Per Quarter Note for the MIDI clock.
pub const PPQN: u64 = 24;

/// Duration of one 16th-note step in microseconds at `bpm`.
/// A quarter note is `60_000_000 / bpm` µs; a 16th is a quarter of that.
pub fn step_dur_micros(bpm: f64) -> u64 {
    (60_000_000.0 / (bpm * 4.0)).round() as u64
}

/// Swing offset for `step_index` given a `swing` ratio (0.5 = straight) and the
/// straight step duration. Even (down-beat) steps are unshifted; odd (off-beat)
/// steps are delayed by `(swing - 0.5) * step_dur * 2`. Signed so off-steps can be
/// pulled earlier if `swing < 0.5`.
pub fn swing_offset_micros(step_index: usize, swing: f32, step_dur: u64) -> i64 {
    if step_index % 2 == 0 {
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

/// Convert a musical beat position to an absolute 16th-step index.
/// 1 beat = 4 16th steps, so `step = floor(beat * 4)`.
/// (Task 11 will expose this from `crate::link`; defined here for Task 9.)
fn step_from_beat(beat: f64) -> usize {
    (beat * 4.0).floor() as usize
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
    /// Time-ordered (ascending `at_micros`) pending events.
    queue: Vec<ScheduledEvent>,
    /// One active melodic note per lane (index parallels `set.lanes`).
    active: Vec<Option<ActiveNote>>,
    /// Externally-set current step (used by `sync_to_beat` / `current_step`).
    current: usize,
    /// Deterministic xorshift64 PRNG state for per-step probability rolls.
    rng: u64,
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
            queue: Vec::new(),
            active: vec![None; n],
            current: 0,
            rng: DEFAULT_SEED,
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

    pub fn play(&mut self, at_micros: u64) {
        self.playing = true;
        self.origin_micros = at_micros;
        self.current = 0;
        self.queue.clear();
        for a in self.active.iter_mut() {
            *a = None;
        }
        // Eagerly materialize step 0 so the caller's first tick (even at t=0) sees
        // its events ready to flush. next_step advances to 1 after materialization.
        let dur = step_dur_micros(self.set.bpm);
        self.next_step = 0;
        self.materialize_step(0, dur);
        self.next_step = 1;
    }

    /// Halt the sequencer, immediately sending NoteOff for every active melodic note.
    /// Emits directly to `sink` at `at_micros` so callers don't need a follow-up tick.
    pub fn stop(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // Release every still-active melodic note directly to sink.
        for slot in self.active.iter_mut() {
            if let Some(active) = slot.take() {
                sink.send(
                    MidiMessage::NoteOff { channel: active.channel, note: active.note },
                    at_micros,
                );
            }
        }
        self.playing = false;
        self.queue.clear();
    }

    /// All-notes-off / all-sound-off live recovery. Sends CC 123 (All Notes Off) and
    /// CC 120 (All Sound Off) on every lane's channel, plus a NoteOff for each tracked
    /// active melodic note, then clears note tracking. Emits directly to `sink` at
    /// `at_micros` (does not enqueue). Does NOT change `playing` — the performer keeps
    /// the transport running while clearing stuck notes mid-pattern.
    pub fn panic(&mut self, at_micros: u64, sink: &mut dyn MidiSink) {
        // CC 123 + CC 120 on each distinct lane channel.
        let mut sent: Vec<u8> = Vec::new();
        for lane in &self.set.lanes {
            let ch = lane.profile.channel;
            if sent.contains(&ch) {
                continue;
            }
            sent.push(ch);
            sink.send(
                MidiMessage::ControlChange { channel: ch, controller: 123, value: 0 },
                at_micros,
            );
            sink.send(
                MidiMessage::ControlChange { channel: ch, controller: 120, value: 0 },
                at_micros,
            );
        }
        // Explicit NoteOff for every tracked active note, then clear tracking.
        for slot in self.active.iter_mut() {
            if let Some(active) = slot.take() {
                sink.send(
                    MidiMessage::NoteOff { channel: active.channel, note: active.note },
                    at_micros,
                );
            }
        }
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
    pub fn lane_step(&self, idx: usize) -> usize {
        let len = self
            .set
            .lanes
            .get(idx)
            .map(|l| l.pattern.length.max(1))
            .unwrap_or(1);
        self.current % len
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

    /// Read accessor for a lane by index (the Sequencer owns `set: Set`).
    pub fn lane(&self, idx: usize) -> Option<&Lane> {
        self.set.lanes.get(idx)
    }

    /// Drive the internal clock to `now_micros`, emitting due events to `sink`.
    /// Returns the step index if a new step was materialized this call.
    pub fn tick(&mut self, now_micros: u64, sink: &mut dyn MidiSink) -> Option<usize> {
        if !self.playing {
            return None;
        }
        let dur = step_dur_micros(self.set.bpm);
        let mut advanced: Option<usize> = None;

        // Materialize every step whose start time has been reached.
        loop {
            let step_start = self.origin_micros + self.next_step as u64 * dur;
            if step_start >= now_micros {
                break;
            }
            let step = self.next_step;
            self.materialize_step(step, dur);
            self.current = step;
            advanced = Some(step);
            self.next_step += 1;
        }

        // Flush all queued events with at_micros <= now.
        self.flush_due(now_micros, sink);
        advanced
    }

    /// Link mode: place the sequencer at musical `beat` (16th = beat*4) at `bpm`.
    pub fn sync_to_beat(&mut self, beat: f64, bpm: f64) {
        self.set.bpm = bpm;
        self.current = step_from_beat(beat);
        self.next_step = self.current;
    }

    // --- internals -------------------------------------------------------

    fn materialize_step(&mut self, step: usize, dur: u64) {
        let step_start = self.origin_micros + step as u64 * dur;
        let swung = (step_start as i64
            + swing_offset_micros(step, self.set.swing, dur))
        .max(0) as u64;

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

    fn materialize_drum_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = step % count;
        let channel = lane.profile.channel;
        let gate_fraction = lane.profile.drum_gate_fraction;
        let hits = match &lane.pattern.data {
            PatternData::Drums(steps) => steps.get(local).cloned().unwrap_or_default(),
            PatternData::Melodic(_) => Vec::new(),
        };
        for hit in hits {
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
                        msg: MidiMessage::NoteOn { channel, note: hit.note, vel: hit.vel },
                    },
                );
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: on_at + gate,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff { channel, note: hit.note },
                    },
                );
            }
        }
    }

    fn materialize_melodic_step(&mut self, lane_idx: usize, step: usize, swung: u64, dur: u64) {
        use crate::devices::profiles::{melodic_velocity, resolve_melodic_pitch};

        let lane = &self.set.lanes[lane_idx];
        let count = lane.pattern.step_count().max(1);
        let local = step % count;
        let channel = lane.profile.channel;
        let root = lane.profile.root_note;
        let transpose = lane.transpose;
        let octave = lane.octave;

        let note = match &lane.pattern.data {
            PatternData::Melodic(steps) => steps.get(local).cloned().flatten(),
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
                msg: MidiMessage::NoteOn { channel, note: pitch, vel },
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
                    msg: MidiMessage::NoteOff { channel, note: pitch },
                },
            );
            for i in 1..r {
                let at = on_at + i * sub;
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: at,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOn { channel, note: pitch, vel },
                    },
                );
                Self::enqueue(
                    &mut self.queue,
                    ScheduledEvent {
                        at_micros: at + ratchet_gate,
                        lane: lane_idx,
                        msg: MidiMessage::NoteOff { channel, note: pitch },
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
                    msg: MidiMessage::NoteOff { channel, note: pitch },
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
        if let PatternData::Melodic(steps) = &self.set.lanes[lane_idx].pattern.data {
            for offset in 1..=count {
                let local = (step + offset) % count;
                if let Some(Some(n)) = steps.get(local) {
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
    fn flush_due(&mut self, now_micros: u64, sink: &mut dyn MidiSink) {
        let i = 0;
        while i < self.queue.len() {
            if self.queue[i].at_micros <= now_micros {
                let ev = self.queue.remove(i);
                sink.send(ev.msg, ev.at_micros);
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
        DrumHit, Lane, MelodicNote, Pattern, PatternData, Set,
    };

    // --- helpers ---------------------------------------------------------

    fn drum_lane_four_on_floor() -> Lane {
        // kick (note 36) on steps 0,4,8,12; 16 steps.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        for &s in &[0usize, 4, 8, 12] {
            steps[s].push(DrumHit { note: 36, vel: 100, prob: 1.0, ratchet: 1 });
        }
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "four".to_string(),
                length: 16,
                data: PatternData::Drums(steps),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
        }
    }

    fn melodic_lane(notes: Vec<Option<MelodicNote>>, profile_bass: bool) -> Lane {
        let len = notes.len();
        Lane {
            profile: if profile_bass { T8_BASS } else { S1 },
            pattern: Pattern {
                name: "mel".to_string(),
                length: len,
                data: PatternData::Melodic(notes),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
        }
    }

    fn set_with(lanes: Vec<Lane>) -> Set {
        Set {
            name: "test".to_string(),
            bpm: 120.0,
            swing: 0.5,
            lanes,
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
        // one bar = 16 steps * 125_000 = 2_000_000 µs; run a hair past.
        run(&mut seq, &mut sink, 16 * dur, 1_000);

        // collect kick NoteOn times.
        let ons: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0, 4 * dur, 8 * dur, 12 * dur]);

        // each NoteOn has a matching NoteOff drum_gate_fraction*step_dur later.
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::NoteOff { channel: 9, note: 36 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(offs, vec![gate, 4 * dur + gate, 8 * dur + gate, 12 * dur + gate]);
    }

    // --- (b) melodic pitch + velocity -----------------------------------

    #[test]
    fn melodic_note_emits_resolved_pitch_and_velocity() {
        let dur = step_dur_micros(120.0);
        // single note at step 0: semi +7, vel mult 1.0, no slide, len 1.0.
        let notes = vec![
            Some(MelodicNote { semi: 7, vel: 1.0, slide: false, len: 1.0, prob: 1.0, ratchet: 1 }),
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
            && *m == MidiMessage::NoteOn { channel: 1, note: 52, vel: 100 }));
        // NoteOff at len 1.0 * step_dur.
        let off_at = note_len_micros(1.0, dur);
        assert!(sink.events.iter().any(|(t, m)| *t == off_at
            && *m == MidiMessage::NoteOff { channel: 1, note: 52 }));
    }

    // --- (c) slide overlap ----------------------------------------------

    #[test]
    fn slide_note_on_precedes_prior_note_off() {
        let dur = step_dur_micros(120.0);
        // step 0: note A (len 1.0, no slide). step 1: note B with slide=true.
        // The slide on B must hold A until *after* B's NoteOn (legato overlap),
        // so A's NoteOff time > B's NoteOn time.
        let notes = vec![
            Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 1.0, prob: 1.0, ratchet: 1 }),
            Some(MelodicNote { semi: 5, vel: 1.0, slide: true, len: 1.0, prob: 1.0, ratchet: 1 }),
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
            .find(|(_, m)| *m == MidiMessage::NoteOff { channel: 1, note: note_a })
            .map(|(t, _)| *t)
            .expect("A must have a NoteOff");
        let b_on = sink
            .events
            .iter()
            .find(|(_, m)| *m == MidiMessage::NoteOn { channel: 1, note: note_b, vel: 100 })
            .map(|(t, _)| *t)
            .expect("B must have a NoteOn");
        // legato: A is released only after B sounds.
        assert!(a_off >= b_on, "expected A off ({}) >= B on ({})", a_off, b_on);
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
            .filter(|(_, m)| {
                matches!(m, MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. })
            })
            .count();
        assert_eq!(note_events, 0);
    }

    // --- (e) solo silences others ---------------------------------------

    #[test]
    fn solo_lane_silences_non_soloed() {
        let dur = step_dur_micros(120.0);
        let drums = drum_lane_four_on_floor(); // not soloed
        let mut bass = melodic_lane(
            vec![Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 1.0, prob: 1.0, ratchet: 1 }), None, None, None],
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
            .filter(|(_, m)| matches!(m,
                MidiMessage::NoteOn { channel: 9, .. } | MidiMessage::NoteOff { channel: 9, .. }))
            .count();
        assert_eq!(drum_events, 0);
        // bass (channel 1) must still sound.
        let bass_on = sink.events.iter().any(|(_, m)| {
            matches!(m, MidiMessage::NoteOn { channel: 1, .. })
        });
        assert!(bass_on);
    }

    #[test]
    fn stop_releases_active_notes_and_halts() {
        let dur = step_dur_micros(120.0);
        // a long note still sounding when we stop.
        let notes = vec![Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 4.0, prob: 1.0, ratchet: 1 })];
        let mut seq = Sequencer::new(set_with(vec![melodic_lane(notes, true)]));
        let mut sink = RecordingSink::new();
        seq.play(0);
        seq.tick(0, &mut sink); // emit the NoteOn
        assert!(seq.is_playing());
        seq.stop(1_000, &mut sink);
        assert!(!seq.is_playing());
        // there is a NoteOff for the still-active note at/after stop time.
        assert!(sink.events.iter().any(|(t, m)| *t >= 1_000
            && *m == MidiMessage::NoteOff { channel: 1, note: 45 }));
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
        steps[0].push(DrumHit { note: 36, vel: 100, prob: 1.0, ratchet: 1 });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: format!("len{length}"),
                length,
                data: PatternData::Drums(steps),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
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
            .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
            .map(|(t, _)| *t / dur)
            .collect();
        // lane 0 onsets present.
        for s in [0u64, 12, 24, 36] {
            assert!(fired_abs.contains(&s), "lane0 onset {s} missing in {fired_abs:?}");
        }
        // lane 2 onsets present (independent 7-step wrap).
        for s in [0u64, 7, 14, 21, 28, 35, 42] {
            assert!(fired_abs.contains(&s), "lane2 onset {s} missing in {fired_abs:?}");
        }
        // lane 1 (16) onsets at 0,16,32 also present.
        for s in [0u64, 16, 32] {
            assert!(fired_abs.contains(&s), "lane1 onset {s} missing in {fired_abs:?}");
        }
    }

    // --- (i) panic: all-notes-off without stopping transport -------------

    #[test]
    fn panic_emits_cc123_cc120_per_channel_and_keeps_playing() {
        // Two lanes on distinct channels (drums ch9, bass ch1).
        let drums = drum_lane_four_on_floor();
        let bass = melodic_lane(
            vec![Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 4.0, prob: 1.0, ratchet: 1 }), None, None, None],
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
                sink.events.iter().any(|(_, m)|
                    *m == MidiMessage::ControlChange { channel: ch, controller: 123, value: 0 }),
                "expected CC123 on channel {ch}"
            );
            // CC 120 (All Sound Off) on each distinct channel.
            assert!(
                sink.events.iter().any(|(_, m)|
                    *m == MidiMessage::ControlChange { channel: ch, controller: 120, value: 0 }),
                "expected CC120 on channel {ch}"
            );
        }
        // Panic must NOT stop transport.
        assert!(seq.is_playing(), "panic must leave transport playing");
    }

    // --- (f) probability -------------------------------------------------

    /// A single-step drum lane with the kick hit on step 0 only, prob `p`, ratchet `r`.
    fn drum_lane_one_hit(prob: f32, ratchet: u8) -> Lane {
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0].push(DrumHit { note: 36, vel: 100, prob, ratchet });
        Lane {
            profile: T8_DRUMS,
            pattern: Pattern {
                name: "one".to_string(),
                length: 16,
                data: PatternData::Drums(steps),
            },
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
        }
    }

    fn kick_on_count(sink: &RecordingSink) -> usize {
        sink.events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
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
        for s in 0..16 {
            steps[s].push(DrumHit { note: 36, vel: 100, prob: 0.5, ratchet: 1 });
        }
        let lane = Lane {
            profile: T8_DRUMS,
            pattern: Pattern { name: "row".to_string(), length: 16, data: PatternData::Drums(steps) },
            mute: false, solo: false, transpose: 0, octave: 0,
        };
        let run_once = |seed: u64| -> Vec<u64> {
            let mut seq = Sequencer::new(set_with(vec![lane.clone()]));
            seq.set_seed(seed);
            let mut sink = RecordingSink::new();
            seq.play(0);
            run(&mut seq, &mut sink, 16 * dur, 1_000);
            sink.events
                .iter()
                .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
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
            .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0, sub, 2 * sub]);

        // Each NoteOn has a NoteOff a (sub * drum_gate_fraction) gate later.
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, sub);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::NoteOff { channel: 9, note: 36 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(offs, vec![gate, sub + gate, 2 * sub + gate]);
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
            .filter(|(_, m)| *m == MidiMessage::NoteOn { channel: 9, note: 36, vel: 100 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(ons, vec![0]);
        let gate = note_len_micros(T8_DRUMS.drum_gate_fraction, dur);
        let offs: Vec<u64> = sink
            .events
            .iter()
            .filter(|(_, m)| *m == MidiMessage::NoteOff { channel: 9, note: 36 })
            .map(|(t, _)| *t)
            .collect();
        assert_eq!(offs, vec![gate]);
    }
}
