//! Cross-feature integration tests (project spec §"cross-feature suites").
//!
//! These exercise COMBINATIONS of shipped v0.7.0 features together through the
//! PUBLIC crate API — not single units (those live in the `src/` `#[cfg(test)]`
//! modules and in `tests/engine_playback.rs`). Each test names the features it
//! combines in a leading comment.
//!
//! Determinism / parallel-safety:
//!   * timing scenarios use `FakeLink` (returns a fixed beat regardless of micros)
//!     and the headless engine driver (virtual clock), so there is no wall-clock
//!     dependence;
//!   * filesystem scenarios use a per-test unique temp dir under
//!     `std::env::temp_dir()` so tests never share state across the parallel runner.
//!
//! Deliberately SKIPPED (per task brief): anything needing the real engine thread
//! or real CoreMIDI (hardware/acceptance-only), and scenes / chords /
//! MIDI-clock-input (NOT built in v0.7.0). The mirror dedup internals
//! (`route_targets_with_mirror`) are PRIVATE — only the routing surface that is
//! `pub` is exercised here; see `virtual_route_maps_to_virtual_port_and_channel`.

use midip::devices::profiles;
use midip::engine::scheduler::{step_dur_micros, Quant, Sequencer};
use midip::engine::{run_engine_headless, EngineEvent, UiCommand};
use midip::link::{step_from_beat, FakeLink};
use midip::midi::message::MidiMessage;
use midip::midi::ports::RecordingSink;
use midip::pattern::library::{GenreMap, Library};
use midip::pattern::model::{
    DrumHit, DrumStep, Lane, LaneRoute, MelodicNote, MelodicStep, Pattern, PatternData, PortRef,
    Set,
};
use midip::pattern::refs::PatternRef;
use midip::pattern::store;

// ─────────────────────────────────────────────────────────────────────────────
// Shared fixtures
// ─────────────────────────────────────────────────────────────────────────────

/// A unique temp dir per test for filesystem isolation under the parallel runner.
fn unique_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("midip-xfeat-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A drum lane (lane 0, ch 9) with BD (note 36) on steps 0,4,8,12 and a clap
/// (note 39) on steps 2,6,10,14 — two independent voices so per-voice mute can
/// be observed. Bass + synth lanes mirror `engine_playback.rs`'s fixture so the
/// channel/pitch expectations match the rest of the suite.
fn three_lane_set() -> Set {
    let profs = profiles::default_profiles();

    let mut drum_steps: Vec<DrumStep> = vec![Vec::new(); 16];
    for &s in &[0usize, 4, 8, 12] {
        drum_steps[s].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        });
    }
    for &s in &[2usize, 6, 10, 14] {
        drum_steps[s].push(DrumHit {
            note: 39,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        });
    }
    let drums = Pattern {
        name: "kick+clap".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Drums(drum_steps),
        id: midip::persist::Id::nil(),
    };

    let bass = melodic_pattern("bass", &[(0, 0, 0.5), (8, 0, 0.5)]);
    let synth = melodic_pattern("synth", &[(4, 12, 0.9)]);

    Set {
        name: "test".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![
            lane(profs[0], drums),
            lane(profs[1], bass),
            lane(profs[2], synth),
        ],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
    }
}

fn lane(profile: profiles::DeviceProfile, pattern: Pattern) -> Lane {
    Lane {
        profile,
        pattern,
        mute: false,
        solo: false,
        transpose: 0,
        octave: 0,
        route: None,
        muted_voices: Vec::new(),
        scale: midip::music::scale::Scale::Chromatic,
        root: None,
    }
}

/// Build a melodic pattern from `(step, semi, len)` tuples.
fn melodic_pattern(name: &str, notes: &[(usize, i8, f32)]) -> Pattern {
    let mut steps: Vec<MelodicStep> = vec![MelodicStep::default(); 16];
    for &(step, semi, len) in notes {
        steps[step] = MelodicStep::from(vec![MelodicNote {
            semi,
            vel: 1.0,
            slide: false,
            len,
            prob: 1.0,
            ratchet: 1,
        }]);
    }
    Pattern {
        name: name.into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(steps),
        id: midip::persist::Id::nil(),
    }
}

