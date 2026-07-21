//! Persistent top-of-frame workspace tab strip. Renders the five sibling
//! workspaces; the active one is highlighted with the same style the per-mode
//! hint bar uses for its context label.

use crate::app::{App, Workspace};
use crate::ui::theme::EMBER;
use ratatui::{prelude::*, widgets::*};

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    for ws in [
        Workspace::Perform,
        Workspace::Pattern,
        Workspace::Library,
        Workspace::Song,
        Workspace::Setup,
    ] {
        let active = ws == app.workspace;
        let style = if active {
            Style::default()
                .fg(EMBER.bg)
                .bg(EMBER.synth)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(EMBER.dim)
        };
        spans.push(Span::styled(format!(" {} ", ws.label()), style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Collect the symbols of every row-0 cell carrying the BOLD modifier, along
    /// with the background color of the first such cell. Only the active tab is
    /// bold, so this isolates the active label from the dim inactive ones.
    fn bold_run(app: &App) -> (String, Option<Color>) {
        let backend = TestBackend::new(80, 1);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, f.area(), app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut text = String::new();
        let mut bg = None;
        for x in 0..buf.area.width {
            let cell = buf.cell((x, 0)).unwrap();
            if cell.style().add_modifier.contains(Modifier::BOLD) {
                if bg.is_none() {
                    bg = cell.style().bg;
                }
                text.push_str(cell.symbol());
            }
        }
        (text, bg)
    }

    #[test]
    fn active_workspace_label_is_highlighted() {
        let app = crate::test_support::app_for_tests(); // defaults to Perform
        let (text, bg) = bold_run(&app);
        assert!(
            text.contains("PERFORM"),
            "active label should be bold, got: {text:?}"
        );
        assert!(
            !text.contains("PATTERN"),
            "inactive labels must not be bold, got: {text:?}"
        );
        assert_eq!(
            bg,
            Some(EMBER.synth),
            "active tab uses the synth background"
        );
    }

    #[test]
    fn switching_workspace_moves_the_highlight() {
        let mut app = crate::test_support::app_for_tests();
        app.set_workspace(Workspace::Setup);
        let (text, _) = bold_run(&app);
        assert!(
            text.contains("SETUP"),
            "expected SETUP highlighted, got: {text:?}"
        );
        assert!(
            !text.contains("PERFORM"),
            "PERFORM should no longer be bold: {text:?}"
        );
    }
}
