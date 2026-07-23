//! Chain manager overlay: list chains, show entries, display live playback status.

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, HitCell, HitTarget};

/// Number of chain rows visible before scrolling.
const VISIBLE_CHAINS: usize = 10;
/// Number of entry rows visible before scrolling.
const VISIBLE_ENTRIES: usize = 8;

/// Render the chain manager overlay into `area`.
pub fn render_chain_view(f: &mut Frame, area: Rect, app: &App) {
    let chain_count = app.set.chains.len();
    let title = format!(" CHAINS  [{chain_count} chain(s)] ");
    let outer = Block::default().borders(Borders::ALL).title(title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::new();

    // Navigation hint.
    lines.push(Line::from(Span::raw(
        "[↑↓]chain [enter]play [c]create [r]rename [d]dup [x]del [m]loop [a]add-scene [X]rm-entry [[/]]bars [{/}]repeats [K/esc]close",
    )));

    // ── Live now-playing line ───────────────────────────────────────────────
    if let Some(pb) = &app.chain_playback {
        // Find the chain by id.
        let chain_opt = app.set.chains.iter().find(|c| c.id == pb.chain_id);
        let chain_name = chain_opt.map(|c| c.name.as_str()).unwrap_or("?");

        let entry_opt = chain_opt.and_then(|c| c.entries.get(pb.entry_idx));
        let scene_name = entry_opt
            .and_then(|e| app.set.scenes.iter().find(|s| s.id == e.scene_id))
            .map(|s| s.name.as_str())
            .unwrap_or("[MISSING]");

        let loop_tag = chain_opt.filter(|c| c.looped).map(|_| " ↻").unwrap_or("");
        let state_tag = if pb.active {
            "▶ PLAYING"
        } else {
            "⏸ QUEUED"
        };

        // Calculate bar progress within entry.
        let spb = app.set.steps_per_bar.max(1) as u64;
        let dwell = entry_opt
            .map(|e| e.dwell_steps(app.set.steps_per_bar))
            .unwrap_or(1)
            .max(1);
        let elapsed = (app.playhead as u64).saturating_sub(pb.entry_start_step);
        let bar_now = (elapsed / spb) + 1;
        let bar_total = dwell / 16;

        lines.push(Line::from(Span::styled(
            format!(
                "  {state_tag}  \"{chain_name}\"  entry {} → \"{scene_name}\"  bar {bar_now}/{bar_total}{loop_tag}",
                pb.entry_idx + 1,
            ),
            Style::default()
                .fg(EMBER.ok)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  ■ stopped",
            Style::default().add_modifier(Modifier::DIM),
        )));
    }

    lines.push(Line::from(Span::raw(
        "─────────────────────────────────────",
    )));

    // ── Chain list ─────────────────────────────────────────────────────────
    if chain_count == 0 {
        lines.push(Line::from(Span::raw("  (no chains — press [c] to create)")));
    } else {
        let scroll = app
            .chain_sel
            .saturating_sub(VISIBLE_CHAINS / 2)
            .min(chain_count.saturating_sub(VISIBLE_CHAINS));

        for (i, chain) in app
            .set
            .chains
            .iter()
            .enumerate()
            .skip(scroll)
            .take(VISIBLE_CHAINS)
        {
            let selected = i == app.chain_sel;
            let marker = if selected { "▸" } else { " " };
            let loop_tag = if chain.looped { " ↻" } else { "" };

            // Is this chain currently playing?
            let playing_tag = if app
                .chain_playback
                .as_ref()
                .is_some_and(|pb| pb.chain_id == chain.id)
            {
                " ▶"
            } else {
                ""
            };

            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            {
                // Feature: mouse hit region for this row (full inner width).
                let y = inner.y + lines.len() as u16;
                if y < inner.y + inner.height && inner.width > 0 {
                    app.hits.borrow_mut().push(HitCell {
                        x0: inner.x,
                        x1: inner.x + inner.width - 1,
                        y0: y,
                        y1: y,
                        target: HitTarget::ListRow(i),
                    });
                }
            }
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}{}{loop_tag}{playing_tag}  ({} entries)",
                    chain.name,
                    chain.entries.len()
                ),
                style,
            )));
        }

        lines.push(Line::from(Span::raw(format!(
            "chain {}/{}",
            app.chain_sel + 1,
            chain_count
        ))));

        // ── Entry list for selected chain ───────────────────────────────────
        if let Some(chain) = app.set.chains.get(app.chain_sel) {
            lines.push(Line::from(Span::styled(
                format!("── Entries: \"{}\" ──", chain.name),
                Style::default().add_modifier(Modifier::DIM),
            )));

            if chain.entries.is_empty() {
                lines.push(Line::from(Span::raw(
                    "  (no entries — press [a] to add a scene)",
                )));
            } else {
                let entry_count = chain.entries.len();
                let entry_scroll = app
                    .chain_entry_sel
                    .saturating_sub(VISIBLE_ENTRIES / 2)
                    .min(entry_count.saturating_sub(VISIBLE_ENTRIES));

                for (j, entry) in chain
                    .entries
                    .iter()
                    .enumerate()
                    .skip(entry_scroll)
                    .take(VISIBLE_ENTRIES)
                {
                    let sel = j == app.chain_entry_sel;
                    let e_marker = if sel { "  ▸" } else { "   " };

                    let scene_name = app
                        .set
                        .scenes
                        .iter()
                        .find(|s| s.id == entry.scene_id)
                        .map(|s| s.name.as_str())
                        .unwrap_or("[MISSING]");

                    // Is this entry currently playing?
                    let live_tag = if app
                        .chain_playback
                        .as_ref()
                        .is_some_and(|pb| pb.chain_id == chain.id && pb.entry_idx == j)
                    {
                        " ▶"
                    } else {
                        ""
                    };

                    let style = if sel {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    lines.push(Line::from(Span::styled(
                        format!(
                            "{e_marker}{j}: \"{scene_name}\"  {}bar × {}x{live_tag}",
                            entry.bars, entry.repeats,
                        ),
                        style,
                    )));
                }

                lines.push(Line::from(Span::raw(format!(
                    "   entry {}/{}",
                    app.chain_entry_sel + 1,
                    entry_count
                ))));
            }
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, App, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            records: Vec::new(),
            v2_index: Default::default(),
            families: Vec::new(),
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_chain_view(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn app_with_chain() -> App {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.apply(Action::CreateChain);
        app.mode = Mode::Chains;
        app
    }

    #[test]
    fn render_shows_chain_name() {
        let app = app_with_chain();
        let s = render_to_string(&app);
        assert!(s.contains("Chain 1"), "must show chain name; got: {s:?}");
    }

    #[test]
    fn render_shows_selection_marker() {
        let app = app_with_chain();
        let s = render_to_string(&app);
        assert!(s.contains('▸'), "must show ▸ selection marker; got: {s:?}");
    }

    #[test]
    fn render_empty_chains_does_not_panic() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Chains;
        let s = render_to_string(&app);
        assert!(
            s.contains("no chains") || s.contains("0 chain"),
            "empty chain list should indicate empty; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_stopped_state_when_no_playback() {
        let app = app_with_chain();
        let s = render_to_string(&app);
        assert!(
            s.contains("stopped"),
            "must show stopped when no chain is playing; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_nav_hint() {
        let app = app_with_chain();
        let s = render_to_string(&app);
        assert!(
            s.contains("create") || s.contains('['),
            "nav hint row must be present; got: {s:?}"
        );
    }

    #[test]
    fn render_entry_missing_scene_shows_missing_label() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.apply(Action::CreateChain);
        // Add an entry with a dangling scene_id (no matching scene exists).
        let bogus_id = crate::persist::mint_id();
        app.apply(Action::AddChainEntry {
            chain: 0,
            scene_id: bogus_id,
        });
        app.mode = Mode::Chains;
        let s = render_to_string(&app);
        assert!(
            s.contains("MISSING"),
            "dangling scene_id must render as [MISSING]; got: {s:?}"
        );
    }
}