/// A melodic pattern with a note on EVERY step (semi 5) so a launch / playback is
/// immediately observable on the next step.
fn distinct_melodic(name: &str) -> Pattern {
    let mut steps: Vec<MelodicStep> = vec![MelodicStep::default(); 16];
    for s in steps.iter_mut() {
        *s = MelodicStep::from(vec![MelodicNote {
            semi: 5,
            vel: 1.0,
            slide: false,
            len: 0.5,
            prob: 1.0,
            ratchet: 1,
        }]);
    }
    Pattern {
        name: name.into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(steps),
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

/// Minimal in-memory library (one genre, one pattern per role) for App tests.
fn test_library() -> Library {
    let mut drums: GenreMap = GenreMap::new();
    drums.insert(
        "techno".into(),
        vec![{
            let mut p = Pattern::empty_drums(16);
            p.name = "lib-drum".into();
            if let PatternData::Drums(ref mut s) = p.data {
                s[0].push(DrumHit {
                    note: 36,
                    vel: 90,
                    prob: 1.0,
                    ratchet: 1,
                });
            }
            p
        }],
    );
    let mut bass: GenreMap = GenreMap::new();
    bass.insert(
        "acid".into(),
        vec![melodic_pattern("lib-bass", &[(0, 3, 0.5)])],
    );
    let mut synth: GenreMap = GenreMap::new();
    synth.insert(
        "dub".into(),
        vec![melodic_pattern("lib-synth", &[(0, 7, 0.5)])],
    );
    Library { drums, bass, synth }
}

// ═════════════════════════════════════════════════════════════════════════════
// 1. Quantized launch — under MANUAL ticking AND under a LINK-driven beat.
//    Features combined: clip-launch queue (QueuePattern/queue_launch) × the
//    quantize grid (Quant::NextBar) × manual transport tick × Link beat sync.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn quantized_launch_manual_fires_once_at_bar_boundary_not_before() {
    // Manual transport: queue a launch mid-bar; it must NOT fire on steps 5..15
    // and must fire EXACTLY once at the next bar boundary (absolute step 16).
    let mut seq = Sequencer::new(three_lane_set());
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);

    seq.play(0);
    // Advance to step 4 (mid-bar) with no launch queued yet.
    for s in 0..=4u64 {
        seq.tick(s * step, &mut sink);
        assert!(
            seq.take_launched().is_empty(),
            "no launch should fire before one is queued (step {s})"
        );
    }
    // Queue a NextBar launch on lane 1 (bass) after we are already mid-bar.
    seq.queue_launch(1, distinct_melodic("queued"), Quant::NextBar);

    // Steps 5..=15 must NOT fire the launch.
    let mut fired_before: Vec<u64> = Vec::new();
    for s in 5..=15u64 {
        seq.tick(s * step, &mut sink);
        if !seq.take_launched().is_empty() {
            fired_before.push(s);
        }
    }
    assert!(
        fired_before.is_empty(),
        "launch fired before the bar boundary at steps {fired_before:?}"
    );

    // Step 16 (next bar boundary) must fire the launch exactly once.
    seq.tick(16 * step, &mut sink);
    let launched_at_bar = seq.take_launched();
    assert_eq!(
        launched_at_bar,
        vec![1],
        "launch must fire exactly once at the bar boundary (step 16)"
    );

    // And not again on the following step.
    seq.tick(17 * step, &mut sink);
    assert!(
        seq.take_launched().is_empty(),
        "launch must fire exactly once, not repeat"
    );
}

