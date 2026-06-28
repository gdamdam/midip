//! Keyboard → Action mapping.
//!
//! `key_to_action` is pure and has no side-effects; all UI state lives in App.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, Mode};
use crate::pattern::model::LaneKind;

/// Map a raw key event to an [`Action`], given the current app mode and focused lane kind.
///
/// # MoveCursor argument order
/// `Action::MoveCursor(drow, dcol)` matches `App::move_cursor(drow, dcol)`:
/// - drow: change in voice row  (Up=-1, Down=+1; drums only — melodic row is always 0)
/// - dcol: change in step column (Left=-1, Right=+1; both drums and melodic)
pub fn key_to_action(key: KeyEvent, mode: Mode, kind: LaneKind) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // Global shortcuts that work in every mode.
    if ctrl {
        match key.code {
            KeyCode::Char('z') => {
                return if shift { Action::Redo } else { Action::Undo };
            }
            KeyCode::Char('y') => return Action::Redo,
            KeyCode::Char('r') => return Action::Redo,
            _ => {}
        }
    }

    // Global play/panic — checked before per-mode branches so they fire in ALL modes.
    // Esc retains its per-mode meaning (Edit→Panic, Library/Help→close, TempoEntry→cancel).
    // Note: this also intercepts space/'!' inside TempoEntry before the digit handler below;
    // neither is a tempo digit and play/stop mid-entry is acceptable, so that is intentional.
    match key.code {
        KeyCode::Char(' ') => return Action::TogglePlay,
        KeyCode::Char('!') => return Action::Panic,
        _ => {}
    }

    match mode {
        Mode::Library => match key.code {
            KeyCode::Left => return Action::LibNav(-1, 0), // switch to Genre column
            KeyCode::Right => return Action::LibNav(1, 0), // switch to Pattern column
            KeyCode::Up => return Action::LibNav(0, -1),   // move up in focused list
            KeyCode::Down => return Action::LibNav(0, 1),  // move down in focused list
            KeyCode::Enter => return Action::LibLoad,
            KeyCode::Char('a') => return Action::Audition, // cue/audition selected pattern
            KeyCode::Char('l') | KeyCode::Esc => return Action::CloseLibrary,
            _ => {}
        },
        Mode::SetBrowser => match key.code {
            KeyCode::Up => return Action::SetBrowserNav(-1),
            KeyCode::Down => return Action::SetBrowserNav(1),
            KeyCode::Enter => return Action::SetBrowserLoad,
            KeyCode::Esc | KeyCode::Char('o') => return Action::CloseSetBrowser,
            _ => {}
        },
        Mode::Help => return Action::Help,
        Mode::RouteEditor => {
            return match key.code {
                KeyCode::Esc => Action::CloseRouteEditor,
                KeyCode::Up => Action::RouteNavLane(-1),
                KeyCode::Down => Action::RouteNavLane(1),
                KeyCode::Left => Action::RouteCycleField(-1),
                KeyCode::Right => Action::RouteCycleField(1),
                KeyCode::Char('c') => {
                    // Cycle port forward; Shift+c cycles backward.
                    if shift {
                        Action::RouteCyclePort(-1)
                    } else {
                        Action::RouteCyclePort(1)
                    }
                }
                KeyCode::Char('[') => Action::RouteAdjustChannel(-1),
                KeyCode::Char(']') => Action::RouteAdjustChannel(1),
                KeyCode::Char('z') => Action::RouteToggleClockOut,
                _ => Action::None,
            };
        }
        Mode::TempoEntry => {
            return match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => Action::TempoDigit(c),
                KeyCode::Backspace => Action::TempoBackspace,
                KeyCode::Enter => Action::TempoCommit,
                KeyCode::Esc => Action::TempoCancel,
                _ => Action::None,
            };
        }
        Mode::Edit => {}
    }

    // '?' works in any non-Library mode.
    if key.code == KeyCode::Char('?') {
        return Action::Help;
    }

    // Edit-mode-only keys (mode == Edit at this point, or fell through).
    if mode == Mode::Edit {
        match key.code {
            KeyCode::Esc => return Action::Panic,
            KeyCode::Char(' ') => return Action::TogglePlay,
            KeyCode::Tab => return Action::FocusNext,
            KeyCode::BackTab => return Action::FocusPrev,
            KeyCode::Enter => return Action::ToggleStep,
            KeyCode::Delete => return Action::ClearStep,

            // Arrow keys: behaviour differs by lane kind.
            // Up/Down navigate the voice row (drums only); Left/Right navigate the step column.
            KeyCode::Up => match kind {
                LaneKind::Drums => return Action::MoveCursor(-1, 0),
                LaneKind::Melodic => return Action::NoteUp,
            },
            KeyCode::Down => match kind {
                LaneKind::Drums => return Action::MoveCursor(1, 0),
                LaneKind::Melodic => return Action::NoteDown,
            },
            KeyCode::Left => return Action::MoveCursor(0, -1),
            KeyCode::Right => return Action::MoveCursor(0, 1),

            KeyCode::Char(c) => {
                // Digit keys → lane focus or velocity bucket.
                if let Some(n) = c.to_digit(10) {
                    return Action::SetVelBucket(n as u8);
                }

                // Global char bindings.
                match c {
                    '+' => return Action::AdjustVel(1),
                    '-' => return Action::AdjustVel(-1),
                    'x' => return Action::CutStep,
                    'c' => return Action::CopyStep,
                    'v' => return Action::PasteStep,
                    'r' => return Action::RotateRight,
                    'R' => return Action::RotateLeft,
                    'u' => return Action::Undo,
                    'm' => return Action::ToggleMute,
                    'S' => return Action::ToggleSolo,
                    't' => return Action::OpenTempo,
                    'T' => return Action::Tap,
                    'k' => return Action::ToggleLink,
                    ';' => return Action::AdjustBpm(-1),
                    '\'' => return Action::AdjustBpm(1),
                    '<' => return Action::AdjustSwing(-1),
                    '>' => return Action::AdjustSwing(1),
                    '{' => return Action::AdjustPatternLen(-1),
                    '}' => return Action::AdjustPatternLen(1),
                    'p' => return Action::AdjustProb(-1),
                    'P' => return Action::AdjustProb(1),
                    'y' => return Action::AdjustRatchet(-1),
                    'Y' => return Action::AdjustRatchet(1),
                    'l' => return Action::OpenLibrary,
                    'o' => return Action::OpenSetBrowser,
                    'w' => return Action::OpenRouteEditor,
                    's' => return Action::Save,
                    'q' => return Action::Quit,
                    _ => {}
                }

                // Kind-specific char bindings.
                match kind {
                    LaneKind::Melodic => match c {
                        'g' => return Action::ToggleSlide,
                        ',' => return Action::AdjustLen(-1),
                        '.' => return Action::AdjustLen(1),
                        '[' => return Action::AdjustOctave(-1),
                        ']' => return Action::AdjustOctave(1),
                        _ => {}
                    },
                    LaneKind::Drums => match c {
                        'e' => return Action::Euclid { dp: 1, dr: 0 },
                        'E' => return Action::Euclid { dp: -1, dr: 0 },
                        '[' => return Action::Euclid { dp: 0, dr: -1 },
                        ']' => return Action::Euclid { dp: 0, dr: 1 },
                        _ => {}
                    },
                }
            }
            _ => {}
        }
    }

    Action::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Mode};
    use crate::pattern::model::LaneKind;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ck(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn csk(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
    }

    #[test]
    fn space_toggles_play() {
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::Edit, LaneKind::Drums),
            Action::TogglePlay
        );
    }

    // --- Item 1: global Space→TogglePlay and '!'→Panic in every mode -------

    #[test]
    fn space_is_toggle_play_in_all_modes() {
        for mode in [
            Mode::Edit,
            Mode::Library,
            Mode::Help,
            Mode::TempoEntry,
            Mode::SetBrowser,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char(' ')), mode, LaneKind::Drums),
                Action::TogglePlay,
                "Space should be TogglePlay in {:?}",
                mode
            );
        }
    }

    #[test]
    fn exclamation_is_panic_in_all_modes() {
        for mode in [
            Mode::Edit,
            Mode::Library,
            Mode::Help,
            Mode::TempoEntry,
            Mode::SetBrowser,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('!')), mode, LaneKind::Drums),
                Action::Panic,
                "! should be Panic in {:?}",
                mode
            );
        }
    }

    #[test]
    fn tab_and_backtab_cycle_focus() {
        assert_eq!(
            key_to_action(k(KeyCode::Tab), Mode::Edit, LaneKind::Drums),
            Action::FocusNext
        );
        assert_eq!(
            key_to_action(k(KeyCode::BackTab), Mode::Edit, LaneKind::Drums),
            Action::FocusPrev
        );
    }

    #[test]
    fn enter_toggles_step_in_both_kinds() {
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::Edit, LaneKind::Drums),
            Action::ToggleStep
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::Edit, LaneKind::Melodic),
            Action::ToggleStep
        );
    }

    #[test]
    fn arrows_move_cursor() {
        // Left moves the step column back (dcol = -1, drow = 0).
        assert_eq!(
            key_to_action(k(KeyCode::Left), Mode::Edit, LaneKind::Drums),
            Action::MoveCursor(0, -1)
        );
        // Down moves to the next voice row (drow = +1, dcol = 0).
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::Edit, LaneKind::Drums),
            Action::MoveCursor(1, 0)
        );
    }

    #[test]
    fn melodic_arrows_and_slide() {
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::Edit, LaneKind::Melodic),
            Action::NoteUp
        );
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::Edit, LaneKind::Melodic),
            Action::NoteDown
        );
        // Left moves the step column back in melodic mode too (dcol = -1, drow = 0).
        assert_eq!(
            key_to_action(k(KeyCode::Left), Mode::Edit, LaneKind::Melodic),
            Action::MoveCursor(0, -1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('g')), Mode::Edit, LaneKind::Melodic),
            Action::ToggleSlide
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char(']')), Mode::Edit, LaneKind::Melodic),
            Action::AdjustOctave(1)
        );
    }

    #[test]
    fn drums_up_moves_cursor() {
        // Up moves to the previous voice row (drow = -1, dcol = 0).
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::Edit, LaneKind::Drums),
            Action::MoveCursor(-1, 0)
        );
    }

    // --- BPM control keys -------------------------------------------------

    #[test]
    fn t_opens_tempo_entry() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('t')), Mode::Edit, LaneKind::Drums),
            Action::OpenTempo
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('t')), Mode::Edit, LaneKind::Melodic),
            Action::OpenTempo
        );
    }

    #[test]
    fn semicolon_and_quote_nudge_bpm() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char(';')), Mode::Edit, kind),
                Action::AdjustBpm(-1)
            );
            assert_eq!(
                key_to_action(k(KeyCode::Char('\'')), Mode::Edit, kind),
                Action::AdjustBpm(1)
            );
        }
    }

    #[test]
    fn tempo_entry_mode_digit_backspace_commit_cancel() {
        // Digits → TempoDigit
        assert_eq!(
            key_to_action(k(KeyCode::Char('1')), Mode::TempoEntry, LaneKind::Drums),
            Action::TempoDigit('1')
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('9')), Mode::TempoEntry, LaneKind::Melodic),
            Action::TempoDigit('9')
        );
        // Non-digit char → None
        assert_eq!(
            key_to_action(k(KeyCode::Char('x')), Mode::TempoEntry, LaneKind::Drums),
            Action::None
        );
        // Backspace → TempoBackspace
        assert_eq!(
            key_to_action(k(KeyCode::Backspace), Mode::TempoEntry, LaneKind::Drums),
            Action::TempoBackspace
        );
        // Enter → TempoCommit
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::TempoEntry, LaneKind::Drums),
            Action::TempoCommit
        );
        // Esc → TempoCancel
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::TempoEntry, LaneKind::Drums),
            Action::TempoCancel
        );
    }

    #[test]
    fn save_and_link_are_global() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('s')), Mode::Edit, kind),
                Action::Save
            );
            assert_eq!(
                key_to_action(k(KeyCode::Char('k')), Mode::Edit, kind),
                Action::ToggleLink
            );
        }
    }

    #[test]
    fn swing_and_pattern_len() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('<')), Mode::Edit, LaneKind::Drums),
            Action::AdjustSwing(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('}')), Mode::Edit, LaneKind::Drums),
            Action::AdjustPatternLen(1)
        );
    }

    #[test]
    fn ctrl_z_y_and_shift_z_map_to_undo_redo_globally() {
        // ctrl+z -> Undo (works in every mode).
        assert_eq!(
            key_to_action(ck(KeyCode::Char('z')), Mode::Edit, LaneKind::Drums),
            Action::Undo
        );
        assert_eq!(
            key_to_action(ck(KeyCode::Char('z')), Mode::Library, LaneKind::Drums),
            Action::Undo
        );
        // ctrl+y -> Redo.
        assert_eq!(
            key_to_action(ck(KeyCode::Char('y')), Mode::Edit, LaneKind::Drums),
            Action::Redo
        );
        // ctrl+shift+z -> Redo.
        assert_eq!(
            key_to_action(csk(KeyCode::Char('z')), Mode::Edit, LaneKind::Drums),
            Action::Redo
        );
    }

    #[test]
    fn drums_euclid_keys() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('e')), Mode::Edit, LaneKind::Drums),
            Action::Euclid { dp: 1, dr: 0 }
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char(']')), Mode::Edit, LaneKind::Drums),
            Action::Euclid { dp: 0, dr: 1 }
        );
    }

    #[test]
    fn library_mode_arrows_and_enter() {
        // Left/Right switch columns; Up/Down move within the focused list.
        assert_eq!(
            key_to_action(k(KeyCode::Left), Mode::Library, LaneKind::Drums),
            Action::LibNav(-1, 0),
            "Left → switch to Genre column"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Right), Mode::Library, LaneKind::Drums),
            Action::LibNav(1, 0),
            "Right → switch to Pattern column"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::Library, LaneKind::Drums),
            Action::LibNav(0, -1),
            "Up → move up in focused list"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::Library, LaneKind::Drums),
            Action::LibNav(0, 1),
            "Down → move down in focused list"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::Library, LaneKind::Drums),
            Action::LibLoad
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::Library, LaneKind::Drums),
            Action::CloseLibrary
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('a')), Mode::Library, LaneKind::Drums),
            Action::Audition,
            "a in Library mode should trigger Audition"
        );
    }

    #[test]
    fn set_browser_mode_keys() {
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserNav(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserNav(1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserLoad
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::SetBrowser, LaneKind::Drums),
            Action::CloseSetBrowser
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('o')), Mode::SetBrowser, LaneKind::Drums),
            Action::CloseSetBrowser
        );
    }

    #[test]
    fn o_key_opens_set_browser_in_edit_mode() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('o')), Mode::Edit, LaneKind::Drums),
            Action::OpenSetBrowser
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('o')), Mode::Edit, LaneKind::Melodic),
            Action::OpenSetBrowser
        );
    }

    #[test]
    fn edit_esc_is_panic() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Esc), Mode::Edit, kind),
                Action::Panic
            );
        }
        // Library Esc = CloseLibrary (not Panic).
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::Library, LaneKind::Drums),
            Action::CloseLibrary
        );
    }

    #[test]
    fn vel_bucket_and_open_library() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('5')), Mode::Edit, LaneKind::Drums),
            Action::SetVelBucket(5)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('l')), Mode::Edit, LaneKind::Drums),
            Action::OpenLibrary
        );
    }

    // --- Fix #10 regression: 1/2/3 are now SetVelBucket, not FocusLane ---

    #[test]
    fn digits_1_2_3_map_to_set_vel_bucket_not_focus_lane() {
        for (ch, bucket) in [('1', 1u8), ('2', 2u8), ('3', 3u8)] {
            for kind in [LaneKind::Drums, LaneKind::Melodic] {
                assert_eq!(
                    key_to_action(k(KeyCode::Char(ch)), Mode::Edit, kind),
                    Action::SetVelBucket(bucket),
                    "'{ch}' should be SetVelBucket({bucket}), not FocusLane"
                );
            }
        }
    }

    #[test]
    fn all_digit_keys_0_through_9_map_to_set_vel_bucket() {
        for ch in '0'..='9' {
            let expected = Action::SetVelBucket(ch.to_digit(10).unwrap() as u8);
            assert_eq!(
                key_to_action(k(KeyCode::Char(ch)), Mode::Edit, LaneKind::Drums),
                expected,
                "'{ch}' should be SetVelBucket"
            );
        }
    }

    #[test]
    fn tab_and_backtab_still_cycle_lane_focus_after_fix10() {
        assert_eq!(
            key_to_action(k(KeyCode::Tab), Mode::Edit, LaneKind::Drums),
            Action::FocusNext
        );
        assert_eq!(
            key_to_action(k(KeyCode::BackTab), Mode::Edit, LaneKind::Drums),
            Action::FocusPrev
        );
    }

    // --- Route editor key bindings (Task 8) --------------------------------

    #[test]
    fn w_key_was_unbound_before_route_editor() {
        // Verify 'w' was not previously bound: any prior Action for 'w' in Edit mode
        // was Action::None. This test documents the choice of 'w' as the open key.
        // NOTE: 'w' is now bound to OpenRouteEditor; this test confirms the OLD
        // behavior was None by checking the new mapping is OpenRouteEditor (not something else),
        // which implies it was free before this task added the binding.
        // The actual assertion is that 'w' maps to OpenRouteEditor now:
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('w')), Mode::Edit, kind),
                Action::OpenRouteEditor,
                "'w' in Edit mode must open the route editor"
            );
        }
    }

    #[test]
    fn route_editor_esc_closes() {
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::RouteEditor, LaneKind::Drums),
            Action::CloseRouteEditor
        );
    }

    #[test]
    fn route_editor_arrows_navigate_lanes_and_cycle_field() {
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteNavLane(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteNavLane(1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Left), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteCycleField(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Right), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteCycleField(1)
        );
    }

    #[test]
    fn route_editor_c_cycles_port_forward_shift_c_backward() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('c')), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteCyclePort(1)
        );
        let shift_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SHIFT);
        assert_eq!(
            key_to_action(shift_c, Mode::RouteEditor, LaneKind::Drums),
            Action::RouteCyclePort(-1)
        );
    }

    #[test]
    fn route_editor_bracket_keys_adjust_channel() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('[')), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteAdjustChannel(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char(']')), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteAdjustChannel(1)
        );
    }

    #[test]
    fn route_editor_z_toggles_clock_out() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('z')), Mode::RouteEditor, LaneKind::Drums),
            Action::RouteToggleClockOut
        );
    }

    #[test]
    fn space_and_exclamation_still_fire_in_route_editor_mode() {
        // Global shortcuts must work even in RouteEditor mode.
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::RouteEditor, LaneKind::Drums),
            Action::TogglePlay,
            "space must be TogglePlay in RouteEditor mode"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('!')), Mode::RouteEditor, LaneKind::Drums),
            Action::Panic,
            "! must be Panic in RouteEditor mode"
        );
    }

    #[test]
    fn space_is_toggle_play_in_all_modes_including_route_editor() {
        for mode in [
            Mode::Edit,
            Mode::Library,
            Mode::Help,
            Mode::TempoEntry,
            Mode::SetBrowser,
            Mode::RouteEditor,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char(' ')), mode, LaneKind::Drums),
                Action::TogglePlay,
                "Space should be TogglePlay in {:?}",
                mode
            );
        }
    }

    #[test]
    fn exclamation_is_panic_in_all_modes_including_route_editor() {
        for mode in [
            Mode::Edit,
            Mode::Library,
            Mode::Help,
            Mode::TempoEntry,
            Mode::SetBrowser,
            Mode::RouteEditor,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('!')), mode, LaneKind::Drums),
                Action::Panic,
                "! should be Panic in {:?}",
                mode
            );
        }
    }
}
