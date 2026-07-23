//! The typed GUI→engine bridge.
//!
//! `GuiCommand` is the serializable surface the frontend sends. It is translated
//! to one or more existing `midip::app::Action`s by [`gui_to_actions`] (a pure,
//! unit-tested function). Cell-targeted commands additionally carry a
//! `(lane,row,col)` reported by [`target_cell`]; the dispatcher positions the
//! cursor (clamped) before applying the edit `Action`, so all editing reuses the
//! engine's own `App::apply` logic — nothing is reimplemented here.

use midip::app::{Action, GenField};
use midip::pattern::generate::GenMode;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "args", rename_all = "camelCase")]
pub enum GuiCommand {
    // --- transport ---
    TogglePlay,
    SetBpm(f64),
    AdjustBpm(i32),
    Tap,
    AdjustSwing(i32),
    ToggleLink,
    Panic,
    ToggleMirror,

    // --- focus ---
    FocusLane(usize),

    // --- lane state (operate on the given lane) ---
    ToggleMute(usize),
    ToggleSolo(usize),
    CancelQueue(usize),
    ToggleVoiceMute {
        lane: usize,
        row: usize,
    },

    // --- pattern-level ---
    AdjustPatternLen {
        lane: usize,
        delta: i32,
    },
    ClearPattern(usize),
    DoubleLength(usize),

    // --- step editing (cell-targeted: lane,row,col) ---
    /// Select a cell without editing it (positions the cursor / inspector).
    SelectStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    ToggleStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    ClearStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    SetVelBucket {
        lane: usize,
        row: usize,
        col: usize,
        bucket: u8,
    },
    AdjustVel {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },
    AdjustProb {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },
    AdjustRatchet {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },
    AdjustMicro {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },
    CycleCond {
        lane: usize,
        row: usize,
        col: usize,
    },
    AdjustLen {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },
    ToggleSlide {
        lane: usize,
        row: usize,
        col: usize,
    },
    NoteUp {
        lane: usize,
        col: usize,
    },
    NoteDown {
        lane: usize,
        col: usize,
    },
    CopyStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    CutStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    PasteStep {
        lane: usize,
        row: usize,
        col: usize,
    },
    CcAdd {
        lane: usize,
        row: usize,
        col: usize,
    },
    CcRemove {
        lane: usize,
        row: usize,
        col: usize,
    },
    AdjustCcVal {
        lane: usize,
        row: usize,
        col: usize,
        delta: i32,
    },

    // --- routing (per lane) ---
    CycleRoutePort {
        lane: usize,
        delta: i32,
    },
    AdjustRouteChannel {
        lane: usize,
        delta: i32,
    },
    ToggleClockOut(usize),

    // --- per-lane params ---
    AdjustLaneSwing {
        lane: usize,
        delta: i32,
    },
    ClearLaneSwing(usize),
    CycleClockDiv(usize),

    // --- pattern transforms ---
    /// Euclidean fill for a drum voice (cell-targeted at the voice row).
    Euclid {
        lane: usize,
        row: usize,
        dp: i32,
        dr: i32,
    },
    RotateRight(usize),
    RotateLeft(usize),
    ConformToScale(usize),
    ToggleFill(usize),
    CommitTransform(usize),

    // --- clock-in (external MIDI clock): 0 = clear, 1..=n = input-port index+1 ---
    SetClockIn(usize),

    // --- per-lane melodic params ---
    CycleScale {
        lane: usize,
        delta: i32,
    },
    AdjustRoot {
        lane: usize,
        delta: i32,
    },
    AdjustOctave {
        lane: usize,
        delta: i32,
    },

    // --- song (scenes + chains) ---
    RecallScene(usize),
    CaptureScene,
    PlayChain(usize),
    StopChain,

    // --- generative tool (operates on the focused lane; live preview) ---
    OpenGenerative,
    GenSetMode(String),
    GenAdjust {
        field: String,
        delta: i32,
    },
    GenReroll,
    GenCommit,
    GenCancel,

    // --- history ---
    Undo,
    Redo,

    // --- persistence (reuse the engine's own Save/Load actions) ---
    Save,
    SaveSetAs(String),
    NewSet,
    LoadSet(String),
    RenameSet(String),
    DuplicateSet,
    DeleteSet(String),
    LoadUserPattern(String),
    SaveLanePattern(String),
}