#[test]
fn quantized_launch_under_link_driven_beat_fires_at_bar() {
    // Link path: the engine advances the playhead by calling `sync_to_beat(beat, bpm)`
    // per tick. `FakeLink` returns a CONSTANT beat (it is intentionally frozen), so a
    // live Link beat is modelled here by driving `Sequencer::sync_to_beat` directly with
    // a beat that walks forward across the bar boundary — exactly the internal Link path
    // (`step_engine` → `sync_to_beat`). This stays deterministic without a real session.
    // Features combined: Link beat sync × clip-launch queue × NextBar quantize.
    let mut seq = Sequencer::new(three_lane_set());
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let bpm = 120.0;

    seq.play(0);
    // Sync to beat 1.0 -> step 4 (mid-bar), then queue a NextBar launch.
    seq.sync_to_beat(1.0, bpm);
    seq.tick(4 * step, &mut sink);
    let _ = seq.take_launched();
    seq.queue_launch(1, distinct_melodic("queued"), Quant::NextBar);

    // Walk the Link beat forward through steps 5..15 (beats 1.25 .. 3.75): the launch
    // must NOT fire before the bar boundary.
    let mut fired_before = false;
    for s in 5..=15u64 {
        seq.sync_to_beat(s as f64 / 4.0, bpm);
        seq.tick(s * step, &mut sink);
        if !seq.take_launched().is_empty() {
            fired_before = true;
        }
    }
    assert!(
        !fired_before,
        "Link-driven launch fired before the bar boundary"
    );

    // Beat 4.0 -> step 16 (bar boundary): launch fires exactly once.
    seq.sync_to_beat(4.0, bpm);
    seq.tick(16 * step, &mut sink);
    assert_eq!(
        seq.take_launched(),
        vec![1],
        "Link-driven launch must fire exactly once at the bar boundary (step 16)"
    );
    assert_eq!(
        seq.current_step() % 16,
        0,
        "the launch must land on a bar boundary"
    );
}

#[test]
fn play_under_link_requests_quantized_start_with_queue_armed() {
    // Full engine + FakeLink: Play with Link enabled must request a quantized
    // (next-bar) start (`request_start` → link.started_at). This is the quantized-
    // launch arming half that FakeLink CAN verify deterministically; the actual
    // beat-boundary firing is covered by the sync_to_beat test above (FakeLink's
    // beat is frozen, so it cannot itself advance across a bar).
    // Features combined: Link enable × transport Play × quantized start request.
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::ToggleLink(true)),
            (0, UiCommand::Play),
            (
                0,
                UiCommand::QueuePattern {
                    lane: 1,
                    pattern: distinct_melodic("queued"),
                    quant: Quant::NextBar,
                },
            ),
        ],
        step,
        1_000,
    );
    assert_eq!(
        link.started_at,
        Some(0),
        "Play under Link should request_start (quantized launch arming)"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. Queue cancellation.
//    Features combined: clip-launch queue × cancellation (CancelQueue) × manual
//    transport — a cancelled launch must never fire.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn cancelled_queue_never_fires_via_sequencer() {
    let mut seq = Sequencer::new(three_lane_set());
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);

    seq.play(0);
    seq.tick(0, &mut sink);
    let _ = seq.take_launched();
    // Queue a NextBar launch on lane 1, then cancel it before the boundary.
    seq.queue_launch(1, distinct_melodic("cancelled"), Quant::NextBar);
    seq.cancel_launch(1);

    // Cross the bar boundary (step 16) and beyond: nothing should ever launch.
    let mut any_launch = false;
    for s in 1..=20u64 {
        seq.tick(s * step, &mut sink);
        if !seq.take_launched().is_empty() {
            any_launch = true;
        }
    }
    assert!(!any_launch, "a cancelled queued launch must never fire");
}

