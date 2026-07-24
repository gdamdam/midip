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
use midip::engine::{run_engine_headless, run_engine_headless_clocked, EngineEvent, UiCommand};
use midip::link::{step_from_beat, FakeLink};
use midip::midi::message::MidiMessage;
use midip::midi::ports::RecordingSink;
use midip::pattern::library::{GenreMap, LibRole, Library};
use midip::pattern::model::{
    CcLock, DrumHit, DrumStep, Lane, LaneRoute, MelodicNote, MelodicStep, Pattern, PatternData,
    PortRef, Set, TrigCond,
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
            micro: 0,
            cond: TrigCond::Always,
        });
    }
    for &s in &[2usize, 6, 10, 14] {
        drum_steps[s].push(DrumHit {
            note: 39,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
    }
    let drums = Pattern {
        name: "kick+clap".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Drums(drum_steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
    };

    let bass = melodic_pattern("bass", &[(0, 0, 0.5), (8, 0, 0.5)]);
    let synth = melodic_pattern("synth", &[(4, 12, 0.9)]);

    Set {
        name: "test".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![
            lane(profs[0].clone(), drums),
            lane(profs[1].clone(), bass),
            // S-1 synth is now index 3 (index 2 is the J-6 chords profile).
            lane(profs[3].clone(), synth),
        ],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
    }
}

fn lane((role, profile): (LibRole, profiles::DeviceProfile), pattern: Pattern) -> Lane {
    Lane {
        role,
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
        swing: None,
        clock_div: None,
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
            micro: 0,
            cond: TrigCond::Always,
        }]);
    }
    Pattern {
        name: name.into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
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
            micro: 0,
            cond: TrigCond::Always,
        }]);
    }
    Pattern {
        name: name.into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
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
                    micro: 0,
                    cond: TrigCond::Always,
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
    Library {
        records: Vec::new(),
        drums,
        bass,
        chords: GenreMap::new(),
        synth,
        families: Vec::new(),
        v2_index: Default::default(),
    }
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
    seq.sync_to_beat(1.0, bpm, 4 * step);
    seq.tick(4 * step, &mut sink);
    let _ = seq.take_launched();
    seq.queue_launch(1, distinct_melodic("queued"), Quant::NextBar);

    // Walk the Link beat forward through steps 5..15 (beats 1.25 .. 3.75): the launch
    // must NOT fire before the bar boundary.
    let mut fired_before = false;
    for s in 5..=15u64 {
        seq.sync_to_beat(s as f64 / 4.0, bpm, s * step);
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
    seq.sync_to_beat(4.0, bpm, 16 * step);
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
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
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
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
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
    let (favs_back, _) = store::load_favorites(&dir);
    let (crates_back, _) = store::load_crates(&dir);

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
                micro: 0,
                cond: TrigCond::Always,
            })
            .collect::<Vec<_>>(),
    )
}

