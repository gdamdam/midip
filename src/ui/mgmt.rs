//! Overlays for name-entry, confirm dialogs, and note-input banner
//! (Mode::NameEntry / Mode::Confirm / Mode::NoteInput).

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, ConfirmAction, Mode, NamePurpose};

/// Render the name-entry dialog into `area`.
pub fn render_name_entry(f: &mut Frame, area: Rect, app: &App) {
    let (title, prompt) = match &app.mode {
        Mode::NameEntry(NamePurpose::SaveSetAs) => (" SAVE SET AS ", "Set name:"),
        Mode::NameEntry(NamePurpose::RenameSet) => (" RENAME SET ", "New name:"),
        Mode::NameEntry(NamePurpose::SaveUserPattern) => (" SAVE USER PATTERN ", "Pattern name:"),
        _ => (" NAME ", "Name:"),
    };

    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cursor_line = format!("{}_", app.name_input);
    let lines: Vec<Line> = vec![
        Line::from(Span::raw(prompt)),
        Line::from(Span::styled(
            cursor_line,
            // fg+BOLD for active input cursor — warn (amber) is reserved for warnings/pending.
            Style::default().fg(EMBER.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::raw("[enter] confirm  [esc] cancel")),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Render the confirmation dialog into `area`.
pub fn render_confirm(f: &mut Frame, area: Rect, app: &App) {
    let (title, message): (&str, String) = match &app.mode {
        Mode::Confirm(ConfirmAction::NewSet) => (
            " NEW SET ",
            "Unsaved changes will be lost. Create new set?".into(),
        ),
        Mode::Confirm(ConfirmAction::DeleteSet(_)) => {
            (" DELETE SET ", "Delete this set file permanently?".into())
        }
        Mode::Confirm(ConfirmAction::LoadSet(_)) => (
            " LOAD SET ",
            "Unsaved changes will be lost. Load this set?".into(),
        ),
        Mode::Confirm(ConfirmAction::ClearPattern) => {
            (" CLEAR PATTERN ", "Clear the focused lane pattern?".into())
        }
        Mode::Confirm(ConfirmAction::ConformToScale(n)) => {
            let lane = &app.set.lanes[app.focus];
            (
                " CONFORM TO SCALE ",
                format!("Conform {} note(s) to {}? [y/n]", n, lane.scale.name()),
            )
        }
        Mode::Confirm(ConfirmAction::DeleteScene(_)) => {
            (" DELETE SCENE ", "Delete this scene permanently?".into())
        }
        _ => (" CONFIRM ", "Are you sure?".into()),
    };

    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = vec![
        Line::from(Span::raw(message)),
        Line::from(""),
        Line::from(Span::styled(
            "[y / enter]  yes    [n / esc]  no",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Render the NOTE INPUT sub-mode banner into `area`.
///
/// Shows the active octave offset and the keymap layout so the performer has an
/// at-a-glance reference without leaving the sub-mode.
pub fn render_note_input(f: &mut Frame, area: Rect, app: &App) {
    let octave = match &app.mode {
        Mode::NoteInput => app.note_input_octave,
        _ => 0,
    };
    let octave_str = format!("oct {:+}", octave);

    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(" NOTE INPUT ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("white: a s d f g h j k   black: w e t y u   {}", octave_str),
            // fg+BOLD for keymap banner — not a warning, warn (amber) stays for attention states.
            Style::default().fg(EMBER.fg).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("[z]oct-  [x]oct+  [bksp]del step  [esc]exit")),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            v2_index: Default::default(),
            families: Vec::new(),
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn new_app() -> App {
        App::new(Set::default_set(default_profiles()), empty_library())
    }

    fn render_name_entry_to_string(app: &App) -> String {
        let backend = TestBackend::new(80, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_name_entry(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn render_confirm_to_string(app: &App) -> String {
        let backend = TestBackend::new(80, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_confirm(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn name_entry_save_set_as_shows_title_and_prompt() {
        let mut app = new_app();
        app.mode = Mode::NameEntry(NamePurpose::SaveSetAs);
        app.name_input = "my set".to_string();
        let s = render_name_entry_to_string(&app);
        assert!(s.contains("SAVE SET AS"), "expected title; got: {s:?}");
        assert!(s.contains("Set name:"), "expected prompt; got: {s:?}");
        assert!(s.contains("my set"), "expected buffer text; got: {s:?}");
        assert!(
            s.contains("confirm"),
            "expected [enter] confirm hint; got: {s:?}"
        );
    }

    #[test]
    fn name_entry_rename_set_shows_correct_title() {
        let mut app = new_app();
        app.mode = Mode::NameEntry(NamePurpose::RenameSet);
        let s = render_name_entry_to_string(&app);
        assert!(
            s.contains("RENAME SET"),
            "expected RENAME SET title; got: {s:?}"
        );
    }

    #[test]
    fn name_entry_save_user_pattern_shows_correct_title() {
        let mut app = new_app();
        app.mode = Mode::NameEntry(NamePurpose::SaveUserPattern);
        let s = render_name_entry_to_string(&app);
        assert!(
            s.contains("SAVE USER PATTERN"),
            "expected SAVE USER PATTERN title; got: {s:?}"
        );
    }

    #[test]
    fn confirm_new_set_shows_unsaved_warning() {
        let mut app = new_app();
        app.mode = Mode::Confirm(ConfirmAction::NewSet);
        let s = render_confirm_to_string(&app);
        assert!(s.contains("NEW SET"), "expected NEW SET title; got: {s:?}");
        assert!(
            s.contains("Unsaved"),
            "expected unsaved warning; got: {s:?}"
        );
        assert!(s.contains("yes"), "expected [y] yes hint; got: {s:?}");
    }

    #[test]
    fn confirm_delete_set_shows_delete_message() {
        let mut app = new_app();
        app.mode = Mode::Confirm(ConfirmAction::DeleteSet(std::path::PathBuf::from(
            "/tmp/test.json",
        )));
        let s = render_confirm_to_string(&app);
        assert!(
            s.contains("DELETE SET"),
            "expected DELETE SET title; got: {s:?}"
        );
        assert!(s.contains("Delete"), "expected Delete message; got: {s:?}");
    }

    #[test]
    fn confirm_clear_pattern_shows_clear_message() {
        let mut app = new_app();
        app.mode = Mode::Confirm(ConfirmAction::ClearPattern);
        let s = render_confirm_to_string(&app);
        assert!(
            s.contains("CLEAR PATTERN"),
            "expected CLEAR PATTERN title; got: {s:?}"
        );
        assert!(s.contains("Clear"), "expected Clear message; got: {s:?}");
    }

    fn render_note_input_to_string(app: &App) -> String {
        let backend = TestBackend::new(80, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_note_input(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    /// The NOTE INPUT banner renders the title and keymap hint.
    #[test]
    fn note_input_banner_shows_title_and_keymap() {
        let mut app = new_app();
        app.mode = Mode::NoteInput;
        let s = render_note_input_to_string(&app);
        assert!(
            s.contains("NOTE INPUT"),
            "expected NOTE INPUT title; got: {s:?}"
        );
        assert!(s.contains("white:"), "expected white key hint; got: {s:?}");
        assert!(s.contains("esc"), "expected [esc]exit hint; got: {s:?}");
    }

    /// The banner shows the current octave offset.
    #[test]
    fn note_input_banner_shows_octave_offset() {
        let mut app = new_app();
        app.mode = Mode::NoteInput;
        app.note_input_octave = 2;
        let s = render_note_input_to_string(&app);
        assert!(
            s.contains("+2"),
            "expected octave offset +2 in banner; got: {s:?}"
        );
    }
}
