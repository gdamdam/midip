use midip::devices::profiles;
use midip::engine::{run_engine_headless, EngineEvent, UiCommand};
use midip::engine::scheduler::step_dur_micros;
use midip::link::{step_from_beat, FakeLink};
use midip::midi::message::MidiMessage;
use midip::midi::ports::RecordingSink;
use midip::pattern::model::{
    DrumHit, DrumStep, Lane, MelodicNote, MelodicStep, Pattern, PatternData, Set,
};

/// Build a deterministic 3-lane set:
///   lane 0 (T-8 DRUM): BD (note 36) on steps 0,4,8,12
///   lane 1 (T-8 BASS): semi 0 note on steps 0 and 8
///   lane 2 (S-1):      semi 12 note on step 4
fn three_lane_set() -> Set {
    let profs = profiles::default_profiles();

    let mut drum_steps: Vec<DrumStep> = vec![Vec::new(); 16];
    for &s in &[0usize, 4, 8, 12] {
        drum_steps[s] = vec![DrumHit { note: 36, vel: 100, prob: 1.0, ratchet: 1 }];
    }
    let drums = Pattern {
        name: "kick".into(),
        length: 16,
        data: PatternData::Drums(drum_steps),
    };

    let mut bass_steps: Vec<MelodicStep> = vec![None; 16];
    bass_steps[0] = Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 0.5, prob: 1.0, ratchet: 1 });
    bass_steps[8] = Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 0.5, prob: 1.0, ratchet: 1 });
    let bass = Pattern {
        name: "bass".into(),
        length: 16,
        data: PatternData::Melodic(bass_steps),
    };

    let mut synth_steps: Vec<MelodicStep> = vec![None; 16];
    synth_steps[4] = Some(MelodicNote { semi: 12, vel: 1.0, slide: false, len: 0.9, prob: 1.0, ratchet: 1 });
    let synth = Pattern {
        name: "synth".into(),
        length: 16,
        data: PatternData::Melodic(synth_steps),
    };

    let lanes = vec![
        Lane { profile: profs[0], pattern: drums, mute: false, solo: false, transpose: 0, octave: 0 },
        Lane { profile: profs[1], pattern: bass,  mute: false, solo: false, transpose: 0, octave: 0 },
        Lane { profile: profs[2], pattern: synth, mute: false, solo: false, transpose: 0, octave: 0 },
    ];
    Set { name: "test".into(), bpm: 120.0, swing: 0.5, lanes }
}

fn note_ons(sink: &RecordingSink) -> Vec<(u64, u8, u8, u8)> {
    sink.events
        .iter()
        .filter_map(|(at, m)| match m {
            MidiMessage::NoteOn { channel, note, vel } => Some((*at, *channel, *note, *vel)),
            _ => None,
        })
        .collect()
}

#[test]
fn play_one_bar_emits_expected_noteons_and_playhead_advances() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    // One bar at 120 bpm: 16 * step_dur. Add a tick margin so step 15 fires.
    let step = step_dur_micros(120.0);
    let total = step * 16;
    let events = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play)],
        total,
        1_000, // 1 ms virtual tick
    );

    let ons = note_ons(&sink);
    // Drum BD on ch 9 (note 36) at steps 0,4,8,12 -> 4 hits.
    let bd: Vec<_> = ons.iter().filter(|(_, ch, n, _)| *ch == 9 && *n == 36).collect();
    assert_eq!(bd.len(), 4, "expected 4 BD hits, got {:?}", bd);
    // Bass on ch 1, root 45 + semi 0 = 45, at steps 0 and 8 -> 2 hits.
    let bass: Vec<_> = ons.iter().filter(|(_, ch, n, _)| *ch == 1 && *n == 45).collect();
    assert_eq!(bass.len(), 2, "expected 2 bass hits, got {:?}", bass);
    // Synth on ch 0, root 45 + semi 12 = 57, at step 4 -> 1 hit.
    let synth: Vec<_> = ons.iter().filter(|(_, ch, n, _)| *ch == 0 && *n == 57).collect();
    assert_eq!(synth.len(), 1, "expected 1 synth hit, got {:?}", synth);

    // Playhead events should cover steps 0..=15 (each step visited at least once).
    let mut seen = [false; 16];
    for ev in &events {
        if let EngineEvent::Playhead { step, .. } = ev {
            assert!(*step < 16);
            seen[*step] = true;
        }
    }
    assert!(seen.iter().all(|s| *s), "not all steps were reported: {:?}", seen);
}