#[test]
fn cancelled_queue_never_fires_via_full_engine() {
    // Same scenario through the public UiCommand surface: QueuePattern then
    // CancelQueue must yield zero EngineEvent::Launched.
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let total = step * 33;

    let events = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::Play),
            (
                step * 2,
                UiCommand::QueuePattern {
                    lane: 1,
                    pattern: distinct_melodic("cancelled"),
                    quant: Quant::NextBar,
                },
            ),
            (step * 3, UiCommand::CancelQueue { lane: 1 }),
        ],
        total,
        1_000,
    );

    let launched = events
        .iter()
        .filter(|e| matches!(e, EngineEvent::Launched { .. }))
        .count();
    assert_eq!(
        launched, 0,
        "cancelled launch must produce no Launched event"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 3. Audition then route change (app-level).
//    Features combined: library audition preview (Action::Audition, isolated
//    overlay) × runtime route change (Action::RouteAdjustChannel →
//    UiCommand::SetRoute) × audition revert. Verifies no stuck state: the route
//    change applies to the committed Set, the audition overlay survives unchanged,
//    and closing the library cleanly reverts the audition (no panic).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn audition_then_route_change_stays_consistent() {
    use midip::app::{Action, App};

    let mut set = three_lane_set();
    // Gate: audition requires the lane be stopped OR muted. Mute lane 0 so we can
    // audition it while (conceptually) playing.
    set.lanes[0].mute = true;
    let mut app = App::new(set, test_library());
    app.engine_playing = true; // simulate live transport; muted lane allows audition

    app.apply(Action::FocusLane(0)); // drums lane -> lib_role Drums
    app.apply(Action::OpenLibrary);
    let audition_cmds = app.apply(Action::Audition);
    assert!(
        app.audition.is_some(),
        "audition overlay must be set after Action::Audition"
    );
    // Auditioning a muted/stopped lane should emit a LoadPattern preview.
    assert!(
        audition_cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
        "audition should emit a LoadPattern preview, got {audition_cmds:?}"
    );
    let auditioned_pattern = app.audition.as_ref().unwrap().pattern.clone();

    // Now change lane 0's route via the route editor. This must NOT disturb the
    // audition overlay and MUST emit a SetRoute.
    app.apply(Action::OpenRouteEditor);
    let route_cmds = app.apply(Action::RouteAdjustChannel(1));
    assert!(
        route_cmds.iter().any(|c| matches!(
            c,
            UiCommand::SetRoute {
                lane: 0,
                route: Some(_)
            }
        )),
        "route change must emit SetRoute for lane 0, got {route_cmds:?}"
    );
    // The audition overlay must be intact and unchanged (route editing does not
    // touch the isolated preview).
    assert!(
        app.audition.is_some(),
        "route change must not clear the audition overlay"
    );
    assert_eq!(
        app.audition.as_ref().unwrap().pattern,
        auditioned_pattern,
        "route change must not mutate the audition preview pattern"
    );
    // The committed lane now carries an explicit route.
    assert!(
        app.set.lanes[0].route.is_some(),
        "route change must commit an explicit LaneRoute on the lane"
    );

    // Finally, close the library: the audition must revert cleanly (overlay
    // cleared, a revert LoadPattern emitted) with no panic.
    app.mode = midip::app::Mode::Library; // re-enter library context to close it
    let close_cmds = app.apply(Action::CloseLibrary);
    assert!(
        app.audition.is_none(),
        "closing the library must clear the audition overlay"
    );
    assert!(
        close_cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
        "audition revert must restore the committed pattern via LoadPattern, got {close_cmds:?}"
    );
    // End state is consistent: route is committed, audition is gone.
    assert!(app.set.lanes[0].route.is_some());
}

