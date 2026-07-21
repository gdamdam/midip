//! Live crate browser overlay: browse a crate's pattern entries and launch them
//! to role-matched lanes (quantized) without committing the active Set.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, CrateIssue, HitCell, HitTarget};
use crate::pattern::refs::resolve_pattern_ref;

/// Number of entry rows visible before scrolling.
const VISIBLE: usize = 14;

/// Render the crate browser overlay into `area`.
pub fn render_crate_view(f: &mut Frame, area: Rect, app: &App) {
    let user_dir = crate::config::data_dir().join("patterns");

    // Crate header: name and ←/→ hint when multiple crates exist.
    let crate_count = app.crates.crates.len();
    let crate_name = app
        .crates
        .crates
        .get(app.crate_sel)
        .map(|c| c.name.as_str())
        .unwrap_or("(no crates)");
    let title = if crate_count > 1 {
        format!(
            " CRATE [{}/{}] {} ",
            app.crate_sel + 1,
            crate_count,
            crate_name
        )
    } else {
        format!(" CRATE  {} ", crate_name)
    };

    let outer = Block::default().borders(Borders::ALL).title(title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::new();

    // Navigation hint row.
    lines.push(Line::from(Span::raw(
        "[↑↓]select [enter]launch [a]audition [f]fav [C]cancel-q [←→]crate [z]validate [V/esc]close",
    )));

    let entries = app
        .crates
        .crates
        .get(app.crate_sel)
        .map(|c| c.entries.as_slice())
        .unwrap_or(&[]);

    if entries.is_empty() {
        lines.push(Line::from(Span::raw("  (empty crate)")));
    } else {
        let total = entries.len();
        let scroll = app
            .crate_entry_sel
            .saturating_sub(VISIBLE / 2)
            .min(total.saturating_sub(VISIBLE));

        for (i, entry) in entries.iter().enumerate().skip(scroll).take(VISIBLE) {
            let selected = i == app.crate_entry_sel;
            let marker = if selected { "▸" } else { " " };
            let star = if app.favorites.contains(&entry.pattern) {
                "\u{2605}"
            } else {
                " "
            };

            // Check resolvability.
            let resolvable = resolve_pattern_ref(&entry.pattern, &app.library, &user_dir).is_some();
            let missing_tag = if resolvable { "" } else { " [MISSING]" };

            // Queued badge: show if this entry's role-matched lane is queued.
            let queued_tag = if let Some(hint) = entry.pattern.role_lane_hint() {
                if app.queued.get(hint).and_then(|q| q.as_deref()).is_some() {
                    " [Q]"
                } else {
                    ""
                }
            } else {
                ""
            };

            let display = entry
                .label
                .clone()
                .unwrap_or_else(|| entry.pattern.display_name());

            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else if !resolvable {
                Style::default().add_modifier(Modifier::DIM)
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
                format!("{marker}{star}{display}{missing_tag}{queued_tag}"),
                style,
            )));
        }

        // Position indicator.
        lines.push(Line::from(Span::raw(format!(
            "{}/{}",
            app.crate_entry_sel + 1,
            total
        ))));
    }

    // Validation results (shown after [z] validate is run; hidden when empty).
    if !app.crate_issues.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("── {} issue(s) ──", app.crate_issues.len()),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for issue in &app.crate_issues {
            let text = match issue {
                CrateIssue::MissingPattern { entry_idx, name } => {
                    format!("  [{}] missing: {}", entry_idx + 1, name)
                }
                CrateIssue::UnavailableTarget { entry_idx, lane } => {
                    format!("  [{}] lane {} disconnected", entry_idx + 1, lane)
                }
            };
            lines.push(Line::from(Span::styled(
                text,
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, CrateIssue, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use crate::pattern::refs::PatternRef;
    use crate::pattern::store::{CrateEntry, Favorites};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(100, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_crate_view(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn app_with_crate() -> App {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let idx = app.crates.add_crate("My Crate".to_string());
        app.crates.add_entry(
            idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "Four on Floor".to_string(),
                },
                label: None,
            },
        );
        app.crates.add_entry(
            idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "bass".to_string(),
                    genre: "techno".to_string(),
                    name: "Bass Line".to_string(),
                },
                label: None,
            },
        );
        app.mode = Mode::CrateView;
        app
    }

    #[test]
    fn render_shows_crate_name() {
        let app = app_with_crate();
        let s = render_to_string(&app);
        assert!(s.contains("My Crate"), "must show crate name; got: {s:?}");
    }

    #[test]
    fn render_shows_entries() {
        let app = app_with_crate();
        let s = render_to_string(&app);
        assert!(
            s.contains("Four on Floor"),
            "must show entry name; got: {s:?}"
        );
        assert!(
            s.contains("Bass Line"),
            "must show second entry; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_selection_marker() {
        let mut app = app_with_crate();
        app.crate_entry_sel = 0;
        let s = render_to_string(&app);
        assert!(s.contains('▸'), "must show selection marker; got: {s:?}");
    }

    #[test]
    fn render_shows_star_for_favorited_entry() {
        let mut app = app_with_crate();
        let r = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "Four on Floor".to_string(),
        };
        let mut favs = Favorites::default();
        favs.toggle(r);
        app.favorites = favs;
        let s = render_to_string(&app);
        assert!(
            s.contains('\u{2605}'),
            "favorited entry must show ★; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_missing_for_unresolvable_entry() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let idx = app.crates.add_crate("Crate".to_string());
        app.crates.add_entry(
            idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "Ghost".to_string(),
                },
                label: None,
            },
        );
        app.mode = Mode::CrateView;
        let s = render_to_string(&app);
        assert!(
            s.to_lowercase().contains("missing") || s.contains("[!]"),
            "unresolvable entry must show MISSING indicator; got: {s:?}"
        );
    }

    #[test]
    fn render_empty_crate_index_does_not_panic() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::CrateView;
        let s = render_to_string(&app);
        let _ = s;
    }

    #[test]
    fn render_shows_validation_issues_after_validate() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let idx = app.crates.add_crate("Validate Test".to_string());
        app.crates.add_entry(
            idx,
            CrateEntry {
                pattern: PatternRef::Vendored {
                    role: "drums".to_string(),
                    genre: "techno".to_string(),
                    name: "Ghost Pattern".to_string(),
                },
                label: None,
            },
        );
        // Inject a MissingPattern issue directly (simulate post-validate state).
        app.crate_issues = vec![CrateIssue::MissingPattern {
            entry_idx: 0,
            name: "Ghost Pattern".to_string(),
        }];
        app.mode = Mode::CrateView;
        let s = render_to_string(&app);
        assert!(
            s.contains("issue") || s.contains("missing"),
            "must show validation issues; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_validate_hint_in_nav() {
        let app = app_with_crate();
        let s = render_to_string(&app);
        assert!(
            s.contains('z') || s.contains("validate"),
            "nav hint must mention validate key; got: {s:?}"
        );
    }
}
