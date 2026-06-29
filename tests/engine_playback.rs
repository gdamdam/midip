use midip::devices::profiles;
use midip::devices::profiles::resolve_melodic_pitch;
use midip::engine::scheduler::step_dur_micros;
use midip::engine::{run_engine_headless, EngineEvent, UiCommand};
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
        drum_steps[s] = vec![DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        }];
    }
    let drums = Pattern {
        name: "kick".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Drums(drum_steps),
        id: midip::persist::Id::nil(),
    };

    let mut bass_steps: Vec<MelodicStep> = vec![None; 16];
    bass_steps[0] = Some(MelodicNote {
        semi: 0,
        vel: 1.0,
        slide: false,
        len: 0.5,
        prob: 1.0,
        ratchet: 1,
    });
    bass_steps[8] = Some(MelodicNote {
        semi: 0,
        vel: 1.0,
        slide: false,
        len: 0.5,
        prob: 1.0,
        ratchet: 1,
    });
    let bass = Pattern {
        name: "bass".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(bass_steps),
        id: midip::persist::Id::nil(),
    };

    let mut synth_steps: Vec<MelodicStep> = vec![None; 16];
    synth_steps[4] = Some(MelodicNote {
        semi: 12,
        vel: 1.0,
        slide: false,
        len: 0.9,
        prob: 1.0,
        ratchet: 1,
    });
    let synth = Pattern {
        name: "synth".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(synth_steps),
        id: midip::persist::Id::nil(),
    };

    let lanes = vec![
        Lane {
            profile: profs[0],
            pattern: drums,
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
        },
        Lane {
            profile: profs[1],
            pattern: bass,
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
        },
        Lane {
            profile: profs[2],
            pattern: synth,
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
        },
    ];
    Set {
        name: "test".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes,
        id: midip::persist::Id::nil(),
    }
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
    let bd: Vec<_> = ons
        .iter()
        .filter(|(_, ch, n, _)| *ch == 9 && *n == 36)
        .collect();
    assert_eq!(bd.len(), 4, "expected 4 BD hits, got {:?}", bd);
    // Bass on ch 1, root 45 + semi 0 = 45, at steps 0 and 8 -> 2 hits.
    let bass: Vec<_> = ons
        .iter()
        .filter(|(_, ch, n, _)| *ch == 1 && *n == 45)
        .collect();
    assert_eq!(bass.len(), 2, "expected 2 bass hits, got {:?}", bass);
    // Synth on ch 0, root 45 + semi 12 = 57, at step 4 -> 1 hit.
    let synth: Vec<_> = ons
        .iter()
        .filter(|(_, ch, n, _)| *ch == 0 && *n == 57)
        .collect();
    assert_eq!(synth.len(), 1, "expected 1 synth hit, got {:?}", synth);

    // Playhead events should cover steps 0..=15 (each step visited at least once).
    let mut seen = [false; 16];
    for ev in &events {
        if let EngineEvent::Playhead { step, .. } = ev {
            assert!(*step < 16);
            seen[*step] = true;
        }
    }
    assert!(
        seen.iter().all(|s| *s),
        "not all steps were reported: {:?}",
        seen
    );
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

    // stop() calls release_all() which always emits CC123 + CC120 per channel and
    // NoteOffs for any notes still sounding at stop time.  In this fixture all
    // NoteOffs were already flushed before step 6, so no extra NoteOff is produced —
    // that is correct behaviour.  What we assert instead is that CC123 (All Notes Off)
    // fired on the drum channel, proving release_all ran.
    // (Premise changed from Task 2: old stop sent a redundant NoteOff for the melodic
    // active tracker even when the NoteOff was already flushed; new stop only releases
    // notes that are genuinely still sounding per the authoritative registry.)
    let cc123_after_stop = sink.events.iter().any(|(at, m)| {
        *at >= stop_at
            && matches!(
                m,
                MidiMessage::ControlChange {
                    controller: 123,
                    ..
                }
            )
    });
    assert!(
        cc123_after_stop,
        "expected CC123 (all-notes-off) at/after stop"
    );

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
    assert_eq!(
        first_step,
        Some(expected),
        "Link sync should place playhead at step {}",
        expected
    );
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
    assert_eq!(
        link.started_at,
        Some(0),
        "Play under Link should call request_start"
    );
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
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: ch,
                    controller: 123,
                    value: 0
                }),
            "expected CC123 on channel {ch} after panic"
        );
        assert!(
            sink.events.iter().any(|(_, m)| *m
                == MidiMessage::ControlChange {
                    channel: ch,
                    controller: 120,
                    value: 0
                }),
            "expected CC120 on channel {ch} after panic"
        );
    }
    // Playback continues: the BD lane (ch 9, note 36) still fires after the panic point.
    let bd_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > panic_at && *ch == 9 && *n == 36)
        .count();
    assert!(
        bd_after > 0,
        "panic must not stop playback (expected BD hits after panic)"
    );
}

// --- Fix #3: undo/redo must reach playback ------------------------------------

