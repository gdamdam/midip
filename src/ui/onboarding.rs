//! First-run onboarding walkthrough overlay (Phase-2 Task 9).
//!
//! A stepped tour of the five workspaces, the command palette and help.
//! `Enter`/`→` advances, `Esc` dismisses; both routes end in
//! `store::mark_onboarded` so the tour auto-shows only once.

use crate::app::{App, ONBOARDING_STEPS};
use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

/// (key, title, body) for each tour page. Length must equal `ONBOARDING_STEPS`.
const STEPS: [(&str, &str, &str); ONBOARDING_STEPS] = [
    (
        "F1",
        "PERFORM",
        "Your live surface. Launch patterns, mute and solo lanes, drop fills — \
         everything here is quantized so nothing ever lands off the grid.",
    ),
    (
        "F2",
        "PATTERN",
        "The step editor. Program drum grids and melodic lines, tweak velocity, \
         probability, ratchets and micro-timing per step.",
    ),
    (
        "F3",
        "LIBRARY",
        "Browse the pattern library, audition before loading, mark favorites \
         and organize crates for the gig.",
    ),
    (
        "F4",
        "SONG",
        "Scenes capture the whole set's state; chains string scenes into an \
         arrangement you can play back or jump around in.",
    ),
    (
        "F5",
        "SETUP",
        "Route lanes to MIDI ports and channels, pick devices, and configure \
         clock in/out and Ableton Link.",
    ),
    (
        ": / ctrl+p",
        "COMMAND PALETTE",
        "Every command, searchable by name. When you forget a key, type what \
         you want instead.",
    ),
    (
        "?",
        "HELP",
        "The essentials on one card; press tab inside help for the full \
         keybinding reference. That's the tour — go make some noise.",
    ),
];

/// Render the walkthrough page for `app.onboarding_step` into `area`.
pub fn render_onboarding(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    let step = app.onboarding_step.min(ONBOARDING_STEPS - 1);
    let (key, title, body) = STEPS[step];

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" WELCOME  {}/{} ", step + 1, ONBOARDING_STEPS),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("  [{key}]  "),
                Style::default().fg(EMBER.ok).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title,
                Style::default()
                    .fg(EMBER.synth)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    lines.push(Line::from(format!("  {body}")));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        if step + 1 == ONBOARDING_STEPS {
            "  [enter] finish   [esc] close".to_string()
        } else {
            "  [enter/→] next   [esc] skip tour".to_string()
        },
        Style::default().fg(EMBER.dim),
    )));

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Overlay;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_step_to_string(step: usize) -> String {
        let mut app = crate::test_support::app_for_tests();
        app.open_overlay(Overlay::Onboarding);
        app.onboarding_step = step;
        let backend = TestBackend::new(70, 14);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_onboarding(f, f.area(), &app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn first_step_tours_perform_workspace() {
        let s = render_step_to_string(0);
        assert!(s.contains("1/7"), "step counter missing: {s:?}");
        assert!(s.contains("PERFORM"), "expected PERFORM, got: {s:?}");
        assert!(s.contains("skip tour"), "dismiss hint missing: {s:?}");
    }

    #[test]
    fn last_step_covers_help_and_offers_finish() {
        let s = render_step_to_string(ONBOARDING_STEPS - 1);
        assert!(s.contains("7/7"), "step counter missing: {s:?}");
        assert!(s.contains("HELP"), "expected HELP, got: {s:?}");
        assert!(s.contains("finish"), "finish hint missing: {s:?}");
    }

    #[test]
    fn out_of_range_step_clamps_instead_of_panicking() {
        let s = render_step_to_string(999);
        assert!(s.contains("7/7"));
    }
}
