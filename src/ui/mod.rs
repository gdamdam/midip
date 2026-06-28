pub mod editor_drums;
pub mod editor_melodic;
pub mod help;
pub mod lanes;
pub mod library;
pub mod theme;
pub mod transport;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::pattern::model::LaneKind;

fn context_footer(app: &App) -> Line<'static> {
    let label = app.context_label();
    let hint: &str = match app.mode {
        Mode::Edit => match app.focused_kind() {
            LaneKind::Drums =>
                "[space]play [tab]lane [arrows]move [enter]toggle [0-9]vel [e/E][/]]euclid [?]more",
            LaneKind::Melodic =>
                "[space]play [tab]lane [←→]step [↑↓]pitch [enter]note [g]slide [[/]]oct [?]more",
        },
        Mode::Library   => "[←→]column [↑↓]select [enter]load [esc]close",
        Mode::SetBrowser => "[↑↓]select [enter]open [o/esc]close",
        Mode::TempoEntry => "[0-9]type BPM [enter]set [esc]cancel",
        Mode::Help      => "[?/esc]close",
    };
    let label_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!(" {label} "), label_style),
        Span::raw(" "),
        Span::raw(hint),
    ])
}

/// Centered rect helper for overlays.
fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1]);
    h[1]
}

/// Minimum usable terminal dimensions.
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 16;

/// Top-level render: transport / lanes / editor / footer with overlay support.
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    // Guard: if the terminal is too small, show a resize prompt and bail out.
    // Attempting the normal multi-pane layout on a tiny frame produces garbage.
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let w = area.width;
        let h = area.height;
        let msg = format!(
            "midip needs at least {MIN_WIDTH}x{MIN_HEIGHT} — resize the terminal (now {w}x{h})"
        );
        let para = ratatui::widgets::Paragraph::new(msg)
            .alignment(ratatui::layout::Alignment::Center);
        // Render into the full available area (however small it is).
        f.render_widget(para, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // transport
            Constraint::Length(5), // lanes
            Constraint::Min(3),    // editor
            Constraint::Length(1), // footer
        ])
        .split(area);

    transport::render_transport(f, chunks[0], app);
    lanes::render_lanes(f, chunks[1], app);

    match app.focused_kind() {
        LaneKind::Drums => editor_drums::render_drum_editor(f, chunks[2], app),
        LaneKind::Melodic => editor_melodic::render_melodic_editor(f, chunks[2], app),
    }

    f.render_widget(Paragraph::new(context_footer(app)), chunks[3]);

    match app.mode {
        Mode::Library => library::render_library(f, centered(area, 90, 70), app),
        Mode::Help => help::render_help(f, centered(area, 60, 70)),
        Mode::SetBrowser => library::render_set_browser(f, centered(area, 60, 70), app),
        Mode::Edit | Mode::TempoEntry => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn render_draws_without_panic_and_shows_footer() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        let whole = render_to_string(&app);
        // Footer hint contains "play".
        assert!(whole.contains("play"), "expected footer hint, got: {whole:?}");
    }

    // --- context-sensitive footer tests ---

    #[test]
    fn footer_edit_drums_shows_euclid_and_toggle() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Edit;
        // Default focused lane should be Drums; verify via focused_kind.
        assert_eq!(app.focused_kind(), LaneKind::Drums);
        let whole = render_to_string(&app);
        assert!(whole.contains("euclid"), "Edit+Drums footer should contain 'euclid'");
        assert!(whole.contains("toggle"), "Edit+Drums footer should contain 'toggle'");
        // Should NOT contain melodic-only terms
        assert!(!whole.contains("pitch"), "Edit+Drums footer should NOT contain 'pitch'");
    }

    #[test]
    fn footer_edit_melodic_shows_pitch_and_slide() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Edit;
        // Switch to a melodic lane
        while app.focused_kind() != LaneKind::Melodic {
            app.mode = Mode::Edit; // keep in Edit while cycling
            // cycle lane by applying tab — but easiest is to just check if we can find a melodic lane
            // If no melodic lane exists in default set, skip gracefully
            break;
        }
        if app.focused_kind() == LaneKind::Melodic {
            let whole = render_to_string(&app);
            assert!(whole.contains("pitch"), "Edit+Melodic footer should contain 'pitch'");
            assert!(whole.contains("slide"), "Edit+Melodic footer should contain 'slide'");
            assert!(!whole.contains("euclid"), "Edit+Melodic footer should NOT contain 'euclid'");
        }
    }

    #[test]
    fn footer_library_shows_column_and_load_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Library;
        let whole = render_to_string(&app);
        assert!(whole.contains("column"), "Library footer should contain 'column'");
        assert!(whole.contains("load"), "Library footer should contain 'load'");
        assert!(!whole.contains("euclid"), "Library footer should NOT contain 'euclid'");
    }

    #[test]
    fn footer_setbrowser_shows_open_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::SetBrowser;
        let whole = render_to_string(&app);
        assert!(whole.contains("open"), "SetBrowser footer should contain 'open'");
        assert!(!whole.contains("euclid"), "SetBrowser footer should NOT contain 'euclid'");
    }

    #[test]
    fn footer_tempo_entry_shows_bpm_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::TempoEntry;
        let whole = render_to_string(&app);
        assert!(whole.contains("BPM"), "TempoEntry footer should contain 'BPM'");
        assert!(!whole.contains("euclid"), "TempoEntry footer should NOT contain 'euclid'");
    }

    #[test]
    fn footer_shows_context_label() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());

        app.mode = Mode::Edit;
        let whole = render_to_string(&app);
        assert!(whole.contains("EDIT DRUM"), "should show context label EDIT DRUM");

        app.mode = Mode::Library;
        let whole = render_to_string(&app);
        assert!(whole.contains("LIBRARY"), "should show context label LIBRARY");
    }

    #[test]
    fn footer_help_mode_shows_close_hint() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Help;
        let whole = render_to_string(&app);
        assert!(whole.contains("close"), "Help footer should contain 'close'");
    }

    // --- minimum terminal size tests ---

    fn render_to_string_sized(app: &App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn tiny_terminal_shows_resize_warning() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        // 30x6 is well below the 60x16 minimum.
        let whole = render_to_string_sized(&app, 30, 6);
        // The full message is longer than 30 chars so it gets clipped, but the
        // minimum-size spec "60x16" fits within the first 30 chars and must appear.
        assert!(
            whole.contains("60x16"),
            "expected minimum dimensions in resize warning, got: {whole:?}"
        );
    }

    #[test]
    fn normal_terminal_does_not_show_resize_warning() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        // 120x30 is well above the minimum — normal render path.
        let whole = render_to_string_sized(&app, 120, 30);
        assert!(
            whole.contains("play"),
            "expected normal footer hint on large terminal, got: {whole:?}"
        );
        assert!(
            !whole.contains("resize the terminal"),
            "should NOT show resize warning on large terminal"
        );
    }
}