#[test]
fn undo_synclanes_updates_engine_pattern_without_resetting_playhead() {
    // Set up: lane 1 (bass) has a note at step 0. We'll add a note at step 4,
    // then send SyncLanes with the original (undone) lanes and verify the engine
    // plays the original pattern (note at 0 only), not the edited one (notes at 0 and 4).
    let mut set = three_lane_set();
    // Modify lane 1 in the "edited" state: add an extra note at step 4.
    if let PatternData::Melodic(ref mut steps) = set.lanes[1].pattern.data {
        steps[4] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
        });
    }

    // The "undone" lanes have note only at steps 0 and 8 (original three_lane_set).
    let original_set = three_lane_set();
    let original_lanes = original_set.lanes.clone();

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let total = step * 16;

    // Play with the edited set, then send SyncLanes (simulating Undo) at step 2.
    let sync_at = step * 2;
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::Play),
            (sync_at, UiCommand::SyncLanes(original_lanes)),
        ],
        total,
        1_000,
    );

    // After SyncLanes the engine should play the restored pattern.
    // Lane 1 (bass ch 1) had note at step 4 in the edited set; after undo it should NOT fire
    // at step 4 (root 45 + semi 0 = 45, but the note at step 4 was edited in).
    // The bass channel 1 should have exactly the notes from the original pattern.
    let bass_ons: Vec<_> = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, _, _)| *ch == 1 && *at >= sync_at)
        .collect();
    // After sync, at most the notes from steps 8 (one step) should fire — NOT step 4.
    // Step 4 note was only in the edited pattern; SyncLanes should have removed it.
    let step4_time = step * 4;
    let step4_margin = step / 2;
    let step4_fires_after_sync = bass_ons.iter().any(|(at, _, _, _)| {
        *at >= step4_time.saturating_sub(step4_margin) && *at <= step4_time + step4_margin
    });
    assert!(
        !step4_fires_after_sync,
        "after SyncLanes (undo), edited step-4 note must not fire; got {:?}",
        bass_ons
    );
}

// --- Fix #4: octave must reach playback --------------------------------------

#[test]
fn set_octave_shifts_emitted_note_pitch() {
    // Lane 2 (S-1, ch 0): semi 12, root 45, octave 0 -> pitch = resolve(45, 12, 0, 0) = 57.
    // After SetOctave { lane: 2, octave: 1 } -> resolve(45, 12, 0, 1) should differ.
    let set = three_lane_set();
    let profs = profiles::default_profiles();
    let root = profs[2].root_note; // S-1 root

    let octave_before: i8 = 0;
    let octave_after: i8 = 1;
    let pitch_before = resolve_melodic_pitch(root, 12, 0, octave_before);
    let pitch_after = resolve_melodic_pitch(root, 12, 0, octave_after);
    assert_ne!(
        pitch_before, pitch_after,
        "test precondition: octave shift must change pitch"
    );

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let total = step * 16;

    // Send SetOctave before play so it takes effect for the whole bar.
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (
                0,
                UiCommand::SetOctave {
                    lane: 2,
                    octave: octave_after,
                },
            ),
            (0, UiCommand::Play),
        ],
        total,
        1_000,
    );

    // Synth lane 2 (ch 0) should emit NoteOn with pitch_after, not pitch_before.
    let synth_ons: Vec<_> = note_ons(&sink)
        .into_iter()
        .filter(|(_, ch, _, _)| *ch == 0)
        .collect();
    assert!(
        !synth_ons.is_empty(),
        "expected synth NoteOns after SetOctave"
    );
    assert!(
        synth_ons.iter().all(|(_, _, note, _)| *note == pitch_after),
        "all synth NoteOns must use shifted pitch {pitch_after}, got {:?}",
        synth_ons
    );
    assert!(
        synth_ons
            .iter()
            .all(|(_, _, note, _)| *note != pitch_before),
        "no synth NoteOn should use unshifted pitch {pitch_before}, got {:?}",
        synth_ons
    );
}

// --- Fix #6: Quit must release notes -----------------------------------------

#[test]
fn quit_emits_all_notes_off_before_stopping() {
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    // Play a few steps to get notes sounding, then Quit.
    let quit_at = step * 4;
    let total = step * 8;

    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (quit_at, UiCommand::Quit)],
        total,
        1_000,
    );

    // CC 123 (All Notes Off) must appear on each lane channel at/after quit_at.
    for ch in [9u8, 1u8, 0u8] {
        assert!(
            sink.events.iter().any(|(at, m)| *at >= quit_at
                && *m
                    == MidiMessage::ControlChange {
                        channel: ch,
                        controller: 123,
                        value: 0
                    }),
            "expected CC123 on channel {ch} at/after Quit"
        );
        assert!(
            sink.events.iter().any(|(at, m)| *at >= quit_at
                && *m
                    == MidiMessage::ControlChange {
                        channel: ch,
                        controller: 120,
                        value: 0
                    }),
            "expected CC120 on channel {ch} at/after Quit"
        );
    }
}

// --- Fix #8: render App::status in transport ---------------------------------

#[test]
fn status_string_appears_in_transport() {
    use midip::app::App;
    use midip::devices::profiles::default_profiles;
    use midip::pattern::library::{GenreMap, Library};
    use midip::pattern::model::Set;
    use midip::ui::transport::render_transport;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    let set = Set::default_set(default_profiles());
    let mut app = App::new(set, empty_library());
    app.status = "library loaded".to_string();

    let backend = TestBackend::new(120, 4);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| render_transport(f, f.area(), &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("library loaded"),
        "status must appear in transport, got: {text:?}"
    );
}