// ═════════════════════════════════════════════════════════════════════════════
// 4. SetSet releases notes AND the loaded set plays at its OWN BPM.
//    Features combined: live set swap (UiCommand::SetSet) × note release on swap
//    (release_all → CC123) × per-set BPM driving step timing.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn setset_releases_held_notes_and_adopts_new_bpm() {
    // First set: a single melodic lane with a LONG held note at step 0 so it is
    // genuinely sounding when SetSet fires mid-bar.
    let profs = profiles::default_profiles();
    let held = melodic_pattern("held", &[(0, 0, 8.0)]); // 8-step gate: still sounding mid-bar
    let set_a = Set {
        name: "A".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![lane(profs[1], held)],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
    };

    // Second set: a different BPM and a note on every step so timing is observable.
    let dense = distinct_melodic("dense");
    let set_b = Set {
        name: "B".into(),
        bpm: 60.0, // half tempo -> step duration doubles
        swing: 0.5,
        lanes: vec![lane(profs[1], dense)],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
    };

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step_a = step_dur_micros(120.0);
    let step_b = step_dur_micros(60.0);

    // Play set A, then SetSet to B at step 2 (the held note is still sounding).
    let swap_at = step_a * 2;
    let total = swap_at + step_b * 5;
    let _ = run_engine_headless(
        set_a,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (swap_at, UiCommand::SetSet(set_b))],
        total,
        1_000,
    );

    // A release went out at/after the swap: SetSet's release_all emits CC123
    // (all-notes-off) on the lane channel (bass = ch 1).
    let release_after_swap = sink.events.iter().any(|(at, m)| {
        *at >= swap_at
            && matches!(
                m,
                MidiMessage::ControlChange {
                    channel: 1,
                    controller: 123,
                    ..
                }
            )
    });
    assert!(
        release_after_swap,
        "SetSet must release the held note (CC123 on ch 1) at the swap"
    );

    // The new set plays at its OWN (slower) BPM: consecutive NoteOns AFTER the
    // swap must be spaced by set B's step duration (60 bpm), not set A's.
    let ons_after: Vec<u64> = note_ons(&sink)
        .into_iter()
        .filter(|(at, _, _, _)| *at >= swap_at)
        .map(|(at, _, _, _)| at)
        .collect();
    assert!(
        ons_after.len() >= 2,
        "expected at least two NoteOns after the swap, got {ons_after:?}"
    );
    let gap = ons_after[1] - ons_after[0];
    // Allow a small tolerance; gap must match set B's step (slower), not set A's.
    let tol = step_a / 4;
    assert!(
        gap.abs_diff(step_b) <= tol,
        "post-swap step gap {gap} must match new-set BPM step {step_b} (not old {step_a})"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// 5. Per-drum-voice mute during playback releases that voice and keeps it silent.
//    Features combined: per-voice mute (set_voice_mute / MuteVoice) × live
//    playback × multi-voice drum lane (BD note 36 + clap note 39). Muting BD must
//    silence note 36 going forward while note 39 keeps sounding.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn voice_mute_during_playback_silences_only_that_voice() {
    let mut seq = Sequencer::new(three_lane_set());
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);

    seq.play(0);
    // Play through step 5 so BD (steps 0,4) and clap (step 2) have sounded.
    for s in 0..=5u64 {
        seq.tick(s * step, &mut sink);
    }
    // Mute the BD voice (note 36) on the drum lane (lane 0, ch 9) mid-playback.
    seq.set_voice_mute(0, 36, true, 5 * step, &mut sink);
    let bd_before = note_ons(&sink)
        .iter()
        .filter(|(_, ch, n, _)| *ch == 9 && *n == 36)
        .count();
    assert!(bd_before > 0, "BD should have sounded before the mute");

    // Continue through the rest of the bar (steps 6..=15).
    for s in 6..=15u64 {
        seq.tick(s * step, &mut sink);
    }
    let bd_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > 5 * step && *ch == 9 && *n == 36)
        .count();
    assert_eq!(bd_after, 0, "muted BD voice (note 36) must stay silent");

    // The clap voice (note 39) on the SAME lane must keep sounding (steps 6,10,14).
    let clap_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > 5 * step && *ch == 9 && *n == 39)
        .count();
    assert!(
        clap_after > 0,
        "the non-muted clap voice (note 39) must keep sounding"
    );
}

#[test]
fn voice_mute_via_full_engine_command_path() {
    // Same scenario through the public UiCommand::MuteVoice surface.
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let mute_at = step * 5;
    let total = step * 16;

    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::Play),
            (
                mute_at,
                UiCommand::MuteVoice {
                    lane: 0,
                    note: 36,
                    on: true,
                },
            ),
        ],
        total,
        1_000,
    );

    let bd_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > mute_at && *ch == 9 && *n == 36)
        .count();
    assert_eq!(bd_after, 0, "MuteVoice must silence note 36 going forward");
    let clap_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, n, _)| *at > mute_at && *ch == 9 && *n == 39)
        .count();
    assert!(clap_after > 0, "other drum voice must be unaffected");
}