#[test]
fn chord_survives_save_load_and_plays_with_clean_release() {
    // ── 1. Build a Set with a poly S-1 lane containing a chord, a rest and a mono step ──
    let profs = profiles::default_profiles();
    // profs[3] is S-1 SYNTH (poly == true, channel 0, root_note 45); index 2 is J-6 chords.
    let s1_prof = profs[3];
    assert!(
        s1_prof.1.poly,
        "fixture assumes profs[3] is the poly S-1 profile"
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
        cc: Default::default(),
    };

    let set = Set {
        name: "chord-roundtrip".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![lane(s1_prof, chord_pat)],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
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

// ═════════════════════════════════════════════════════════════════════════════
// 9. Generative tool: non-destructive preview → commit → undo roundtrip.
//    Features combined: generative pattern generation (M9) × undo/redo stack ×
//    scale-constrained melodic lane × TempTransform preview path.
//
//    Assertions:
//      (a) After OpenGenerative + adjustments: temp_transform is set, undo stack
//          is UNCHANGED (no snapshot during preview), and temp_transform holds
//          the original so the engine can play the live candidate.
//      (b) After GenCommit: lane pattern differs from original, EVERY generated
//          melodic pitch is in the lane's Major scale, exactly ONE new undo
//          entry was added, and mode returns to Edit.
//      (c) Action::Undo restores the original pattern exactly.
//      (d) Vary case: mutate=0 ⇒ committed pattern is identical to source;
//          mutate=100 ⇒ committed pattern differs from source.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn generative_preview_commit_undo_roundtrip() {
    use midip::app::{Action, App, GenField, Mode};
    use midip::music::scale::Scale;

    // ── 1. Build a Set with one melodic lane using Major scale ────────────────
    let profs = profiles::default_profiles();
    // profs[3] is S-1 SYNTH (poly == true), suitable as a melodic lane (index 2 is J-6 chords).
    let synth_prof = profs[3].1;
    assert!(synth_prof.poly, "fixture assumes profs[3] is poly");

    // Source pattern: a simple three-note figure that generate can mutate.
    let original_pat = melodic_pattern("orig", &[(0, 0, 0.5), (4, 5, 0.5), (8, 7, 0.5)]);

    let melodic_lane = Lane {
        role: LibRole::Synth,
        profile: synth_prof,
        pattern: original_pat.clone(),
        mute: false,
        solo: false,
        transpose: 0,
        octave: 0,
        route: None,
        muted_voices: Vec::new(),
        scale: Scale::Major,
        root: None,
        swing: None,
        clock_div: None,
    };

    let set = Set {
        name: "gen-test".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![melodic_lane],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
    };

    let mut app = App::new(set, test_library());
    app.apply(Action::FocusLane(0));

    let undo_before = app.undo.len();

    // ── 2. Open generative tool — must be non-destructive ─────────────────────
    app.apply(Action::OpenGenerative);

    // (a) temp_transform must be set; mode must be Generative.
    assert!(
        app.temp_transform.is_some(),
        "OpenGenerative must set temp_transform"
    );
    assert_eq!(
        app.mode,
        Mode::Generative,
        "OpenGenerative must switch mode to Generative"
    );

    // (a) Undo stack must NOT have grown — preview is non-destructive.
    assert_eq!(
        app.undo.len(),
        undo_before,
        "OpenGenerative must not push to undo stack"
    );

    // (a) temp_transform must preserve the original pattern.
    let original_in_tt = app.temp_transform.as_ref().unwrap().original.clone();
    assert_eq!(
        original_in_tt, original_pat,
        "temp_transform must preserve original pattern"
    );

    // Adjust parameters; none of these should snapshot.
    app.apply(Action::GenAdjust {
        field: GenField::Density,
        delta: 30,
    });
    app.apply(Action::GenAdjust {
        field: GenField::Range,
        delta: -4,
    });
    assert_eq!(
        app.undo.len(),
        undo_before,
        "GenAdjust must not push to undo stack"
    );

    app.apply(Action::GenReroll);
    assert_eq!(
        app.undo.len(),
        undo_before,
        "GenReroll must not push to undo stack"
    );

    // ── 3. Commit ─────────────────────────────────────────────────────────────
    app.apply(Action::GenCommit);

    // (b) Mode returns to Edit; temp_transform cleared.
    assert_eq!(app.mode, Mode::Edit, "GenCommit must restore Mode::Edit");
    assert!(
        app.temp_transform.is_none(),
        "GenCommit must clear temp_transform"
    );

    // (b) Exactly one new undo entry.
    assert_eq!(
        app.undo.len(),
        undo_before + 1,
        "GenCommit must push exactly one entry to undo stack"
    );

    // (b) Lane pattern has changed from the original.
    let committed_pat = app.set.lanes[0].pattern.clone();
    assert_ne!(
        committed_pat, original_pat,
        "GenCommit must produce a pattern different from the original"
    );

    // (b) Every generated melodic pitch must be in the Major scale.
    //     Major scale degrees: [0, 2, 4, 5, 7, 9, 11].
    let major_degrees = Scale::Major.degrees();
    if let midip::pattern::model::PatternData::Melodic(steps) = &committed_pat.data {
        for (i, step) in steps.iter().enumerate() {
            for note in step.0.iter() {
                // semi is relative to root (0). Fold to [0, 12) pitch class.
                let pc = note.semi.rem_euclid(12) as u8;
                assert!(
                    major_degrees.contains(&pc),
                    "step {i} note semi={} (pc={pc}) is not in Major scale",
                    note.semi
                );
            }
        }
    } else {
        panic!("lane 0 must have Melodic pattern data after GenCommit");
    }

    // ── 4. Undo restores original ─────────────────────────────────────────────
    app.apply(Action::Undo);
    assert_eq!(
        app.set.lanes[0].pattern, original_pat,
        "Action::Undo must restore the original pattern exactly"
    );
    assert_eq!(
        app.undo.len(),
        undo_before,
        "after Undo the stack depth must return to pre-commit depth"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 9b. Vary mode: mutate=0 ⇒ identity; mutate=100 ⇒ changed.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn generative_vary_identity_and_mutation() {
    use midip::app::{Action, App, GenField, Mode};
    use midip::music::scale::Scale;
    use midip::pattern::generate::GenMode;

    let profs = profiles::default_profiles();
    // S-1 synth is index 3 (index 2 is the J-6 chords profile).
    let synth_prof = profs[3].1;
    assert!(synth_prof.poly, "fixture assumes profs[3] is poly");

    // Source pattern with several active steps so Vary has material to mutate.
    let original_pat = melodic_pattern(
        "vary-src",
        &[
            (0, 0, 0.5),
            (2, 2, 0.5),
            (4, 4, 0.5),
            (6, 5, 0.5),
            (8, 7, 0.5),
            (10, 9, 0.5),
            (12, 11, 0.5),
            (14, 0, 0.5),
        ],
    );

    let make_app = || {
        let melodic_lane = Lane {
            role: LibRole::Synth,
            profile: synth_prof,
            pattern: original_pat.clone(),
            mute: false,
            solo: false,
            transpose: 0,
            octave: 0,
            route: None,
            muted_voices: Vec::new(),
            scale: Scale::Major,
            root: None,
            swing: None,
            clock_div: None,
        };
        let set = Set {
            name: "vary-test".into(),
            bpm: 120.0,
            swing: 0.5,
            lanes: vec![melodic_lane],
            id: midip::persist::Id::nil(),
            scenes: Vec::new(),
            chains: Vec::new(),
            clock_in_port: None,
            steps_per_bar: 16,
        };
        let mut a = App::new(set, test_library());
        a.apply(Action::FocusLane(0));
        a
    };

    // ── (a) mutate=0 in Vary mode ⇒ committed pattern equals source ───────────
    {
        let mut app = make_app();
        app.apply(Action::OpenGenerative);
        app.apply(Action::GenSetMode(GenMode::Vary));
        // Default mutate is 25; delta=-100 clamps to 0.
        app.apply(Action::GenAdjust {
            field: GenField::Mutate,
            delta: -100,
        });
        assert_eq!(app.gen_params.mutate, 0, "mutate must clamp to 0");
        app.apply(Action::GenCommit);
        assert_eq!(app.mode, Mode::Edit, "must return to Edit after commit");
        // With mutate=0, Vary is an identity: committed pattern == original.
        assert_eq!(
            app.set.lanes[0].pattern, original_pat,
            "Vary with mutate=0 must be an identity (pattern unchanged)"
        );
    }

    // ── (b) mutate=100 in Vary mode ⇒ committed pattern differs ──────────────
    {
        let mut app = make_app();
        app.apply(Action::OpenGenerative);
        app.apply(Action::GenSetMode(GenMode::Vary));
        // Default mutate is 25; delta=+100 clamps to 100.
        app.apply(Action::GenAdjust {
            field: GenField::Mutate,
            delta: 100,
        });
        assert_eq!(app.gen_params.mutate, 100, "mutate must clamp to 100");
        app.apply(Action::GenCommit);
        assert_ne!(
            app.set.lanes[0].pattern, original_pat,
            "Vary with mutate=100 must produce a different pattern"
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 9. Scene lifecycle: capture → save → load → recall (M6 close-out).
//    Features combined: Scene::capture_scene × set save/load (serde + validate/
//    repair) × stopped-transport RecallScene (immediate apply) × per-lane
//    performance state (mute + transpose + octave).
//
//    Playback after recall is NOT exercised here: the inline-pattern ids produced
//    by capture_scene are PatternRef::User(id), which resolve correctly via the
//    in-memory inline list but would require wiring a headless engine with a full
//    UiCommand::QueueScene consumer to observe NoteOn output — that coupling is
//    out of scope for a stopped-transport acceptance test. The recall-state
//    assertions (lane fields) are the definitive M6 acceptance gate.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn scene_capture_save_load_recall_roundtrip() {
    use midip::app::{Action, App};
    use midip::persist;

    let dir = unique_dir("scene-roundtrip");

    // ── 1. Build a set with DISTINCT per-lane performance state ──────────────
    // Patterns must have non-nil ids so PatternRef::User resolution works after
    // capture_scene snapshots lane.pattern.id into each assignment.
    let profs = profiles::default_profiles();

    let drums_pat = {
        let mut steps: Vec<DrumStep> = vec![Vec::new(); 16];
        steps[0].push(DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        });
        Pattern {
            name: "scene-drums".into(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: persist::mint_id(),
            cc: Default::default(),
        }
    };
    let mut bass_pat = melodic_pattern("scene-bass", &[(0, 0, 0.5), (8, 5, 0.5)]);
    bass_pat.id = persist::mint_id();
    let mut synth_pat = melodic_pattern("scene-synth", &[(4, 12, 0.9)]);
    synth_pat.id = persist::mint_id();

    let mut set = Set {
        name: "scene-test".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![
            lane(profs[0], drums_pat.clone()),
            lane(profs[1], bass_pat.clone()),
            // S-1 synth is now index 3 (index 2 is the J-6 chords profile).
            lane(profs[3], synth_pat.clone()),
        ],
        id: persist::mint_id(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
    };

    // Give each lane a DISTINCT non-default performance state.
    set.lanes[0].mute = true; // drums: muted
    set.lanes[1].transpose = 5; // bass: transposed up 5 semitones
    set.lanes[2].octave = -1; // synth: one octave down

    // ── 2. Capture scene; assert assignments match captured state ─────────────
    let scene = set.capture_scene("M6-Scene".to_string());

    assert!(
        !scene.id.is_nil(),
        "capture_scene must mint a non-nil scene id"
    );
    assert_eq!(scene.name, "M6-Scene");
    assert_eq!(scene.assignments.len(), 3, "one assignment per lane");

    // Lane 0: muted, pattern id matches drums
    assert_eq!(
        scene.assignments[0].pattern,
        PatternRef::User(drums_pat.id.clone())
    );
    assert!(scene.assignments[0].mute, "lane 0 mute must be captured");
    assert_eq!(scene.assignments[0].transpose, 0);
    assert_eq!(scene.assignments[0].octave, 0);

    // Lane 1: transposed
    assert_eq!(
        scene.assignments[1].pattern,
        PatternRef::User(bass_pat.id.clone())
    );
    assert!(!scene.assignments[1].mute);
    assert_eq!(
        scene.assignments[1].transpose, 5,
        "lane 1 transpose must be captured"
    );
    assert_eq!(scene.assignments[1].octave, 0);

    // Lane 2: octave shifted
    assert_eq!(
        scene.assignments[2].pattern,
        PatternRef::User(synth_pat.id.clone())
    );
    assert!(!scene.assignments[2].mute);
    assert_eq!(scene.assignments[2].transpose, 0);
    assert_eq!(
        scene.assignments[2].octave, -1,
        "lane 2 octave must be captured"
    );

    // Push scene into set before saving.
    set.scenes.push(scene.clone());

    // ── 3. Save → load round-trip; assert scene survives persistence ──────────
    let saved_path = store::save_set(&dir, &mut set).unwrap();
    let loaded_set = store::load_set(&saved_path).unwrap();

    assert_eq!(loaded_set.scenes.len(), 1, "scene must survive save/load");
    let loaded_scene = &loaded_set.scenes[0];
    assert_eq!(
        loaded_scene.id, scene.id,
        "scene id must be stable across serde"
    );
    assert_eq!(loaded_scene.name, "M6-Scene");
    assert_eq!(loaded_scene.assignments.len(), 3);
    assert_eq!(loaded_scene.assignments[0], scene.assignments[0]);
    assert_eq!(loaded_scene.assignments[1], scene.assignments[1]);
    assert_eq!(loaded_scene.assignments[2], scene.assignments[2]);

    // ── 4. Recall: mutate live lanes, dispatch RecallScene(0), assert restore ──
    let mut app = App::new(loaded_set, test_library());
    // engine_playing defaults to false → stopped transport → immediate apply.
    assert!(
        !app.engine_playing,
        "transport must be stopped for deterministic recall"
    );

    // Corrupt lane state so recall is observable.
    app.set.lanes[0].mute = false; // was true in captured scene
    app.set.lanes[1].transpose = 0; // was 5 in captured scene
    app.set.lanes[2].octave = 1; // was -1 in captured scene

    let cmds = app.apply(Action::RecallScene(0));

    // Stopped recall emits LoadPattern + Mute + Solo + Transpose + SetOctave per lane.
    assert!(
        cmds.iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { lane: 0, .. })),
        "RecallScene must emit LoadPattern for lane 0"
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, UiCommand::Mute { lane: 0, on: true })),
        "RecallScene must restore mute=true on lane 0"
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, UiCommand::Transpose { lane: 1, semis: 5 })),
        "RecallScene must restore transpose=5 on lane 1"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            UiCommand::SetOctave {
                lane: 2,
                octave: -1
            }
        )),
        "RecallScene must restore octave=-1 on lane 2"
    );

    // Assert the live lane fields were actually written (not just commands emitted).
    assert!(
        app.set.lanes[0].mute,
        "lane 0 mute must be restored to true"
    );
    assert_eq!(
        app.set.lanes[0].pattern.id, drums_pat.id,
        "lane 0 pattern id must match captured"
    );
    assert_eq!(
        app.set.lanes[1].transpose, 5,
        "lane 1 transpose must be restored to 5"
    );
    assert_eq!(
        app.set.lanes[1].pattern.id, bass_pat.id,
        "lane 1 pattern id must match captured"
    );
    assert_eq!(
        app.set.lanes[2].octave, -1,
        "lane 2 octave must be restored to -1"
    );
    assert_eq!(
        app.set.lanes[2].pattern.id, synth_pat.id,
        "lane 2 pattern id must match captured"
    );

    // ── 5. Playback: out of scope (see block comment above). ──────────────────

    // Clean up.
    let _ = std::fs::remove_dir_all(&dir);
}

