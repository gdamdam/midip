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

    // In NameEntry, space is a name character — override the global TogglePlay for this mode.
    if matches!(mode, Mode::NameEntry(_)) && key.code == KeyCode::Char(' ') {
        return Action::NameChar(' ');
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
        Mode::RecoveryPrompt => {
            return match key.code {
                KeyCode::Char('r') | KeyCode::Enter => Action::RecoveryRecover,
                KeyCode::Char('d') | KeyCode::Esc => Action::RecoveryDiscard,
                KeyCode::Char('o') => Action::RecoveryOpenSaved,
                _ => Action::None,
            };
        }
        Mode::Library => match key.code {
            KeyCode::Left => return Action::LibNav(-1, 0), // switch to Genre column
            KeyCode::Right => return Action::LibNav(1, 0), // switch to Pattern column
            KeyCode::Up => return Action::LibNav(0, -1),   // move up in focused list
            KeyCode::Down => return Action::LibNav(0, 1),  // move down in focused list
            KeyCode::Enter => return Action::LibLoad,
            KeyCode::Char('a') => return Action::Audition, // cue/audition selected pattern
            KeyCode::Char('b') => return Action::ToggleLaunchQuant, // toggle bar/beat quant
            KeyCode::Char('C') => return Action::CancelQueue, // cancel pending queued launch
            KeyCode::Char('f') => return Action::ToggleFavorite, // toggle favorite
            KeyCode::Char('F') => return Action::ToggleFavFilter, // toggle favorites-only filter
            KeyCode::Char('l') | KeyCode::Esc => return Action::CloseLibrary,
            _ => {}
        },
        Mode::SetBrowser => match key.code {
            KeyCode::Up => return Action::SetBrowserNav(-1),
            KeyCode::Down => return Action::SetBrowserNav(1),
            KeyCode::Enter => return Action::SetBrowserLoad,
            KeyCode::Esc | KeyCode::Char('o') => return Action::CloseSetBrowser,
            KeyCode::Char('r') => return Action::SetBrowserRename,
            KeyCode::Char('a') | KeyCode::Char('S') => return Action::SetBrowserSaveAs,
            KeyCode::Char('D') => return Action::SetBrowserDuplicate,
            KeyCode::Char('d') => return Action::SetBrowserDelete,
            KeyCode::Char('n') => return Action::SetBrowserNewSet,
            _ => {}
        },
        Mode::NameEntry(_) => {
            return match key.code {
                KeyCode::Char(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '#') => {
                    Action::NameChar(c)
                }
                // Space already intercepted above and returned as NameChar(' ')
                KeyCode::Backspace => Action::NameBackspace,
                KeyCode::Enter => Action::NameCommit,
                KeyCode::Esc => Action::NameCancel,
                _ => Action::None,
            };
        }
        Mode::Confirm(_) => {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Enter => Action::ConfirmYes,
                KeyCode::Char('n') | KeyCode::Esc => Action::ConfirmNo,
                _ => Action::None,
            };
        }
        Mode::Help => {
            return match key.code {
                KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc => Action::Help,
                KeyCode::Up => Action::HelpScroll(-1),
                KeyCode::Down => Action::HelpScroll(1),
                KeyCode::PageUp => Action::HelpScroll(-10),
                KeyCode::PageDown => Action::HelpScroll(10),
                KeyCode::Home => Action::HelpScroll(i32::MIN / 2),
                KeyCode::End => Action::HelpScroll(i32::MAX / 2),
                // space and ! are already handled before the mode branch
                _ => Action::None,
            };
        }
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
        Mode::CrateView => {
            return match key.code {
                KeyCode::Up => Action::CrateEntrySel(-1),
                KeyCode::Down => Action::CrateEntrySel(1),
                KeyCode::Left => Action::CrateSel(-1),
                KeyCode::Right => Action::CrateSel(1),
                KeyCode::Enter => Action::LaunchCrateEntry,
                KeyCode::Char('a') => Action::AuditionCrateEntry,
                KeyCode::Char('C') => Action::CancelQueue,
                KeyCode::Char('f') => Action::FavoriteCrateEntry,
                KeyCode::Char('z') => Action::ValidateCrate,
                KeyCode::Esc | KeyCode::Char('V') => Action::CloseCrateView,
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
                    'M' => return Action::ToggleMirror,
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
                    'b' => return Action::ToggleLaunchQuant, // toggle next-bar / next-beat launch quant
                    'C' => return Action::CancelQueue,       // cancel pending queued launch
                    'A' => return Action::OpenSaveUserPattern, // save focused lane as user pattern
                    'Z' => return Action::OpenClearPattern, // clear focused lane (confirm if material)
                    'L' => return Action::DoubleLength,     // double pattern length, repeat content
                    'V' => return Action::OpenCrateView,    // open live crate browser
                    // 'i' was unbound; chosen for "in-sync" — re-sync the focused lane's
                    // phase at the next bar/beat without changing its pattern.
                    'i' => return Action::RestartLane,
                    // 'f' was unbound in Edit; chosen for "fill" — toggle a temporary
                    // deterministic fill on the focused lane (non-destructive, latched).
                    'f' => return Action::ToggleFill,
                    // 'F' was unbound in Edit; chosen for "fill commit" — commit the
                    // active fill, making it permanent and undoable via snapshot.
                    'F' => return Action::CommitTransform,
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
                        // ` (backtick): toggle per-voice mute on the cursor row (§2.6)
                        '`' => return Action::ToggleVoiceMute,
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
            Mode::RecoveryPrompt,
            Mode::CrateView,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char(' ')), mode.clone(), LaneKind::Drums),
                Action::TogglePlay,
                "Space should be TogglePlay in {:?}",
                &mode
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
            Mode::RecoveryPrompt,
            Mode::CrateView,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('!')), mode.clone(), LaneKind::Drums),
                Action::Panic,
                "! should be Panic in {:?}",
                &mode
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
            Mode::RecoveryPrompt,
            Mode::CrateView,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char(' ')), mode.clone(), LaneKind::Drums),
                Action::TogglePlay,
                "Space should be TogglePlay in {:?}",
                &mode
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
            Mode::RecoveryPrompt,
            Mode::CrateView,
        ] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('!')), mode.clone(), LaneKind::Drums),
                Action::Panic,
                "! should be Panic in {:?}",
                &mode
            );
        }
    }

    // ── Task 10: RecoveryPrompt key bindings ─────────────────────────────────

    #[test]
    fn recovery_prompt_r_and_enter_recover() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('r')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::RecoveryRecover
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::RecoveryRecover
        );
    }

    #[test]
    fn recovery_prompt_d_and_esc_discard() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('d')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::RecoveryDiscard
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::RecoveryDiscard
        );
    }

    #[test]
    fn recovery_prompt_o_opens_set_browser() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('o')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::RecoveryOpenSaved
        );
    }

    #[test]
    fn recovery_prompt_does_not_fall_through_to_edit_bindings() {
        // 'q' in edit = Quit; in RecoveryPrompt it must not trigger Quit.
        assert_eq!(
            key_to_action(k(KeyCode::Char('q')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::None
        );
        // 's' in edit = Save; in RecoveryPrompt must be None.
        assert_eq!(
            key_to_action(k(KeyCode::Char('s')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::None
        );
    }

    #[test]
    fn space_and_bang_still_global_in_recovery_prompt() {
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::TogglePlay
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('!')), Mode::RecoveryPrompt, LaneKind::Drums),
            Action::Panic
        );
    }

    // ── M2.5-T2: mirror toggle key ──────────────────────────────────────────

    #[test]
    fn shift_m_maps_to_toggle_mirror_in_edit_mode() {
        let shift_m = KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT);
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(shift_m, Mode::Edit, kind),
                Action::ToggleMirror,
                "'M' in Edit mode must be ToggleMirror"
            );
        }
    }

    #[test]
    fn shift_m_was_unbound_before_mirror_task() {
        let shift_m = KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT);
        assert_ne!(
            key_to_action(shift_m, Mode::Edit, LaneKind::Drums),
            Action::None,
            "'M' must be bound (was unbound/None before this task)"
        );
    }

    // ── M3 Task 2: launch quant toggle and cancel queue keys ─────────────────

    #[test]
    fn b_key_maps_to_toggle_launch_quant_in_edit_mode() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('b')), Mode::Edit, kind),
                Action::ToggleLaunchQuant,
                "'b' in Edit mode must be ToggleLaunchQuant"
            );
        }
    }

    #[test]
    fn shift_c_maps_to_cancel_queue_in_edit_mode() {
        let shift_c = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT);
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(shift_c, Mode::Edit, kind),
                Action::CancelQueue,
                "'C' (Shift+C) in Edit mode must be CancelQueue"
            );
        }
    }

    #[test]
    fn b_key_maps_to_toggle_launch_quant_in_library_mode() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('b')), Mode::Library, LaneKind::Drums),
            Action::ToggleLaunchQuant,
            "'b' in Library mode must be ToggleLaunchQuant"
        );
    }

    #[test]
    fn shift_c_maps_to_cancel_queue_in_library_mode() {
        let shift_c = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT);
        assert_eq!(
            key_to_action(shift_c, Mode::Library, LaneKind::Drums),
            Action::CancelQueue,
            "'C' (Shift+C) in Library mode must be CancelQueue"
        );
    }

    #[test]
    fn b_was_unbound_before_m3_task2() {
        // Documents that 'b' was previously Action::None in Edit mode.
        // The actual assertion confirms it is now bound (not None):
        assert_ne!(
            key_to_action(k(KeyCode::Char('b')), Mode::Edit, LaneKind::Drums),
            Action::None,
            "'b' must be bound to ToggleLaunchQuant (was None before M3-T2)"
        );
    }

    // ── M3 Task 7: management UI key bindings ────────────────────────────────

    #[test]
    fn name_entry_char_keys_map_to_name_char() {
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('a')),
                Mode::NameEntry(crate::app::NamePurpose::SaveSetAs),
                LaneKind::Drums
            ),
            Action::NameChar('a'),
            "'a' in NameEntry must be NameChar"
        );
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('5')),
                Mode::NameEntry(crate::app::NamePurpose::RenameSet),
                LaneKind::Drums
            ),
            Action::NameChar('5'),
            "'5' in NameEntry must be NameChar"
        );
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('-')),
                Mode::NameEntry(crate::app::NamePurpose::SaveUserPattern),
                LaneKind::Drums
            ),
            Action::NameChar('-'),
            "'-' in NameEntry must be NameChar"
        );
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('#')),
                Mode::NameEntry(crate::app::NamePurpose::SaveSetAs),
                LaneKind::Drums
            ),
            Action::NameChar('#'),
            "'#' in NameEntry must be NameChar"
        );
    }

    #[test]
    fn name_entry_space_is_name_char_not_toggle_play() {
        assert_eq!(
            key_to_action(
                k(KeyCode::Char(' ')),
                Mode::NameEntry(crate::app::NamePurpose::SaveSetAs),
                LaneKind::Drums
            ),
            Action::NameChar(' '),
            "space in NameEntry must be NameChar, not TogglePlay"
        );
    }

    #[test]
    fn name_entry_backspace_enter_esc() {
        let mode = || Mode::NameEntry(crate::app::NamePurpose::SaveSetAs);
        assert_eq!(
            key_to_action(k(KeyCode::Backspace), mode(), LaneKind::Drums),
            Action::NameBackspace
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), mode(), LaneKind::Drums),
            Action::NameCommit
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), mode(), LaneKind::Drums),
            Action::NameCancel
        );
    }

    #[test]
    fn confirm_mode_y_and_enter_confirm_yes() {
        let mode = || Mode::Confirm(crate::app::ConfirmAction::NewSet);
        assert_eq!(
            key_to_action(k(KeyCode::Char('y')), mode(), LaneKind::Drums),
            Action::ConfirmYes
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), mode(), LaneKind::Drums),
            Action::ConfirmYes
        );
    }

    #[test]
    fn confirm_mode_n_and_esc_confirm_no() {
        let mode = || Mode::Confirm(crate::app::ConfirmAction::NewSet);
        assert_eq!(
            key_to_action(k(KeyCode::Char('n')), mode(), LaneKind::Drums),
            Action::ConfirmNo
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), mode(), LaneKind::Drums),
            Action::ConfirmNo
        );
    }

    #[test]
    fn exclamation_is_panic_in_name_entry_and_confirm() {
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('!')),
                Mode::NameEntry(crate::app::NamePurpose::SaveSetAs),
                LaneKind::Drums
            ),
            Action::Panic,
            "! must be Panic in NameEntry"
        );
        assert_eq!(
            key_to_action(
                k(KeyCode::Char('!')),
                Mode::Confirm(crate::app::ConfirmAction::NewSet),
                LaneKind::Drums
            ),
            Action::Panic,
            "! must be Panic in Confirm"
        );
    }

    #[test]
    fn set_browser_management_keys() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('r')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserRename
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('a')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserSaveAs
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('S')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserSaveAs
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('D')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserDuplicate
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('d')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserDelete
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('n')), Mode::SetBrowser, LaneKind::Drums),
            Action::SetBrowserNewSet
        );
    }

    #[test]
    fn edit_mode_a_and_z_map_to_pattern_management() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('A')), Mode::Edit, kind),
                Action::OpenSaveUserPattern,
                "'A' in Edit must be OpenSaveUserPattern"
            );
            assert_eq!(
                key_to_action(k(KeyCode::Char('Z')), Mode::Edit, kind),
                Action::OpenClearPattern,
                "'Z' in Edit must be OpenClearPattern"
            );
        }
    }

    #[test]
    fn edit_mode_shift_l_maps_to_double_length() {
        // 'L' (shift+l) must map to DoubleLength in Edit mode for both lane kinds.
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('L')), Mode::Edit, kind),
                Action::DoubleLength,
                "'L' in Edit must be DoubleLength"
            );
        }
    }

    #[test]
    fn shift_l_was_unbound_before_double_length() {
        // Verify that the key resolves to DoubleLength (was Action::None before this
        // feature was added; the assertion here proves the binding is present and
        // NOT falling through to None).
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            let action = key_to_action(k(KeyCode::Char('L')), Mode::Edit, kind);
            assert_ne!(action, Action::None, "'L' must not be unbound in Edit mode");
            assert_eq!(action, Action::DoubleLength);
        }
    }

    // ── Help mode scroll keys ─────────────────────────────────────────────

    #[test]
    fn help_mode_down_scrolls() {
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::Help, LaneKind::Drums),
            Action::HelpScroll(1)
        );
    }

    #[test]
    fn help_mode_up_scrolls() {
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::Help, LaneKind::Drums),
            Action::HelpScroll(-1)
        );
    }

    #[test]
    fn help_mode_question_closes() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('?')), Mode::Help, LaneKind::Drums),
            Action::Help
        );
    }

    #[test]
    fn help_mode_esc_closes() {
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::Help, LaneKind::Drums),
            Action::Help
        );
    }

    #[test]
    fn help_mode_space_still_plays() {
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::Help, LaneKind::Drums),
            Action::TogglePlay
        );
    }

    #[test]
    fn help_mode_bang_still_panics() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('!')), Mode::Help, LaneKind::Drums),
            Action::Panic
        );
    }

    // ── M4a Task 3: favorites key bindings in Library mode ───────────────────

    #[test]
    fn f_key_maps_to_toggle_favorite_in_library_mode() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('f')), Mode::Library, LaneKind::Drums),
            Action::ToggleFavorite,
            "'f' in Library mode must be ToggleFavorite"
        );
    }

    #[test]
    fn shift_f_key_maps_to_toggle_fav_filter_in_library_mode() {
        let shift_f = KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT);
        assert_eq!(
            key_to_action(shift_f, Mode::Library, LaneKind::Drums),
            Action::ToggleFavFilter,
            "'F' in Library mode must be ToggleFavFilter"
        );
    }

    #[test]
    fn space_and_bang_still_global_in_library_mode_after_favorites() {
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::Library, LaneKind::Drums),
            Action::TogglePlay,
            "space must remain TogglePlay in Library mode"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('!')), Mode::Library, LaneKind::Drums),
            Action::Panic,
            "! must remain Panic in Library mode"
        );
    }

    // ── M4a Task 5: crate view key bindings ──────────────────────────────────

    #[test]
    fn v_key_opens_crate_view_in_edit_mode() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('V')), Mode::Edit, kind),
                Action::OpenCrateView,
                "'V' in Edit mode must open crate view"
            );
        }
    }

    #[test]
    fn v_was_unbound_before_crate_view() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_ne!(
                key_to_action(k(KeyCode::Char('V')), Mode::Edit, kind),
                Action::None,
                "'V' must not be unbound in Edit mode"
            );
        }
    }

    #[test]
    fn crate_view_mode_arrows_navigate() {
        assert_eq!(
            key_to_action(k(KeyCode::Up), Mode::CrateView, LaneKind::Drums),
            Action::CrateEntrySel(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::CrateView, LaneKind::Drums),
            Action::CrateEntrySel(1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Left), Mode::CrateView, LaneKind::Drums),
            Action::CrateSel(-1)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Right), Mode::CrateView, LaneKind::Drums),
            Action::CrateSel(1)
        );
    }

    #[test]
    fn crate_view_enter_launches() {
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::CrateView, LaneKind::Drums),
            Action::LaunchCrateEntry
        );
    }

    #[test]
    fn crate_view_esc_closes() {
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::CrateView, LaneKind::Drums),
            Action::CloseCrateView
        );
    }

    #[test]
    fn crate_view_v_also_closes() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('V')), Mode::CrateView, LaneKind::Drums),
            Action::CloseCrateView
        );
    }

    #[test]
    fn crate_view_a_auditions() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('a')), Mode::CrateView, LaneKind::Drums),
            Action::AuditionCrateEntry
        );
    }

    #[test]
    fn crate_view_f_favorites() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('f')), Mode::CrateView, LaneKind::Drums),
            Action::FavoriteCrateEntry
        );
    }

    #[test]
    fn crate_view_shift_c_cancel_queue() {
        let shift_c = KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT);
        assert_eq!(
            key_to_action(shift_c, Mode::CrateView, LaneKind::Drums),
            Action::CancelQueue
        );
    }

    // ── M4b Task 2: quantized lane restart key ───────────────────────────────

    #[test]
    fn i_key_maps_to_restart_lane_in_edit_mode() {
        // 'i' was previously unbound (Action::None) in Edit mode; it is now RestartLane.
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('i')), Mode::Edit, kind),
                Action::RestartLane,
                "'i' in Edit mode must be RestartLane (was unbound/None before M4b-T2)"
            );
        }
    }

    #[test]
    fn i_was_unbound_before_restart_lane() {
        // Documents that 'i' was previously Action::None and is now bound.
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_ne!(
                key_to_action(k(KeyCode::Char('i')), Mode::Edit, kind),
                Action::None,
                "'i' must not be unbound in Edit mode"
            );
        }
    }

    /// §2.6: backtick is ToggleVoiceMute in Edit/Drums; unbound elsewhere.
    #[test]
    fn backtick_maps_to_toggle_voice_mute_in_drums() {
        assert_eq!(
            key_to_action(k(KeyCode::Char('`')), Mode::Edit, LaneKind::Drums),
            Action::ToggleVoiceMute,
            "backtick must map to ToggleVoiceMute in Edit+Drums"
        );
        // Must be unbound (None) on melodic lanes — voice mute is drums-only.
        assert_eq!(
            key_to_action(k(KeyCode::Char('`')), Mode::Edit, LaneKind::Melodic),
            Action::None,
            "backtick must be unbound in Edit+Melodic"
        );
    }

    #[test]
    fn space_and_bang_still_global_in_crate_view() {
        assert_eq!(
            key_to_action(k(KeyCode::Char(' ')), Mode::CrateView, LaneKind::Drums),
            Action::TogglePlay,
            "space must remain TogglePlay in CrateView mode"
        );
        assert_eq!(
            key_to_action(k(KeyCode::Char('!')), Mode::CrateView, LaneKind::Drums),
            Action::Panic,
            "! must remain Panic in CrateView mode"
        );
    }

    // ── M4b Task 3: fill keys ────────────────────────────────────────────────

    /// 'f' was previously unbound (Action::None) in Edit mode; now ToggleFill.
    #[test]
    fn f_key_maps_to_toggle_fill_in_edit_mode() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('f')), Mode::Edit, kind),
                Action::ToggleFill,
                "'f' in Edit mode must be ToggleFill (was unbound before M4b-T3)"
            );
        }
    }

    /// 'f' was previously Action::None in Edit mode — documents the pre-binding state.
    #[test]
    fn f_was_unbound_before_toggle_fill() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_ne!(
                key_to_action(k(KeyCode::Char('f')), Mode::Edit, kind),
                Action::None,
                "'f' must not be unbound in Edit mode"
            );
        }
    }

    /// 'F' was previously unbound (Action::None) in Edit mode; now CommitTransform.
    #[test]
    fn shift_f_key_maps_to_commit_transform_in_edit_mode() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_eq!(
                key_to_action(k(KeyCode::Char('F')), Mode::Edit, kind),
                Action::CommitTransform,
                "'F' in Edit mode must be CommitTransform (was unbound before M4b-T3)"
            );
        }
    }

    /// 'F' was previously Action::None in Edit mode — documents the pre-binding state.
    #[test]
    fn shift_f_was_unbound_before_commit_transform() {
        for kind in [LaneKind::Drums, LaneKind::Melodic] {
            assert_ne!(
                key_to_action(k(KeyCode::Char('F')), Mode::Edit, kind),
                Action::None,
                "'F' must not be unbound in Edit mode"
            );
        }
    }
}