// ═════════════════════════════════════════════════════════════════════════════
// 6. Favorites + crate persistence ACROSS a crate rename.
//    Features combined: user-pattern save × favorites (user + vendored refs) ×
//    crates (entries referencing both) × crate rename × full save/load round-trip
//    on disk. References must survive the rename and the round-trip.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn favorites_and_crates_survive_rename_and_roundtrip() {
    let dir = unique_dir("fav-crate");

    // Save a user pattern (mints a stable id we can reference).
    let mut user_pat = Pattern::empty_drums(16);
    user_pat.name = "my-beat".into();
    store::save_user_pattern(&dir, &mut user_pat).unwrap();
    let user_ref = PatternRef::User(user_pat.id.clone());
    let vendored_ref = PatternRef::Vendored {
        role: "drums".into(),
        genre: "techno".into(),
        name: "Four on Floor".into(),
    };

    // Favorite BOTH the user pattern and the vendored ref.
    let mut favs = store::Favorites::default();
    assert!(favs.toggle(user_ref.clone()));
    assert!(favs.toggle(vendored_ref.clone()));
    store::save_favorites(&dir, &favs).unwrap();

    // Build a crate containing both refs, then RENAME it.
    let mut crates = store::CrateIndex::default();
    let idx = crates.add_crate("Set A".into());
    crates.add_entry(
        idx,
        store::CrateEntry {
            pattern: user_ref.clone(),
            label: None,
        },
    );
    crates.add_entry(
        idx,
        store::CrateEntry {
            pattern: vendored_ref.clone(),
            label: Some("kick".into()),
        },
    );
    let crate_id_before = crates.crates[idx].id.clone();
    crates.rename_crate(idx, "Renamed Set".into());
    store::save_crates(&dir, &crates).unwrap();

    // Round-trip both stores from disk.
    let favs_back = store::load_favorites(&dir);
    let crates_back = store::load_crates(&dir);

    // Favorites references survive.
    assert!(
        favs_back.contains(&user_ref),
        "user favorite must survive round-trip"
    );
    assert!(
        favs_back.contains(&vendored_ref),
        "vendored favorite must survive round-trip"
    );

    // Crate survives the rename with a STABLE id and BOTH entry references intact.
    assert_eq!(crates_back.crates.len(), 1);
    let c = &crates_back.crates[0];
    assert_eq!(c.name, "Renamed Set", "rename must persist");
    assert_eq!(
        c.id, crate_id_before,
        "rename must keep the crate's stable id"
    );
    assert_eq!(c.entries.len(), 2, "both crate entries must survive");
    assert_eq!(c.entries[0].pattern, user_ref);
    assert_eq!(c.entries[1].pattern, vendored_ref);
    assert_eq!(c.entries[1].label.as_deref(), Some("kick"));

    let _ = std::fs::remove_dir_all(&dir);
}

// ═════════════════════════════════════════════════════════════════════════════
// 7. Recovery snapshot containing a CUSTOM route.
//    Features combined: crash-recovery autosave (save_recovery) × explicit
//    per-lane routing (LaneRoute, incl. the virtual "midip" port) × set
//    save/load DTO. The route must survive save_recovery → load round-trip.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn recovery_snapshot_preserves_custom_lane_route() {
    let dir = unique_dir("recovery-route");
    let mut set = three_lane_set();
    set.name = "with-route".into();

    // Lane 1 gets an explicit hardware route on a non-default channel.
    set.lanes[1].route = Some(LaneRoute {
        port: PortRef {
            stable_key: "USB-MIDI-1".into(),
            name: "USB MIDI Device".into(),
        },
        channel: 7,
        clock_out: false,
    });
    // Lane 2 gets routed to the engine-managed VIRTUAL "midip" port.
    set.lanes[2].route = Some(LaneRoute {
        port: PortRef::virtual_midip(),
        channel: 3,
        clock_out: true,
    });

    store::save_recovery(&dir, &set).unwrap();
    assert!(store::recovery_exists(&dir), "recovery file should exist");

    // The recovery file is a SetDto, so it round-trips through load_set.
    let loaded = store::load_set(&store::recovery_path(&dir)).unwrap();

    assert_eq!(
        loaded.lanes[1].route, set.lanes[1].route,
        "explicit hardware route must survive recovery round-trip"
    );
    let virt = loaded.lanes[2]
        .route
        .as_ref()
        .expect("virtual route must survive");
    assert!(
        virt.port.is_virtual(),
        "the virtual 'midip' route must round-trip as virtual"
    );
    assert_eq!(virt.channel, 3);
    // Lane 0 had no explicit route -> stays None (derives from profile).
    assert!(loaded.lanes[0].route.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

// ═════════════════════════════════════════════════════════════════════════════
// 8. Virtual-routable lane maps correctly (pure routing surface) + live re-route.
//    Features combined: per-lane routing override × the virtual "midip" port ×
//    the channel-resolution path the scheduler uses (route_channel /
//    effective_route).
//
//    NOTE on the "mirror does not double-send" half of this scenario: the mirror
//    fold-in / dedup logic (`route_targets_with_mirror`,
//    `build_route_plan_with_virtual`) is PRIVATE to `engine::mod` and not reachable
//    through the public API, so it cannot be exercised from an integration test. It
//    IS covered by the in-crate `#[cfg(test)]` unit tests in `src/engine/mod.rs`.
//    Here we exercise only the public routing surface.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn virtual_route_maps_to_virtual_port_and_channel() {
    let mut set = three_lane_set();
    set.lanes[0].route = Some(LaneRoute {
        port: PortRef::virtual_midip(),
        channel: 11,
        clock_out: true,
    });

    let eff = set.lanes[0].effective_route();
    assert!(
        eff.port.is_virtual(),
        "explicit virtual route must resolve to the virtual port"
    );
    assert_eq!(eff.channel, 11);
    // route_channel() is the allocation-free hot-path accessor the scheduler uses;
    // it must agree with effective_route().channel.
    assert_eq!(
        set.lanes[0].route_channel(),
        11,
        "route_channel must reflect the explicit route override"
    );

    // A lane WITHOUT an explicit route falls back to its profile channel and is
    // NOT virtual — confirming the virtual mapping is opt-in per lane.
    let default_eff = set.lanes[1].effective_route();
    assert!(!default_eff.port.is_virtual());
    assert_eq!(set.lanes[1].route_channel(), set.lanes[1].profile.channel);
}

