//! Crash-recovery prompt overlay.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Render the recovery prompt overlay into `area`.
pub fn render_recovery_prompt(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "  Unsaved work was recovered from an unclean shutdown.",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  What would you like to do?"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  [r] / [Enter]  ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Recover — load the autosaved work"),
        ]),
        Line::from(vec![
            Span::styled(
                "  [d] / [Esc]    ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Discard — start fresh (unsaved work will be lost)"),
        ]),
        Line::from(vec![
            Span::styled(
                "  [o]            ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Open saved — browse your saved sets"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("[space]", Style::default().fg(Color::DarkGray)),
            Span::raw(" play/stop   "),
            Span::styled("[!]", Style::default().fg(Color::DarkGray)),
            Span::raw(" panic"),
        ]),
    ];

    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " RECOVERY ",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_to_string(w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_recovery_prompt(f, f.area())).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn recovery_prompt_shows_title_and_options() {
        let whole = render_to_string(80, 15);
        assert!(
            whole.contains("RECOVERY"),
            "must show RECOVERY title; got: {whole:?}"
        );
        assert!(
            whole.contains("unclean shutdown"),
            "must mention unclean shutdown; got: {whole:?}"
        );
        assert!(
            whole.contains("Recover"),
            "must show Recover option; got: {whole:?}"
        );
        assert!(
            whole.contains("Discard"),
            "must show Discard option; got: {whole:?}"
        );
        assert!(
            whole.contains("Open saved"),
            "must show Open saved option; got: {whole:?}"
        );
    }

    #[test]
    fn recovery_prompt_shows_key_hints() {
        let whole = render_to_string(80, 15);
        assert!(whole.contains("[r]"), "must show [r] hint");
        assert!(whole.contains("[d]"), "must show [d] hint");
        assert!(whole.contains("[o]"), "must show [o] hint");
    }
}