/// Cell-targeted commands report the `(lane,row,col)` the dispatcher must move
/// the cursor to (clamped) before applying. Lane-only and global commands return
/// `None` — their focus (if any) is folded into [`gui_to_actions`].
pub fn target_cell(cmd: &GuiCommand) -> Option<(usize, usize, usize)> {
    use GuiCommand::*;
    match *cmd {
        SelectStep { lane, row, col }
        | ToggleStep { lane, row, col }
        | ClearStep { lane, row, col }
        | SetVelBucket { lane, row, col, .. }
        | AdjustVel { lane, row, col, .. }
        | AdjustProb { lane, row, col, .. }
        | AdjustRatchet { lane, row, col, .. }
        | AdjustMicro { lane, row, col, .. }
        | CycleCond { lane, row, col }
        | AdjustLen { lane, row, col, .. }
        | ToggleSlide { lane, row, col }
        | CopyStep { lane, row, col }
        | CutStep { lane, row, col }
        | PasteStep { lane, row, col }
        | CcAdd { lane, row, col }
        | CcRemove { lane, row, col }
        | AdjustCcVal { lane, row, col, .. } => Some((lane, row, col)),
        // Euclid targets the drum voice row (column irrelevant).
        Euclid { lane, row, .. } => Some((lane, row, 0)),
        // Note pitch edits are melodic (single row); target row 0.
        NoteUp { lane, col } | NoteDown { lane, col } => Some((lane, 0, col)),
        // Per-voice mute uses the drum row as the voice selector.
        ToggleVoiceMute { lane, row } => Some((lane, row, 0)),
        _ => None,
    }
}

/// The primary lane a command targets, if any. The dispatcher bounds-checks
/// this against the live lane count and drops out-of-range commands (rather than
/// letting a bad index reach the engine). Global/transport commands return `None`.
pub fn command_lane(cmd: &GuiCommand) -> Option<usize> {
    use GuiCommand::*;
    if let Some((lane, _, _)) = target_cell(cmd) {
        return Some(lane);
    }
    match *cmd {
        FocusLane(l) | ToggleMute(l) | ToggleSolo(l) | CancelQueue(l) | ClearPattern(l)
        | DoubleLength(l) | ToggleClockOut(l) => Some(l),
        ClearLaneSwing(l) | CycleClockDiv(l) => Some(l),
        RotateRight(l) | RotateLeft(l) | ConformToScale(l) | ToggleFill(l) | CommitTransform(l) => {
            Some(l)
        }
        AdjustPatternLen { lane, .. }
        | CycleScale { lane, .. }
        | AdjustRoot { lane, .. }
        | AdjustOctave { lane, .. }
        | CycleRoutePort { lane, .. }
        | AdjustRouteChannel { lane, .. }
        | AdjustLaneSwing { lane, .. } => Some(lane),
        _ => None,
    }
}

/// Clock-in selection reads `App::clock_in_sel` (against a refreshed
/// `clock_in_ports`); the dispatcher primes both before applying `ClockInConfirm`.
/// Returns the target selection index (0 = clear, 1..=n = port).
pub fn clockin_prep(cmd: &GuiCommand) -> Option<usize> {
    match *cmd {
        GuiCommand::SetClockIn(idx) => Some(idx),
        _ => None,
    }
}

/// Routing commands drive the engine's route-editor actions, which read
/// `App::route_editor_lane` (and `route_editor_ports` for port cycling). The
/// dispatcher must set those up first. Returns `(lane, needs_port_list)`.
pub fn route_prep(cmd: &GuiCommand) -> Option<(usize, bool)> {
    use GuiCommand::*;
    match *cmd {
        CycleRoutePort { lane, .. } => Some((lane, true)),
        AdjustRouteChannel { lane, .. } => Some((lane, false)),
        ToggleClockOut(lane) => Some((lane, false)),
        _ => None,
    }
}

