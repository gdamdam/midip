//! Persistent top-of-frame workspace tab strip. Renders the five sibling
//! workspaces; the active one is highlighted with the same style the per-mode
//! hint bar uses for its context label.

use crate::app::{App, Workspace};
use crate::ui::theme::EMBER;
use ratatui::{prelude::*, widgets::*};

pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    // Feature: mouse hit regions. Track the x cursor of each " LABEL " cell as
    // it is emitted so the recorded ranges match the drawn geometry exactly.
    let mut tabs: Vec<(std::ops::Range<u16>, Workspace)> = Vec::new();
    let mut x = area.x;
    let x_max = area.x + area.width; // clip to the strip's own area
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
        let label = format!(" {} ", ws.label());
        // Labels are ASCII, so chars == columns.
        let w = label.chars().count() as u16;
        tabs.push((x..(x + w).min(x_max), ws));
        x += w + 1; // +1 for the inter-tab gap (not clickable)
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    app.register_tab_hits(area.y, &tabs);
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

    /// The regions registered during render must map each drawn label's
    /// columns to its workspace, so a click lands on the tab under the pointer.
    #[test]
    fn render_registers_clickable_tab_regions() {
        use crate::app::HitTarget;
        let mut app = crate::test_support::app_for_tests();
        let backend = TestBackend::new(80, 1);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, f.area(), &app)).unwrap();

        // Locate each label in the drawn buffer, then hit-test its middle column.
        let buf = term.backend().buffer().clone();
        let row: String = (0..buf.area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        for ws in [
            Workspace::Perform,
            Workspace::Pattern,
            Workspace::Library,
            Workspace::Song,
            Workspace::Setup,
        ] {
            let x = row.find(ws.label()).expect("label drawn") as u16 + 2;
            let cell = app.hit_test(x, 0).expect("region registered");
            assert_eq!(cell.target, HitTarget::Workspace(ws), "at column {x}");
        }

        // End-to-end: press on PATTERN's columns switches the workspace.
        let x = row.find(Workspace::Pattern.label()).unwrap() as u16;
        let _ = app.mouse_press(x, 0, false);
        assert_eq!(app.workspace, Workspace::Pattern);
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
