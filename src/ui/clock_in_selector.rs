//! Clock-in source selector overlay.
//!
//! Opens via [W] from Edit mode. User picks an input MIDI port (or "(none)" to
//! revert to manual tempo). Dispatches UiCommand::SetClockInPort on Enter.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

pub fn render_clock_in_selector(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " CLK-IN SOURCE ",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        "Select MIDI input port for external clock (MIDI Clock / F8):",
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(""));

    // Entry 0: "(none)" — clear clock-in, revert to manual.
    let none_sel = app.clock_in_sel == 0;
    lines.push(Line::from(Span::styled(
        if none_sel {
            "▶ (none — manual tempo)"
        } else {
            "  (none — manual tempo)"
        },
        if none_sel {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));

    // Entries 1..N: real input ports.
    for (i, port) in app.clock_in_ports.iter().enumerate() {
        let idx = i + 1;
        let selected = app.clock_in_sel == idx;
        lines.push(Line::from(Span::styled(
            format!("{} {}", if selected { "▶" } else { " " }, port),
            if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        )));
    }

    if app.clock_in_ports.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no MIDI input ports found)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[↑↓] select  [enter] confirm  [esc] cancel",
        Style::default().fg(Color::DarkGray),
    )));

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

    fn make_app() -> App {
        let set = Set::default_set(default_profiles());
        App::new(
            set,
            Library {
                drums: GenreMap::new(),
                bass: GenreMap::new(),
                synth: GenreMap::new(),
            },
        )
    }

    fn render_sel(app: &App) -> String {
        let backend = TestBackend::new(80, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_clock_in_selector(f, f.area(), app))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn shows_none_selected_by_default() {
        let app = make_app();
        let text = render_sel(&app);
        assert!(
            text.contains("manual tempo"),
            "expected manual tempo: {text:?}"
        );
    }

    #[test]
    fn shows_port_when_ports_available() {
        let mut app = make_app();
        app.clock_in_ports = vec!["FakeDevice".to_string()];
        app.clock_in_sel = 1;
        let text = render_sel(&app);
        assert!(text.contains("FakeDevice"), "expected port name: {text:?}");
    }

    #[test]
    fn shows_empty_message_when_no_ports() {
        let app = make_app();
        let text = render_sel(&app);
        assert!(
            text.contains("no MIDI input ports found"),
            "expected empty-ports message: {text:?}"
        );
    }
}