/// Pure translation of a `GuiCommand` into the engine's `Action` vocabulary.
///
/// For cell-targeted commands the cursor is positioned by the dispatcher first,
/// so only the edit `Action` is returned here. For lane-scoped state commands
/// the returned sequence begins with `FocusLane` so the (focus-relative) engine
/// action lands on the intended lane.
pub fn gui_to_actions(cmd: &GuiCommand) -> Vec<Action> {
    use GuiCommand as G;
    match *cmd {
        // transport / global
        G::TogglePlay => vec![Action::TogglePlay],
        G::SetBpm(b) => vec![Action::SetBpm(b)],
        G::AdjustBpm(d) => vec![Action::AdjustBpm(d)],
        G::Tap => vec![Action::Tap],
        G::AdjustSwing(d) => vec![Action::AdjustSwing(clamp_i8(d))],
        G::ToggleLink => vec![Action::ToggleLink],
        G::Panic => vec![Action::Panic],
        G::ToggleMirror => vec![Action::ToggleMirror],
        G::Undo => vec![Action::Undo],
        G::Redo => vec![Action::Redo],

        // song — App guards scene/chain indices internally
        G::RecallScene(i) => vec![Action::RecallScene(i)],
        G::CaptureScene => vec![Action::CaptureScene],
        G::PlayChain(i) => vec![Action::PlayChain(i)],
        G::StopChain => vec![Action::StopChain],

        // generative — all no-ops unless the preview is active (App guards on
        // temp_transform); unknown mode/field strings translate to nothing.
        G::OpenGenerative => vec![Action::OpenGenerative],
        G::GenSetMode(ref m) => match gen_mode(m) {
            Some(mode) => vec![Action::GenSetMode(mode)],
            None => vec![],
        },
        G::GenAdjust { ref field, delta } => match gen_field(field) {
            Some(f) => vec![Action::GenAdjust { field: f, delta }],
            None => vec![],
        },
        G::GenReroll => vec![Action::GenReroll],
        G::GenCommit => vec![Action::GenCommit],
        G::GenCancel => vec![Action::GenCancel],

        // persistence — the engine's own actions self-resolve the data dir,
        // mark dirty, set status and manage recovery, so we simply forward them.
        G::Save => vec![Action::Save],
        G::SaveSetAs(ref name) => vec![Action::SaveSetAs(name.clone())],
        G::NewSet => vec![Action::NewSet],
        G::LoadSet(ref path) => vec![Action::DoLoadSet(std::path::PathBuf::from(path))],
        G::RenameSet(ref name) => vec![Action::RenameSet(name.clone())],
        G::DuplicateSet => vec![Action::DuplicateSet],
        G::DeleteSet(ref path) => vec![Action::DeleteSet(std::path::PathBuf::from(path))],
        G::LoadUserPattern(ref path) => {
            vec![Action::LoadUserPattern(std::path::PathBuf::from(path))]
        }
        G::SaveLanePattern(ref name) => vec![Action::SaveAsUserPattern(name.clone())],

        G::FocusLane(l) => vec![Action::FocusLane(l)],

        // lane state — focus then act
        G::ToggleMute(l) => vec![Action::FocusLane(l), Action::ToggleMute],
        G::ToggleSolo(l) => vec![Action::FocusLane(l), Action::ToggleSolo],
        G::CancelQueue(l) => vec![Action::FocusLane(l), Action::CancelQueue],
        G::ToggleVoiceMute { lane, .. } => {
            vec![Action::FocusLane(lane), Action::ToggleVoiceMute]
        }

        // pattern-level — focus then act
        G::AdjustPatternLen { lane, delta } => {
            vec![
                Action::FocusLane(lane),
                Action::AdjustPatternLen(clamp_i8(delta)),
            ]
        }
        G::ClearPattern(l) => vec![Action::FocusLane(l), Action::ClearPattern],
        G::DoubleLength(l) => vec![Action::FocusLane(l), Action::DoubleLength],

        // routing — route_editor_lane/ports are set up by the dispatcher
        G::CycleRoutePort { delta, .. } => vec![Action::RouteCyclePort(delta)],
        G::AdjustRouteChannel { delta, .. } => vec![Action::RouteAdjustChannel(delta)],
        G::ToggleClockOut(_) => vec![Action::RouteToggleClockOut],

        // per-lane params — focus then act
        G::AdjustLaneSwing { lane, delta } => {
            vec![
                Action::FocusLane(lane),
                Action::AdjustLaneSwing(clamp_i8(delta)),
            ]
        }
        G::ClearLaneSwing(l) => vec![Action::FocusLane(l), Action::ClearLaneSwing],
        G::CycleClockDiv(l) => vec![Action::FocusLane(l), Action::CycleClockDiv],

        // transforms — cursor/focus positioned by the dispatcher
        G::Euclid { dp, dr, .. } => vec![Action::Euclid {
            dp: clamp_i8(dp),
            dr: clamp_i8(dr),
        }],
        G::RotateRight(l) => vec![Action::FocusLane(l), Action::RotateRight],
        G::RotateLeft(l) => vec![Action::FocusLane(l), Action::RotateLeft],
        G::ConformToScale(l) => vec![Action::FocusLane(l), Action::ConformToScale],
        G::ToggleFill(l) => vec![Action::FocusLane(l), Action::ToggleFill],
        G::CommitTransform(l) => vec![Action::FocusLane(l), Action::CommitTransform],

        // clock-in — selection primed by the dispatcher, then confirmed
        G::SetClockIn(_) => vec![Action::ClockInConfirm],

        // per-lane melodic params — focus then act
        G::CycleScale { lane, delta } => {
            vec![Action::FocusLane(lane), Action::CycleScale(clamp_i8(delta))]
        }
        G::AdjustRoot { lane, delta } => {
            vec![Action::FocusLane(lane), Action::AdjustRoot(clamp_i8(delta))]
        }
        G::AdjustOctave { lane, delta } => {
            vec![
                Action::FocusLane(lane),
                Action::AdjustOctave(clamp_i8(delta)),
            ]
        }

        // cell edits — cursor already positioned by dispatcher
        // Select-only: the cursor move happened in the dispatcher; no edit action.
        G::SelectStep { .. } => vec![],
        G::ToggleStep { .. } => vec![Action::ToggleStep],
        G::ClearStep { .. } => vec![Action::ClearStep],
        G::SetVelBucket { bucket, .. } => vec![Action::SetVelBucket(bucket)],
        G::AdjustVel { delta, .. } => vec![Action::AdjustVel(clamp_i8(delta))],
        G::AdjustProb { delta, .. } => vec![Action::AdjustProb(clamp_i8(delta))],
        G::AdjustRatchet { delta, .. } => vec![Action::AdjustRatchet(clamp_i8(delta))],
        G::AdjustMicro { delta, .. } => vec![Action::AdjustMicro(clamp_i8(delta))],
        G::CycleCond { .. } => vec![Action::CycleCond],
        G::AdjustLen { delta, .. } => vec![Action::AdjustLen(clamp_i8(delta))],
        G::ToggleSlide { .. } => vec![Action::ToggleSlide],
        G::NoteUp { .. } => vec![Action::NoteUp],
        G::NoteDown { .. } => vec![Action::NoteDown],
        G::CopyStep { .. } => vec![Action::CopyStep],
        G::CutStep { .. } => vec![Action::CutStep],
        G::PasteStep { .. } => vec![Action::PasteStep],
        G::CcAdd { .. } => vec![Action::CcAdd],
        G::CcRemove { .. } => vec![Action::CcRemove],
        G::AdjustCcVal { delta, .. } => vec![Action::AdjustCcVal(clamp_i8(delta))],
    }
}

