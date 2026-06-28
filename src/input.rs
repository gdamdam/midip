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

    match mode {
        Mode::Library => match key.code {
            KeyCode::Up => return Action::LibNav(-1, 0),
            KeyCode::Down => return Action::LibNav(1, 0),
            KeyCode::Left => return Action::LibNav(0, -1),
            KeyCode::Right => return Action::LibNav(0, 1),
            KeyCode::Enter => return Action::LibLoad,
            KeyCode::Char('l') | KeyCode::Esc => return Action::CloseLibrary,
            _ => {}
        },
        Mode::Help => return Action::Help,
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
                    let n = n as u8;
                    match n {
                        1 => return Action::FocusLane(0),
                        2 => return Action::FocusLane(1),
                        3 => return Action::FocusLane(2),
                        _ => return Action::SetVelBucket(n),
                    }
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
        assert_eq!(
            key_to_action(k(KeyCode::Down), Mode::Library, LaneKind::Drums),
            Action::LibNav(1, 0)
        );
        assert_eq!(
            key_to_action(k(KeyCode::Enter), Mode::Library, LaneKind::Drums),
            Action::LibLoad
        );
        assert_eq!(
            key_to_action(k(KeyCode::Esc), Mode::Library, LaneKind::Drums),
            Action::CloseLibrary
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
}
