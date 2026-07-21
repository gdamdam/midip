//! Command palette overlay: a global fuzzy finder over the command registry.
//!
//! Opened with `:` from any bare workspace (never while an overlay is raised,
//! so text-entry contexts keep their keys) or Ctrl+P from anywhere. Lists ALL
//! registry commands — the per-command `workspace` field is hint metadata, not
//! a palette filter — and running one re-dispatches its `Action` through
//! `App::apply`, so a palette run behaves exactly like the bound keypress.

use crate::app::App;
use crate::commands::{self, Command};
use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Fixed right-column width for accelerator labels (longest is "Ctrl+P").
const ACCEL_COL: usize = 7;

/// Case-insensitive fuzzy filter over `items`, ranked for the palette:
/// prefix matches first, then substring matches, then scattered-subsequence
/// matches; registry (declaration) order is preserved within each rank.
/// An empty query returns ALL items — the palette is a global finder.
pub fn fuzzy_filter<'a>(items: &'a [Command], q: &str) -> Vec<&'a Command> {
    let q = q.to_lowercase();
    if q.is_empty() {
        return items.iter().collect();
    }
    let mut ranked: Vec<(u8, &Command)> = items
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            if name.starts_with(&q) {
                Some((0u8, c))
            } else if name.contains(&q) {
                Some((1, c))
            } else if is_subsequence(&q, &name) {
                Some((2, c))
            } else {
                None
            }
        })
        .collect();
    // Stable sort: within a rank, registry declaration order is kept.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, c)| c).collect()
}

/// True if every char of `needle` appears in `hay` in order (not contiguously).
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay_chars = hay.chars();
    needle
        .chars()
        .all(|nc| hay_chars.by_ref().any(|hc| hc == nc))
}

/// Render the command palette overlay: a query line followed by the filtered
/// command rows — `Command.name` left, `Command.accel` right-aligned, selected
/// row highlighted. Scrolls to keep the selection visible.
pub fn render_command_palette(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " COMMAND PALETTE ",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let hits = fuzzy_filter(commands::registry(), &app.palette_query);
    // Defensive clamp: `apply` resets palette_sel on query edits, but render
    // must never index past the filtered list.
    let sel = app.palette_sel.min(hits.len().saturating_sub(1));

    let mut lines: Vec<Line> = Vec::with_capacity(hits.len() + 2);
    lines.push(Line::from(vec![
        Span::styled(
            "> ",
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.palette_query.clone(), Style::default().fg(EMBER.fg)),
        Span::styled("▏", Style::default().fg(EMBER.synth)),
    ]));

    if hits.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no matching commands)",
            Style::default().fg(EMBER.dim),
        )));
    }

    // Rows visible under the query line; scroll so the selection stays in view.
    let visible = (inner.height.saturating_sub(1) as usize).max(1);
    let start = sel.saturating_sub(visible - 1);
    let accel_w = ACCEL_COL;
    let name_w = (inner.width as usize).saturating_sub(2 + accel_w).max(1);

    for (i, cmd) in hits.iter().enumerate().skip(start).take(visible) {
        let selected = i == sel;
        let marker = if selected { "▸ " } else { "  " };
        if selected {
            lines.push(Line::from(Span::styled(
                format!("{marker}{:<name_w$}{:>accel_w$}", cmd.name, cmd.accel),
                Style::default()
                    .fg(EMBER.bg)
                    .bg(EMBER.synth)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::raw(format!("{marker}{:<name_w$}", cmd.name)),
                Span::styled(
                    format!("{:>accel_w$}", cmd.accel),
                    Style::default().fg(EMBER.dim),
                ),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::registry;

    #[test]
    fn fuzzy_filter_ranks_prefix_then_subsequence() {
        let cmds = registry();
        let hits = fuzzy_filter(cmds, "libr");
        assert!(
            hits.first()
                .map(|c| c.name.contains("ibrary"))
                .unwrap_or(false),
            "top hit for 'libr' must be a library command, got {:?}",
            hits.first().map(|c| c.name)
        );
    }

    #[test]
    fn fuzzy_filter_empty_query_returns_all() {
        let cmds = registry();
        assert_eq!(fuzzy_filter(cmds, "").len(), cmds.len());
    }

    #[test]
    fn fuzzy_filter_prefix_beats_subsequence() {
        let cmds = registry();
        let hits = fuzzy_filter(cmds, "open");
        // Every prefix match ("Open …") must precede any subsequence-only hit.
        let first_non_prefix = hits
            .iter()
            .position(|c| !c.name.to_lowercase().starts_with("open"));
        if let Some(cut) = first_non_prefix {
            assert!(
                hits[cut..]
                    .iter()
                    .all(|c| !c.name.to_lowercase().starts_with("open")),
                "prefix matches must all rank above non-prefix matches"
            );
            assert!(cut > 0, "'open' has prefix matches; one must be first");
        }
    }

    #[test]
    fn fuzzy_filter_is_case_insensitive() {
        let cmds = registry();
        let lower = fuzzy_filter(cmds, "libr");
        let upper = fuzzy_filter(cmds, "LIBR");
        assert!(!lower.is_empty());
        assert_eq!(lower.first().map(|c| c.name), upper.first().map(|c| c.name));
    }

    #[test]
    fn fuzzy_filter_no_match_returns_empty() {
        let cmds = registry();
        assert!(fuzzy_filter(cmds, "zzzzqqq").is_empty());
    }

    #[test]
    fn render_shows_command_names_and_accels() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = crate::test_support::app_for_tests();
        app.apply(crate::app::Action::OpenPalette);
        let backend = TestBackend::new(80, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_command_palette(f, f.area(), &app))
            .unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("COMMAND PALETTE"));
        assert!(whole.contains("Play / stop"), "row shows Command.name");
        assert!(whole.contains("Space"), "row shows Command.accel");
    }
}