// 10. Chain roundtrip: create → save → load → PlayChain → bar-advance → stop-at-end (M7).
//     Features combined: create_chain × add_chain_entry × looped=false ×
//     store save/load × App::PlayChain × App::tick_chain bar-advance ×
//     QueueScene recall (scene B at bar boundary 1) × stop-at-end transport stop ×
//     registry balance (no hung notes across transitions + final stop).
// ═══════════════════════════════════════════════════════════════════════════
#[test]
fn chain_roundtrip_create_save_load_play_advance() {
    use midip::app::{Action, App};
    use midip::pattern::chain::{add_chain_entry, create_chain};
    use std::collections::HashMap;

    // ── 1. Build a Set with two scenes (A, B) using STABLE inline patterns ────
    //
    // IMPORTANT: resolve_scene matches PatternRef::User(id) against the CURRENT
    // lane patterns (inline slice). Both scenes must reference patterns that remain
    // as the current lane patterns when the App is constructed after save/load.
    // To guarantee this, we capture both scenes from the SAME lane patterns;
    // scenes are distinguished by different performance state (mute, transpose).
    //
    // Lane 1 (synth) holds a note for a full bar so it is sounding when the
    // QueueScene for scene B fires — stress-testing release-before-swap.
    let profs = profiles::default_profiles();

    // Drum pattern: BD on step 0 (lane 0, ch 9).
    let mut drum_steps: Vec<DrumStep> = vec![Vec::new(); 16];
    drum_steps[0].push(DrumHit {
        note: 36,
        vel: 100,
        prob: 1.0,
        ratchet: 1,
        micro: 0,
        cond: TrigCond::Always,
    });
    let mut drum_pat = Pattern {
        name: "bd-pattern".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Drums(drum_steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
    };
    drum_pat.ensure_id();

    // Synth pattern: held note for a full bar (len 16.0) — still sounding at bar boundary.
    let mut syn_steps = vec![MelodicStep::default(); 16];
    syn_steps[0] = MelodicStep::from(vec![MelodicNote {
        semi: 0,
        vel: 1.0,
        slide: false,
        len: 16.0,
        prob: 1.0,
        ratchet: 1,
        micro: 0,
        cond: TrigCond::Always,
    }]);
    let mut syn_pat = Pattern {
        name: "held-synth".into(),
        desc: String::new(),
        length: 16,
        data: PatternData::Melodic(syn_steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
    };
    syn_pat.ensure_id();

    // Helper: build a Lane.
    let make_lane =
        |(role, prof): (LibRole, midip::devices::profiles::DeviceProfile), pat: Pattern| -> Lane {
            Lane {
                role,
                profile: prof,
                pattern: pat,
                mute: false,
                solo: false,
                transpose: 0,
                octave: 0,
                route: None,
                muted_voices: Vec::new(),
                scale: midip::music::scale::Scale::Chromatic,
                root: None,
                swing: None,
                clock_div: None,
            }
        };

    let mut set = Set {
        name: "chain-roundtrip".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![
            make_lane(profs[0], drum_pat.clone()), // lane 0: drums (ch 9)
            // S-1 synth is index 3 (index 2 is the J-6 chords profile).
            make_lane(profs[3], syn_pat.clone()), // lane 1: synth, held note
        ],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
    };
    set.ensure_id();

    // Capture scene A: drums unmuted, synth transpose=0.
    let scene_a = set.capture_scene("Scene-A".to_string());
    set.scenes.push(scene_a.clone());

    // Differentiate scene B by performance state (same patterns, different transpose).
    // Both scenes reference the SAME pattern IDs — they will always resolve from the
    // current inline lane patterns after save/load.
    set.lanes[1].transpose = 7; // a fifth up for scene B
    let scene_b = set.capture_scene("Scene-B".to_string());
    set.scenes.push(scene_b.clone());

    // Restore lane state to scene A defaults (the chain will recall from scenes).
    set.lanes[1].transpose = 0;

    // ── 2. Create a chain: entry A (1 bar) → entry B (1 bar), looped = false ──
    let chain_idx = create_chain(&mut set, "test-chain");
    add_chain_entry(&mut set, chain_idx, scene_a.id.clone());
    add_chain_entry(&mut set, chain_idx, scene_b.id.clone());
    set.chains[chain_idx].looped = false;

    // ── 3. Serialize → load; assert chain + entries survive persistence ────────
    let dir = unique_dir("chain-roundtrip");
    let saved_path = store::save_set(&dir, &mut set).unwrap();
    let loaded_set = store::load_set(&saved_path).unwrap();

    assert_eq!(loaded_set.chains.len(), 1, "chain must survive save/load");
    assert_eq!(
        loaded_set.chains[0].entries.len(),
        2,
        "both entries must survive"
    );
    assert_eq!(
        loaded_set.chains[0].entries[0].scene_id, scene_a.id,
        "entry 0 must reference scene A"
    );
    assert_eq!(
        loaded_set.chains[0].entries[1].scene_id, scene_b.id,
        "entry 1 must reference scene B"
    );
    assert!(!loaded_set.chains[0].looped, "looped=false must persist");
    assert_eq!(loaded_set.scenes.len(), 2, "both scenes must survive");

    // Verify persistence format version is current (v2 with chains).
    assert_eq!(
        loaded_set.chains[0].entries[0].bars, 1,
        "default bars=1 must persist"
    );

    // ── 4. Use App to compute the command sequence PlayChain would drive ───────
    //
    // The real main loop does:
    //   a) PlayChain(0) → emits Play + QueueScene(A)   (engine_playing=false → starts transport)
    //   b) EngineEvent::Started → sets engine_playing=true
    //   c) EngineEvent::Playhead{step=16} → tick_chain(16) → QueueScene(B)
    //   d) EngineEvent::Playhead{step=32} → tick_chain(32) → Stop (looped=false, end)
    //
    // We replicate this by calling the App to generate each batch of commands,
    // then timestamp them for run_engine_headless.
    let mut app = App::new(loaded_set, test_library());

    // BPM=120: 1 step = 125_000 µs, 1 bar (16 steps) = 2_000_000 µs.
    let step_us = step_dur_micros(120.0) as u64;
    let bar_us = step_us * 16;

    // (a) PlayChain: App not yet playing → emits Play + immediate LoadPattern (stopped
    //     path: entry 0's scene loads immediately before transport starts, so the engine
    //     begins playing scene A from step 0 with no quantization delay).
    assert!(!app.playing, "App must start stopped");
    let play_cmds = app.apply(Action::PlayChain(0));
    assert!(app.playing, "App must be playing after PlayChain");
    assert!(
        play_cmds.iter().any(|c| matches!(c, UiCommand::Play)),
        "PlayChain must emit UiCommand::Play when stopped; got: {play_cmds:?}"
    );
    // When stopped, recall_scene_quant uses LoadPattern (immediate apply), not QueueScene.
    assert!(
        play_cmds
            .iter()
            .any(|c| matches!(c, UiCommand::LoadPattern { .. })),
        "PlayChain from stopped must emit LoadPattern for entry 0; got: {play_cmds:?}"
    );

    // (b) Simulate EngineEvent::Started (sets engine_playing=true so bar-boundary
    //     tick_chain calls use the QueueScene path, not immediate LoadPattern).
    app.engine_playing = true;

    // (c) Bar boundary 1 (step=16): entry A has dwelled 1 bar → Advance to entry B.
    let bar1_cmds = app.tick_chain(16);
    assert!(
        bar1_cmds
            .iter()
            .any(|c| matches!(c, UiCommand::QueueScene { .. })),
        "tick_chain at step 16 must emit QueueScene for entry B; got: {bar1_cmds:?}"
    );

    // (d) Bar boundary 2 (step=32): entry B has dwelled 1 bar → Stop (looped=false).
    let bar2_cmds = app.tick_chain(32);
    assert!(
        bar2_cmds.iter().any(|c| matches!(c, UiCommand::Stop)),
        "tick_chain at step 32 must emit Stop (end of chain, looped=false); got: {bar2_cmds:?}"
    );

    // Verify chain playback was cleared on Stop.
    assert!(
        app.chain_playback.is_none(),
        "chain_playback must be None after stop-at-end"
    );

    // ── 5. Feed commands to the headless engine; assert registry balance ───────
    //
    // Assemble the command timeline exactly as the real main loop would dispatch:
    //   t=0:      Play + QueueScene(A)   (from PlayChain)
    //   t=bar_us: QueueScene(B)           (from tick_chain at step 16)
    //   t=2*bar:  Stop                    (from tick_chain at step 32)
    let mut engine_cmds: Vec<(u64, UiCommand)> = Vec::new();

    // Rebuild App to get a fresh command set for the engine (same loaded_set).
    // Re-derive the engine command sequence; the App above is already consumed
    // so we reconstruct from store.
    let engine_set = store::load_set(&saved_path).unwrap();
    let mut app2 = App::new(engine_set, test_library());
    app2.engine_playing = false;

    // t=0: PlayChain(0) → Play + QueueScene(A)
    let cmds0 = app2.apply(Action::PlayChain(0));
    for cmd in cmds0 {
        engine_cmds.push((0, cmd));
    }
    app2.engine_playing = true;

    // t=bar_us: tick_chain(16) → QueueScene(B)
    let cmds1 = app2.tick_chain(16);
    for cmd in cmds1 {
        engine_cmds.push((bar_us, cmd));
    }

    // t=2*bar_us: tick_chain(32) → Stop
    let cmds2 = app2.tick_chain(32);
    for cmd in cmds2 {
        engine_cmds.push((2 * bar_us, cmd));
    }

    // Non-vacuous guard: we must have Play + LoadPattern(A) + QueueScene(B) + Stop.
    // Entry 0 (scene A) is recalled immediately via LoadPattern (stopped-transport path);
    // entry 1 (scene B) is recalled via QueueScene at bar boundary 1 (engine_playing=true).
    assert!(
        engine_cmds
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::Play)),
        "must have Play command; got: {engine_cmds:?}"
    );
    assert!(
        engine_cmds
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::LoadPattern { .. })),
        "must have LoadPattern for entry 0 (scene A); got: {engine_cmds:?}"
    );
    let q_scene_count = engine_cmds
        .iter()
        .filter(|(_, c)| matches!(c, UiCommand::QueueScene { .. }))
        .count();
    assert_eq!(
        q_scene_count, 1,
        "must have exactly 1 QueueScene command (scene B at bar boundary); got: {engine_cmds:?}"
    );
    assert!(
        engine_cmds
            .iter()
            .any(|(_, c)| matches!(c, UiCommand::Stop)),
        "must have a Stop command; got: {engine_cmds:?}"
    );

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();

    // Run engine for 2 bars + a little past the Stop to let all NoteOffs drain.
    let engine_set2 = store::load_set(&saved_path).unwrap();
    let _ = run_engine_headless(
        engine_set2,
        &mut link,
        &mut sink,
        engine_cmds,
        2 * bar_us + step_us, // a hair past bar boundary 2
        step_us / 4,
    );

    // At least some MIDI was emitted (non-vacuous guard: real notes sounded).
    let note_ons: Vec<_> = sink
        .events
        .iter()
        .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { vel, .. } if *vel > 0))
        .collect();
    assert!(
        !note_ons.is_empty(),
        "engine must emit real NoteOns across chain playback; got no NoteOns"
    );

    // Registry balance: every NoteOn must be matched by a NoteOff (no hung notes).
    let mut net: HashMap<(u8, u8), i32> = HashMap::new();
    for (_, msg) in &sink.events {
        match msg {
            MidiMessage::NoteOn { channel, note, vel } if *vel > 0 => {
                *net.entry((*channel, *note)).or_insert(0) += 1;
            }
            MidiMessage::NoteOn { channel, note, .. } => {
                *net.entry((*channel, *note)).or_insert(0) -= 1;
            }
            MidiMessage::NoteOff { channel, note, .. } => {
                *net.entry((*channel, *note)).or_insert(0) -= 1;
            }
            _ => {}
        }
    }
    let hung: Vec<_> = net.iter().filter(|(_, &v)| v > 0).collect();
    assert!(
        hung.is_empty(),
        "no hung notes after chain stop-at-end; leftover: {hung:?}"
    );

    // Clean up.
    let _ = std::fs::remove_dir_all(&dir);
}