#[test]
fn setroute_to_virtual_port_routes_through_engine_without_stuck_notes() {
    // Engine-level: while playing, switch a lane's route to the virtual port via
    // UiCommand::SetRoute. SetRoute releases the lane's sounding notes first, then
    // re-routes — no panic, playback continues on the new channel. Features
    // combined: live route change × virtual port × playback.
    let set = three_lane_set();
    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    let route_at = step * 4;
    let total = step * 16;

    let _ = run_engine_headless(
        set,
        &mut link,
        &mut sink,
        vec![
            (0, UiCommand::Play),
            (
                route_at,
                UiCommand::SetRoute {
                    lane: 1,
                    route: Some(LaneRoute {
                        port: PortRef::virtual_midip(),
                        channel: 5,
                        clock_out: true,
                    }),
                },
            ),
        ],
        total,
        1_000,
    );

    // After the re-route, bass-lane NoteOns must appear on the NEW channel (5),
    // proving the route change reached the scheduler's emission path.
    let on_ch5_after = note_ons(&sink)
        .into_iter()
        .filter(|(at, ch, _, _)| *at > route_at && *ch == 5)
        .count();
    assert!(
        on_ch5_after > 0,
        "after SetRoute to channel 5, bass NoteOns must emit on channel 5"
    );
}

// Self-check: the FakeLink step mapping used above is the shipped one.
#[test]
fn link_step_mapping_is_the_shipped_one() {
    assert_eq!(step_from_beat(0.0), 0);
    assert_eq!(step_from_beat(1.0), 4);
    assert_eq!(step_from_beat(4.0), 16);
}

// ═════════════════════════════════════════════════════════════════════════════
// 9. Chord survives save → load and plays with clean note release (M5b close-out).
//    Features combined: poly S-1 chord entry (MelodicStep ≥ 2 notes) ×
//    adaptive JSON serialization × store save_set/load_set round-trip ×
//    engine playback asserting 3 NoteOns per chord step × CC123 clean release.
// ═════════════════════════════════════════════════════════════════════════════

/// Build a chord MelodicStep from a slice of `(semi, len)` pairs.
fn chord_step(notes: &[(i8, f32)]) -> MelodicStep {
    MelodicStep::from(
        notes
            .iter()
            .map(|&(semi, len)| MelodicNote {
                semi,
                vel: 1.0,
                slide: false,
                len,
                prob: 1.0,
                ratchet: 1,
            })
            .collect::<Vec<_>>(),
    )
}

