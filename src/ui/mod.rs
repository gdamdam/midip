pub mod editor_drums;
pub mod editor_melodic;
pub mod help;
pub mod lanes;
pub mod library;
pub mod theme;
pub mod transport;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::pattern::model::LaneKind;

const FOOTER: &str = "[space]play [tab/shift+tab]lane [↑↓←→]move/pitch [g]slide [enter]toggle [0-9]vel \
[<>]swing [{}]len [^z]undo [esc]panic [l]ibrary [s]ave [t]empo [;/']BPM−/+ [k]link [?]help [q]uit";

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

/// Top-level render: transport / lanes / editor / footer with overlay support.
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
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

    f.render_widget(Paragraph::new(FOOTER), chunks[3]);

    match app.mode {
        Mode::Library => library::render_library(f, centered(area, 90, 70), app),
        Mode::Help => help::render_help(f, centered(area, 60, 70)),
        Mode::Edit | Mode::TempoEntry => {}
    }
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
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
    }

    #[test]
    fn render_draws_without_panic_and_shows_footer() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        let backend = TestBackend::new(100, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // Footer hint contains "play".
        assert!(whole.contains("play"), "expected footer hint, got: {whole:?}");
    }
}