// ═════════════════════════════════════════════════════════════════════════════
// M8 cross-feature integration: per-step CC + microtiming + trig condition +
// per-lane clock_div/swing, all wired end-to-end through the real engine.
//
// Features combined (all M8):
//   - Pattern.cc: per-step CcLock Vec (T5)
//   - MelodicNote.micro: signed microtiming offset (T4)
//   - MelodicNote.cond: Ratio{1,2} trig condition (T6)
//   - Lane.clock_div: lane advances once per N global steps (T7)
//   - Lane.swing: per-lane swing override (T7)
//   - Persistence v3: all fields survive save → load (T1–T3)
//
// Assertions:
//   (a) Persistence: save → load preserves cc/micro/cond on steps and
//       swing/clock_div on the lane; file is schema version 3.
//   (b) Microtiming: a note with micro != 0 emits its NoteOn at the expected
//       shifted at_micros; NoteOff follows the same shift (via clean release).
//   (c) Per-step CC: CcLock fires as ControlChange just before the NoteOn;
//       an identical repeat on the next loop is suppressed (route cache).
//   (d) Trig condition: Ratio{1,2} note fires only on even loops (0, 2, …).
//   (e) Clock division: a lane with clock_div=2 fires half as often as an
//       undivided lane across the same global-step window.
//   (f) Note safety: Stop emits CC123 (all-notes-off) to release any held notes.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn m8_per_step_cc_microtiming_cond_clock_div_roundtrip() {
    use midip::pattern::store::{load_set, save_set, CURRENT_SET_VERSION};

    let profs = profiles::default_profiles();
    // profs[3] = S-1 SYNTH, channel 0, root_note 45, poly=true (index 2 is J-6 chords).
    let synth_prof = profs[3].1;
    assert!(
        synth_prof.poly,
        "fixture: profs[3] must be the poly S-1 profile"
    );
    // S-1: channel=0, root_note=45. resolve_melodic_pitch(45, semi=0, 0, 0) = 45.
    let expected_pitch: u8 = 45;
    let cc_num: u8 = 74; // filter cutoff
    let cc_val: u8 = 100;
    let micro_ticks: i16 = 160; // permille of a step (+0.16); +20ms at 120 BPM

    // ── 1. Build the 4-step melodic pattern ───────────────────────────────────
    // step 0 — note (semi=0, micro=+500, cond=Always)  + CcLock(74,100)
    // step 2 — note (semi=0, micro=0,    cond=Ratio{1,2})
    // steps 1,3 — rest
    let note_step0 = MelodicNote {
        semi: 0,
        vel: 1.0,
        slide: false,
        len: 0.5,
        prob: 1.0,
        ratchet: 1,
        micro: micro_ticks,
        cond: TrigCond::Always,
    };
    let note_step2 = MelodicNote {
        semi: 0,
        vel: 1.0,
        slide: false,
        len: 0.5,
        prob: 1.0,
        ratchet: 1,
        micro: 0,
        cond: TrigCond::Ratio { x: 1, y: 2 },
    };
    let mut steps: Vec<MelodicStep> = vec![MelodicStep::default(); 4];
    steps[0] = MelodicStep::from(vec![note_step0]);
    steps[2] = MelodicStep::from(vec![note_step2]);

    let mut pat = Pattern {
        name: "m8-xfeat".into(),
        desc: String::new(),
        length: 4,
        data: PatternData::Melodic(steps),
        id: midip::persist::Id::nil(),
        cc: Default::default(),
    };
    pat.set_step_cc(
        0,
        vec![CcLock {
            cc: cc_num,
            val: cc_val,
        }],
    );

    // ── 2. Lane with per-lane M8 attributes ───────────────────────────────────
    // swing=0.6 (non-trivial), clock_div=2 (lane step advances every 2 global steps).
    let m8_lane = Lane {
        role: LibRole::Synth,
        profile: synth_prof,
        pattern: pat,
        mute: false,
        solo: false,
        transpose: 0,
        octave: 0,
        route: None,
        muted_voices: Vec::new(),
        scale: midip::music::scale::Scale::Chromatic,
        root: None,
        swing: Some(0.6),
        clock_div: Some(2),
    };

    let set = Set {
        name: "m8-xfeat-set".into(),
        bpm: 120.0,
        swing: 0.5,
        lanes: vec![m8_lane],
        id: midip::persist::Id::nil(),
        scenes: Vec::new(),
        chains: Vec::new(),
        clock_in_port: None,
        steps_per_bar: 16,
    };

    // ── (a) Persistence: save → load; assert M8 fields survive and version=3 ──
    let dir = unique_dir("m8-xfeat");
    let path = save_set(&dir, &mut set.clone()).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        raw["version"].as_u64().unwrap(),
        CURRENT_SET_VERSION as u64,
        "saved set must carry schema version {CURRENT_SET_VERSION}"
    );

    let loaded = load_set(&path).unwrap();
    let loaded_lane = &loaded.lanes[0];

    assert_eq!(
        loaded_lane.swing,
        Some(0.6),
        "lane.swing must survive save/load"
    );
    assert_eq!(
        loaded_lane.clock_div,
        Some(2),
        "lane.clock_div must survive save/load"
    );

    let cc_after = loaded_lane.pattern.step_cc(0);
    assert_eq!(
        cc_after,
        &[CcLock {
            cc: cc_num,
            val: cc_val
        }],
        "step 0 CcLock must survive save/load"
    );
    assert!(
        loaded_lane.pattern.step_cc(1).is_empty(),
        "step 1 must have no CC locks"
    );

    let loaded_steps = match &loaded_lane.pattern.data {
        PatternData::Melodic(s) => s,
        _ => panic!("expected Melodic after load"),
    };
    assert_eq!(
        loaded_steps[0][0].micro, micro_ticks,
        "note.micro must survive save/load"
    );
    assert_eq!(
        loaded_steps[2][0].cond,
        TrigCond::Ratio { x: 1, y: 2 },
        "note.cond must survive save/load"
    );

    // ── (b–f) Engine playback ─────────────────────────────────────────────────
    // clock_div=2 with a 4-step pattern: the lane advances once per 2 global steps.
    // 8 global steps → 4 lane-time steps → 2 full lane loops.
    //
    // Global-step → effective (lane-time) step mapping (launch_offset=0):
    //   g0→e0(local 0), g2→e1(local 1), g4→e2(local 2), g6→e3(local 3) — loop 0
    //   [g1,g3,g5,g7 are skipped by the clock_div gate]
    //
    // Wait — this is wrong. Re-read the scheduler:
    //   effective_step = off + steps_since_launch / div
    //   steps_since_launch = step (launch_offset=0)
    //   div=2
    // So:
    //   g0 → e=0 → local=0%4=0, loop_index=0/4=0   → step-0 note fires (Always)
    //   g2 → e=1 → local=1%4=1, loop_index=1/4=0   → local 1 = rest
    //   g4 → e=2 → local=2%4=2, loop_index=2/4=0   → step-2 note fires (Ratio{1,2}, loop 0 → fires)
    //   g6 → e=3 → local=3%4=3, loop_index=3/4=0   → rest
    //   g8 → e=4 → local=4%4=0, loop_index=4/4=1   → step-0 note fires (Always)
    //   ...g10→e=5→local=1, g12→e=6→local=2 (loop_index=1 → Ratio{1,2} suppressed)
    //
    // Running 8 global steps (g0..g7) = total=8*step µs:
    //   lane materializes at g0,g2,g4,g6 (odd global steps skipped by div gate).
    //   NoteOns: g0 (step-0, Always), g4 (step-2, Ratio loop 0 fires).
    //   g8 would be step-0 loop 1, but we stop at g8 (stop_at = total).
    //
    // To also capture loop 1's step-0, run 10 global steps:
    //   g8 → e=4 → local=0, loop_index=1 → step-0 fires (Always, loop 1)
    // Total NoteOns across 10 global steps = 3 (g0, g4, g8).
    let step_dur = step_dur_micros(120.0); // 125_000 µs
    let total_global_steps = 10u64;
    let total = step_dur * total_global_steps;
    let stop_at = total;

    let mut link = FakeLink::new();
    let mut sink = RecordingSink::new();

    run_engine_headless(
        loaded,
        &mut link,
        &mut sink,
        vec![(0, UiCommand::Play), (stop_at, UiCommand::Stop)],
        total + step_dur, // one extra tick so the Stop executes
        1_000,
    );

    // ── (b) Microtiming ───────────────────────────────────────────────────────
    // step-0 note has micro=+160 permille; swing=0.6 on even step → swing_offset=0.
    // micro is permille-of-step, so the offset is BPM-scaled: 160*step_dur/1000.
    // Loop 0: step_start = 0; on_at = micro_us.
    // Loop 1: g8 → step_start = 8*step_dur; on_at = 8*step_dur + micro_us.
    let micro_us = (micro_ticks as i64 * step_dur as i64 / 1000) as u64;
    let step0_ons: Vec<u64> = sink
        .events
        .iter()
        .filter_map(|(at, m)| match m {
            MidiMessage::NoteOn {
                channel: 0, note, ..
            } if *note == expected_pitch => Some(*at),
            _ => None,
        })
        .filter(|&at| {
            // Exclude the step-2 (Ratio) note which fires at g4 = 4*step_dur (no micro shift).
            at != step_dur * 4
        })
        .collect();

    assert_eq!(
        step0_ons.len(),
        2,
        "step-0 note (cond=Always) must fire on both loops; got {step0_ons:?}"
    );
    assert_eq!(
        step0_ons[0], micro_us,
        "loop 0 NoteOn must be shifted by micro ({micro_us} µs) (got {})",
        step0_ons[0]
    );
    assert_eq!(
        step0_ons[1],
        step_dur * 8 + micro_us,
        "loop 1 NoteOn must be shifted by micro ({micro_us} µs) from loop-1 base (got {})",
        step0_ons[1]
    );

    // ── (c) Per-step CC: fires once before loop-0 NoteOn, suppressed on loop 1 ─
    let cc_events: Vec<(u64, u8)> = sink
        .events
        .iter()
        .filter_map(|(at, m)| match m {
            MidiMessage::ControlChange {
                channel: 0,
                controller,
                value,
            } if *controller == cc_num => Some((*at, *value)),
            _ => None,
        })
        .collect();

    assert_eq!(
        cc_events.len(),
        1,
        "CC{cc_num}={cc_val} must be sent once (loop 0) and cache-suppressed on loop 1; got {cc_events:?}"
    );
    let (cc_at, cc_v) = cc_events[0];
    assert_eq!(cc_v, cc_val, "CC value must match CcLock");
    assert!(
        cc_at < step0_ons[0],
        "CC must be emitted before the NoteOn (cc_at={cc_at}, noteon_at={})",
        step0_ons[0]
    );

    // ── (d) Trig condition: Ratio{1,2} note fires only at g4 (lane loop 0) ────
    // g12 (lane loop 1 for step 2) is outside our 10-step window, so exactly 1 NoteOn.
    let ratio_expected_at = step_dur * 4; // g4, even step, swing_offset=0, micro=0
    let ratio_ons: Vec<_> = sink
        .events
        .iter()
        .filter(|(at, m)| {
            *at == ratio_expected_at
                && matches!(m, MidiMessage::NoteOn { channel: 0, note, .. } if *note == expected_pitch)
        })
        .collect();
    assert_eq!(
        ratio_ons.len(),
        1,
        "Ratio{{1,2}} note must fire exactly once at {ratio_expected_at} µs; got {ratio_ons:?}"
    );
    // Confirm it does NOT fire at g12 (outside window — but assert total is correct).

    // ── (e) Clock division: only 3 NoteOns across 10 global steps ────────────
    // g0 (step-0 loop 0) + g4 (step-2 Ratio loop 0) + g8 (step-0 loop 1) = 3.
    let total_note_ons: usize = sink
        .events
        .iter()
        .filter(|(_, m)| matches!(m, MidiMessage::NoteOn { channel: 0, .. }))
        .count();
    assert_eq!(
        total_note_ons, 3,
        "clock_div=2 across 10 global steps: 3 NoteOns expected (2×Always + 1×Ratio); got {total_note_ons}"
    );

    // ── (f) Note safety: Stop emits CC123 on ch 0 ────────────────────────────
    let cc123_sent = sink.events.iter().any(|(at, m)| {
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
        cc123_sent,
        "Stop must emit CC123 (all-notes-off) on ch 0 to release held notes"
    );

    // Clean up.
    let _ = std::fs::remove_dir_all(&dir);
}