#[test]
fn stop_mid_run_emits_all_notes_off_and_halts_noteons() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    // Play at t=0, Stop after 6 steps; run a full bar of virtual time.
    let stop_at = step * 6;
    let total = step * 16;
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (stop_at, UiCommand::Stop)],
        total,
        1_000,
    );

    // There must be at least one NoteOff at/after stop_at (all-notes-off on stop).
    let off_after_stop = sink
        .events
        .iter()
        .any(|(at, m)| *at >= stop_at && matches!(m, MidiMessage::NoteOff { .. }));
    assert!(off_after_stop, "expected NoteOff after stop");

    // No NoteOn should be scheduled after stop (allow a one-step grace for the stop tick).
    let on_after_stop = note_ons(&sink)
        .into_iter()
        .filter(|(at, _, _, _)| *at >= stop_at + step)
        .count();
    assert_eq!(on_after_stop, 0, "no NoteOns should fire after Stop");
}

#[test]
fn mute_lane_silences_only_that_lane() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let total = step * 16;
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::Mute { lane: 0, on: true }),
            (0, UiCommand::Play),
        ],
        total,
        1_000,
    );

    let ons = note_ons(&sink);
    // Lane 0 (drums, ch 9) must be silent; bass + synth still sound.
    let drum_ons = ons.iter().filter(|(_, ch, _, _)| *ch == 9).count();
    assert_eq!(drum_ons, 0, "muted drum lane should emit no NoteOns");
    let bass_ons = ons.iter().filter(|(_, ch, _, _)| *ch == 1).count();
    assert_eq!(bass_ons, 2, "bass lane should still sound");
}

#[test]
fn link_enabled_sync_drives_step_from_beat() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    // Place the session at beat 2.0 -> step 8.
    link.set_beat(2.0);
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    // Run a short window so sync places the playhead before it advances far.
    let total = step; // one step of virtual time
    let events = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::ToggleLink(true)), (0, UiCommand::Play)],
        total,
        1_000,
    );

    let expected = step_from_beat(2.0) % 16; // 8
    let first_step = events.iter().find_map(|ev| match ev {
        EngineEvent::Playhead { step, .. } => Some(*step),
        _ => None,
    });
    assert_eq!(first_step, Some(expected), "Link sync should place playhead at step {}", expected);
}

#[test]
fn play_with_link_enabled_requests_quantized_start() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::ToggleLink(true)), (0, UiCommand::Play)],
        step,
        1_000,
    );
    // With Link enabled, Play must request a quantized (next-bar) start.
    assert_eq!(link.started_at, Some(0), "Play under Link should call request_start");
}

#[test]
fn panic_emits_all_notes_off_without_stopping_playback() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    // Play, then Panic a few steps in. Panic must NOT stop the sequencer, so NoteOns
    // continue to fire after the panic point.
    let panic_at = step * 3;
    let total = step * 16;
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (panic_at, UiCommand::Panic)],
        total,
        1_000,
    );

    // CC 123 (All Notes Off) appears on each distinct lane channel (9, 1, 0).
    for ch in [9u8, 1u8, 0u8] {
        assert!(
            sink.events.iter().any(|(_, m)|
                *m == MidiMessage::ControlChange { channel: ch, controller: 123, value: 0 }),
            "expected CC123 on channel {ch} after panic"
        );
        assert!(
            sink.events.iter().any(|(_, m)|
                *m == MidiMessage::ControlChange { channel: ch, controller: 120, value: 0 }),
            "expected CC120 on channel {ch} after panic"
        );
    }
    // Playback continues: the BD lane (ch 9, note 36) still fires after the panic point.
    let bd_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > panic_at && *ch == 9 && *n == 36)
        .count();
    assert!(bd_after > 0, "panic must not stop playback (expected BD hits after panic)");
}