#[test]
fn chord_survives_save_load_and_plays_with_clean_release() {
    // ── 1. Build a Set with a poly S-1 lane containing a chord, a rest and a mono step ──
    let profs = profiles::default_profiles();
    // profs[2] is S-1 SYNTH (poly == true, channel 0, root_note 45).
    let s1_prof = profs[2];
    assert!(
        s1_prof.poly,
        "fixture assumes profs[2] is the poly S-1 profile"
    );

    // Pattern layout (16 steps):
    //   step  0 – C-major triad semis 0,4,7 (chord, 3 notes)
    //   step  4 – mono note semi 12
    //   step  8 – rest (MelodicStep::default(), 0 notes)
    //   all others – rest
    let mut steps: Vec<MelodicStep> = vec![MelodicStep::default(); 16];
    // step 0: C-major triad; step 4: mono; step 8: rest (default)
    steps[0] = chord_step(&[(0, 0.9), (4, 0.9), (7, 0.9)]);
    steps[4] = chord_step(&[(12, 0.5)]);

    let chord_pat = Pattern {
        name: "chord-test".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(steps),
        id: midip::persist::Id::nil(),
    };

    let set = Set {
        name: "chord-roundtrip".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![lane(s1_prof, chord_pat)],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
    };

    // ── 2. Save → load round-trip ──
    let dir = unique_dir("chord-roundtrip");
    let path = store::save_set(&dir, &mut set.clone()).unwrap();
    let loaded = store::load_set(&path).unwrap();

    // ── 3. Assert model survived adaptive serialization ──
    let loaded_steps = match &loaded.lanes[0].pattern.data {
        PatternData::Melodic(s) => s,
        _ => panic!("expected Melodic pattern after load"),
    };

    // Chord step: 3 notes with the original semis, in any order.
    let chord = &loaded_steps[0];
    assert_eq!(
        chord.len(),
        3,
        "chord step must have exactly 3 notes after round-trip; got {:?}",
        chord.iter().map(|n| n.semi).collect::<Vec<_>>()
    );
    let mut loaded_semis: Vec<i8> = chord.iter().map(|n| n.semi).collect();
    loaded_semis.sort_unstable();
    assert_eq!(
        loaded_semis,
        vec![0, 4, 7],
        "chord semis must survive adaptive serialize round-trip"
    );

    // Mono step: exactly 1 note.
    assert_eq!(
        loaded_steps[4].len(),
        1,
        "mono step must still have 1 note after round-trip"
    );
    assert_eq!(loaded_steps[4][0].semi, 12);

    // Rest step: 0 notes.
    assert_eq!(
        loaded_steps[8].len(),
        0,
        "rest step must remain empty after round-trip"
    );

    // ── 4. Play: drive the engine and assert 3 NoteOns for the chord + clean release ──
    // S-1: channel 0, root_note 45, transpose 0, octave 0.
    // resolve_melodic_pitch(45, semi, 0, 0) = 45 + semi.
    // Chord note numbers: 45 (semi 0), 49 (semi 4), 52 (semi 7).
    let expected_chord_notes: Vec<u8> = vec![45, 49, 52];

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();
    let step = step_dur_micros(120.0);
    // Run one full bar (16 steps) + a small margin, then stop.
    let total = step * 16;
    let stop_at = total; // stop command issued at the bar boundary

    run_engine_headless(
        loaded,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (stop_at, UiCommand::Stop)],
        total + step, // give one extra step so stop tick executes
        1_000,
    );

    // Assert all 3 chord notes fired on channel 0.
    let ons = note_ons(&sink);
    for &expected_note in &expected_chord_notes {
        let hits: Vec<_> = ons
            .iter()
            .filter(|(_, ch, n, _)| *ch == 0 && *n == expected_note)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly 1 NoteOn for chord note {expected_note} on ch 0, got {:?}",
            hits
        );
    }

    // Assert clean release: CC123 (All Notes Off) must have been emitted on
    // channel 0 at or after stop, proving release_all ran and no chord note is hung.
    let cc123_on_ch0 = sink.events.iter().any(|(at, m)| {
        *at >= stop_at
            && matches!(
                m,
                MidiMessage::ControlChange {
                    channel: 0,
                    controller: 123,
                    ..
                }
            )
    });
    assert!(
        cc123_on_ch0,
        "expected CC123 (all-notes-off) on S-1 channel 0 at/after stop — chord notes must be released cleanly"
    );

    // Clean up.
    let _ = std::fs::remove_dir_all(&dir);
}