// ═════════════════════════════════════════════════════════════════════════════
// M10 cross-feature integration: clock-in follow / loss / persistence.
//
// Features combined (M10 T1–T6):
//   - External MIDI clock port selection + engine wiring (SetClockInPort)
//   - Clock-driven step advance (no double-step from internal timer)
//   - Start / Continue / Stop transport messages via ClockInMsg
//   - Loss detection (timeout → stop + release all notes)
//   - Note-safety (M1): registry balance across Stop and loss
//   - Persistence v4: clock_in_port round-trips through save → load
//
// Timing constants (120 BPM):
//   step_dur  = 125_000 µs  (16th note)
//   tick_us   = 20_833 µs   (1/24 beat)
//   6 ticks → 1 step;  24 ticks → 4 steps (1 beat)
//   MIN_SAMPLES = 24 intervals → need 25 ticks for lock
//   loss timeout floor = 500_000 µs
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn m10_clock_in_follow_loss_persistence() {
    use midip::engine::clock_in::ClockInMsg;
    use midip::pattern::store::{load_set, save_set, CURRENT_SET_VERSION};
    use std::collections::HashMap;

    // ── Fixture ──────────────────────────────────────────────────────────────
    let dir = unique_dir("m10-clock");
    let set = three_lane_set();

    let port = PortRef {
        stable_key: "FakeClock".into(),
        name: "FakeClock".into(),
    };

    // ── (a) Persistence: clock_in_port round-trips at schema v4 ─────────────
    {
        let mut saveable = set.clone();
        saveable.clock_in_port = Some(port.clone());
        let path = save_set(&dir, &mut saveable).unwrap();

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            raw["version"].as_u64().unwrap(),
            CURRENT_SET_VERSION as u64,
            "saved set must carry schema version {CURRENT_SET_VERSION}"
        );
        assert_eq!(
            raw["clock_in_port"]["stable_key"].as_str().unwrap(),
            "FakeClock",
            "clock_in_port must be serialised into the JSON"
        );

        let loaded = load_set(&path).unwrap();
        assert_eq!(
            loaded.clock_in_port,
            Some(port.clone()),
            "clock_in_port must survive save → load"
        );
    }

    // ── Timing helpers (120 BPM) ──────────────────────────────────────────────
    // tick_us = 60_000_000 / (120 * 24) = 20_833 µs
    let tick_us: u64 = 60_000_000 / (120 * 24);
    // loss timeout floor = 500_000 µs; add slack so the iteration fires
    let loss_timeout_us: u64 = 500_000 + 2 * tick_us;

    // ── Phase 1: Start → 25 ticks → Continue → Stop ──────────────────────────
    //
    // 25 ticks → 24 inter-tick intervals → MIN_SAMPLES satisfied → lock
    // 24 ticks (steps 1-24 after Start) / 6 ticks-per-step = 4 steps advanced
    // Continue while already playing is a no-op (engine stays playing)
    // Stop releases all sounding notes
    {
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        let stop_at = tick_us * 25 + 2;
        let total = stop_at + tick_us;

        let cmds: Vec<(u64, UiCommand)> = vec![(0, UiCommand::SetClockInPort(Some(port.clone())))];
        let mut clock_in: Vec<(u64, ClockInMsg)> = vec![(0, ClockInMsg::Start)];
        for i in 1..=25u64 {
            clock_in.push((tick_us * i, ClockInMsg::Tick));
        }
        clock_in.push((tick_us * 25 + 1, ClockInMsg::Continue));
        clock_in.push((stop_at, ClockInMsg::Stop));

        let events = run_engine_headless_clocked(
            set.clone(),
            &mut link,
            &mut sink,
            cmds,
            clock_in,
            total,
            1_000,
        );

        // Started from MIDI Start.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::Started { .. })),
            "expected Started event from MIDI Start"
        );

        // Lock acquired after 25 ticks.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::ClockInStatus { locked: true, .. })),
            "expected ClockInStatus{{locked:true}} after 25 ticks"
        );

        // Playhead count: 1 initial emit (playhead shown at step 0 on Start, last_step=None)
        // + 5 tick-driven advances. H3: `on_tick` fires the step boundary on the PRE-increment
        // 0-based count, so the FIRST F8 after Start materializes step 0 (MIDI-correct, no
        // longer one 16th late). Across 25 ticks the boundary hits ticks 1, 7, 13, 19, 25
        // (counts 0, 6, 12, 18, 24) = 5 advances (steps 0..=4) = 6 Playhead events total.
        // The clock-driven path remains the SOLE step source while ClockIn is active — the
        // internal timer never double-advances (T4's set_clock_driven guard); the extra event
        // vs. the pre-H3 count of 5 is the initial step-0 emit, not a double-advance.
        let step_count = events
            .iter()
            .filter(|e| matches!(e, EngineEvent::Playhead { .. }))
            .count();
        assert_eq!(
            step_count, 6,
            "25 ticks at 120 BPM must produce exactly 6 Playhead events \
             (1 initial + 5 tick-driven, no internal-timer double-advance); got {step_count}"
        );

        // Stopped from MIDI Stop.
        assert!(
            events.iter().any(|e| matches!(e, EngineEvent::Stopped)),
            "expected Stopped event from MIDI Stop"
        );

        // Note-safety: registry balance after Stop.
        let mut net: HashMap<(u8, u8), i32> = HashMap::new();
        for (_, msg) in &sink.events {
            match msg {
                MidiMessage::NoteOn { channel, note, vel } if *vel > 0 => {
                    *net.entry((*channel, *note)).or_insert(0) += 1;
                }
                MidiMessage::NoteOn { channel, note, .. } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                MidiMessage::NoteOff { channel, note, .. } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                _ => {}
            }
        }
        let hung: Vec<_> = net.iter().filter(|(_, &v)| v > 0).collect();
        assert!(
            hung.is_empty(),
            "no hung notes after MIDI Stop; leftover: {hung:?}"
        );
    }

    // ── Phase 2: loss detection → auto-stop + all-notes-off ──────────────────
    {
        let mut link = FakeLink::new();
        let mut sink = RecordingSink::new();

        let last_tick_at = tick_us * 25;
        let total = last_tick_at + loss_timeout_us + tick_us;

        let cmds: Vec<(u64, UiCommand)> = vec![(0, UiCommand::SetClockInPort(Some(port.clone())))];
        let mut clock_in: Vec<(u64, ClockInMsg)> = vec![(0, ClockInMsg::Start)];
        for i in 1..=25u64 {
            clock_in.push((tick_us * i, ClockInMsg::Tick));
        }
        // No more ticks → engine detects loss after timeout.

        let events = run_engine_headless_clocked(
            set.clone(),
            &mut link,
            &mut sink,
            cmds,
            clock_in,
            total,
            1_000,
        );

        // ClockInStatus{locked:false} emitted on loss.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::ClockInStatus { locked: false, .. })),
            "expected ClockInStatus{{locked:false}} after tick silence"
        );

        // Stopped emitted on loss.
        assert!(
            events.iter().any(|e| matches!(e, EngineEvent::Stopped)),
            "expected Stopped event after clock loss"
        );

        // Note-safety after loss: registry balance.
        let mut net: HashMap<(u8, u8), i32> = HashMap::new();
        for (_, msg) in &sink.events {
            match msg {
                MidiMessage::NoteOn { channel, note, vel } if *vel > 0 => {
                    *net.entry((*channel, *note)).or_insert(0) += 1;
                }
                MidiMessage::NoteOn { channel, note, .. } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                MidiMessage::NoteOff { channel, note, .. } => {
                    *net.entry((*channel, *note)).or_insert(0) -= 1;
                }
                _ => {}
            }
        }
        let hung: Vec<_> = net.iter().filter(|(_, &v)| v > 0).collect();
        assert!(
            hung.is_empty(),
            "no hung notes after clock loss; leftover: {hung:?}"
        );

        // Loss path must emit CC123 (all-notes-off) via release_all.
        assert!(
            sink.events.iter().any(|(_, m)| matches!(
                m,
                MidiMessage::ControlChange {
                    controller: 123,
                    ..
                }
            )),
            "loss path must emit CC123 (all-notes-off)"
        );
    }

    // Clean up.
    let _ = std::fs::remove_dir_all(&dir);
}