/// GUI deltas arrive as `i32`; engine adjust-actions take `i8`. Saturate.
fn clamp_i8(v: i32) -> i8 {
    v.clamp(i8::MIN as i32, i8::MAX as i32) as i8
}

fn gen_mode(s: &str) -> Option<GenMode> {
    match s {
        "generate" => Some(GenMode::Generate),
        "vary" => Some(GenMode::Vary),
        "arp" => Some(GenMode::Arp),
        _ => None,
    }
}

fn gen_field(s: &str) -> Option<GenField> {
    match s {
        "density" => Some(GenField::Density),
        "range" => Some(GenField::Range),
        "mutate" => Some(GenField::Mutate),
        "chord" => Some(GenField::Chord),
        "octaves" => Some(GenField::Octaves),
        "shape" => Some(GenField::Shape),
        "gate" => Some(GenField::Gate),
        "velvar" => Some(GenField::VelVar),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_translation() {
        assert_eq!(
            gui_to_actions(&GuiCommand::TogglePlay),
            vec![Action::TogglePlay]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::SetBpm(128.0)),
            vec![Action::SetBpm(128.0)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::AdjustBpm(-3)),
            vec![Action::AdjustBpm(-3)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::ToggleLink),
            vec![Action::ToggleLink]
        );
        assert_eq!(gui_to_actions(&GuiCommand::Panic), vec![Action::Panic]);
    }

    #[test]
    fn lane_state_focuses_first() {
        assert_eq!(
            gui_to_actions(&GuiCommand::ToggleMute(2)),
            vec![Action::FocusLane(2), Action::ToggleMute]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::ToggleSolo(0)),
            vec![Action::FocusLane(0), Action::ToggleSolo]
        );
    }

    #[test]
    fn cell_edit_returns_bare_action() {
        // Focus+cursor are handled by the dispatcher via target_cell, so the
        // translation is just the edit action.
        assert_eq!(
            gui_to_actions(&GuiCommand::ToggleStep {
                lane: 1,
                row: 3,
                col: 7
            }),
            vec![Action::ToggleStep]
        );
        assert_eq!(
            target_cell(&GuiCommand::ToggleStep {
                lane: 1,
                row: 3,
                col: 7
            }),
            Some((1, 3, 7))
        );
    }

    #[test]
    fn delta_saturates_to_i8() {
        assert_eq!(
            gui_to_actions(&GuiCommand::AdjustBpm(9999)),
            vec![Action::AdjustBpm(9999)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::AdjustVel {
                lane: 0,
                row: 0,
                col: 0,
                delta: 9999
            }),
            vec![Action::AdjustVel(127)]
        );
    }

    #[test]
    fn routing_translation_and_prep() {
        assert_eq!(
            gui_to_actions(&GuiCommand::CycleRoutePort { lane: 1, delta: 1 }),
            vec![Action::RouteCyclePort(1)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::AdjustRouteChannel { lane: 2, delta: -1 }),
            vec![Action::RouteAdjustChannel(-1)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::ToggleClockOut(0)),
            vec![Action::RouteToggleClockOut]
        );
        // Port cycling needs the port list refreshed; channel/clock-out don't.
        assert_eq!(
            route_prep(&GuiCommand::CycleRoutePort { lane: 1, delta: 1 }),
            Some((1, true))
        );
        assert_eq!(
            route_prep(&GuiCommand::AdjustRouteChannel { lane: 2, delta: 1 }),
            Some((2, false))
        );
        assert_eq!(route_prep(&GuiCommand::ToggleClockOut(0)), Some((0, false)));
        assert_eq!(route_prep(&GuiCommand::TogglePlay), None);
        // Routing commands are lane-bounds-checked.
        assert_eq!(command_lane(&GuiCommand::ToggleClockOut(2)), Some(2));
    }

    #[test]
    fn song_translation() {
        assert_eq!(
            gui_to_actions(&GuiCommand::RecallScene(2)),
            vec![Action::RecallScene(2)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::CaptureScene),
            vec![Action::CaptureScene]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::PlayChain(1)),
            vec![Action::PlayChain(1)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::StopChain),
            vec![Action::StopChain]
        );
        // Song commands are global (no lane / no cell targeting).
        assert_eq!(command_lane(&GuiCommand::PlayChain(1)), None);
        assert_eq!(target_cell(&GuiCommand::RecallScene(0)), None);
    }

    #[test]
    fn generative_translation() {
        assert_eq!(
            gui_to_actions(&GuiCommand::OpenGenerative),
            vec![Action::OpenGenerative]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::GenReroll),
            vec![Action::GenReroll]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::GenCommit),
            vec![Action::GenCommit]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::GenSetMode("arp".into())),
            vec![Action::GenSetMode(GenMode::Arp)]
        );
        assert_eq!(
            gui_to_actions(&GuiCommand::GenAdjust {
                field: "density".into(),
                delta: 5
            }),
            vec![Action::GenAdjust {
                field: GenField::Density,
                delta: 5
            }]
        );
        // Unknown mode/field strings translate to a no-op (not a panic/bad action).
        assert!(gui_to_actions(&GuiCommand::GenSetMode("bogus".into())).is_empty());
        assert!(gui_to_actions(&GuiCommand::GenAdjust {
            field: "bogus".into(),
            delta: 1
        })
        .is_empty());
    }

    #[test]
    fn global_commands_have_no_target_cell() {
        assert_eq!(target_cell(&GuiCommand::TogglePlay), None);
        assert_eq!(target_cell(&GuiCommand::ToggleMute(1)), None);
    }
}
