//! Scene manager overlay: list scenes, show selected detail, allow capture/recall/CRUD.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, HitCell, HitTarget};

/// Number of scene rows visible before scrolling.
const VISIBLE: usize = 12;

/// Render the scene manager overlay into `area`.
pub fn render_scene_view(f: &mut Frame, area: Rect, app: &App) {
    let count = app.set.scenes.len();
    let title = format!(" SCENES  [{count} scene(s)] ");
    let outer = Block::default().borders(Borders::ALL).title(title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::new();

    // Navigation hint.
    lines.push(Line::from(Span::raw(
        "[↑↓]select [enter]recall [c]capture [r]rename [d]dup [x]delete [z]validate [C]cancel-q [G/esc]close",
    )));

    if count == 0 {
        lines.push(Line::from(Span::raw(
            "  (no scenes — press [c] to capture)",
        )));
    } else {
        let scroll = app
            .scene_sel
            .saturating_sub(VISIBLE / 2)
            .min(count.saturating_sub(VISIBLE));

        for (i, scene) in app.set.scenes.iter().enumerate().skip(scroll).take(VISIBLE) {
            let selected = i == app.scene_sel;
            let marker = if selected { "▸" } else { " " };

            // Compact lane summary: pattern display names separated by " | ".
            let summary: String = scene
                .assignments
                .iter()
                .map(|a| a.pattern.display_name())
                .collect::<Vec<_>>()
                .join(" | ");

            // QUEUED marker: any lane involved in this scene is queued.
            let queued_tag = if selected {
                let any_queued = scene
                    .assignments
                    .iter()
                    .enumerate()
                    .any(|(lane, _)| app.queued.get(lane).and_then(|q| q.as_deref()).is_some());
                if any_queued {
                    "  QUEUED\u{27F6}"
                } else {
                    ""
                }
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
                format!("{marker}{}{queued_tag}", scene.name),
                style,
            )));
            if selected && !summary.is_empty() {
                lines.push(Line::from(Span::raw(format!("   {summary}"))));
            }
        }

        // Position indicator.
        lines.push(Line::from(Span::raw(format!(
            "{}/{}",
            app.scene_sel + 1,
            count
        ))));

        // Selected scene detail.
        if let Some(scene) = app.set.scenes.get(app.scene_sel) {
            lines.push(Line::from(Span::styled(
                "── Assignment detail ──",
                Style::default().add_modifier(Modifier::DIM),
            )));
            for (i, a) in scene.assignments.iter().enumerate() {
                let name = a.pattern.display_name();
                let mut flags = String::new();
                if a.mute {
                    flags.push_str(" mute");
                }
                if a.solo {
                    flags.push_str(" solo");
                }
                if a.transpose != 0 {
                    flags.push_str(&format!(" xp{:+}", a.transpose));
                }
                if a.octave != 0 {
                    flags.push_str(&format!(" oct{:+}", a.octave));
                }
                let missing = if app.scene_issues.contains(&i) {
                    " [MISSING]"
                } else {
                    ""
                };
                lines.push(Line::from(Span::raw(format!(
                    "  L{i}: {name}{flags}{missing}"
                ))));
            }
        }

        // Validation issues summary.
        if !app.scene_issues.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("── {} missing assignment(s) ──", app.scene_issues.len()),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
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
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_scene_view(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn app_with_scenes() -> App {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let scene = app.set.capture_scene("My Scene".to_string());
        app.set.scenes.push(scene);
        app.mode = Mode::Scenes;
        app
    }

    #[test]
    fn render_shows_scene_name() {
        let app = app_with_scenes();
        let s = render_to_string(&app);
        assert!(s.contains("My Scene"), "must show scene name; got: {s:?}");
    }

    #[test]
    fn render_shows_selection_marker() {
        let app = app_with_scenes();
        let s = render_to_string(&app);
        assert!(s.contains('▸'), "must show ▸ selection marker; got: {s:?}");
    }

    #[test]
    fn render_empty_scenes_does_not_panic() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.mode = Mode::Scenes;
        let s = render_to_string(&app);
        // Should mention "no scenes" or something similar.
        assert!(
            s.contains("no scenes") || s.contains("0 scene"),
            "empty scene list should indicate empty; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_queued_marker() {
        let mut app = app_with_scenes();
        // Simulate a queued lane.
        if !app.queued.is_empty() {
            app.queued[0] = Some("pattern".to_string());
        }
        let s = render_to_string(&app);
        // QUEUED marker or ⟶ should appear when lane is queued.
        assert!(
            s.contains("QUEUED") || s.contains('\u{27F6}'),
            "queued lane must show QUEUED marker; got: {s:?}"
        );
    }

    #[test]
    fn render_shows_nav_hint() {
        let app = app_with_scenes();
        let s = render_to_string(&app);
        assert!(
            s.contains("capture") || s.contains('['),
            "nav hint row must be present; got: {s:?}"
        );
    }

    #[test]
    fn render_two_scenes_shows_both() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let s1 = app.set.capture_scene("Alpha".to_string());
        let s2 = app.set.capture_scene("Beta".to_string());
        app.set.scenes.push(s1);
        app.set.scenes.push(s2);
        app.mode = Mode::Scenes;
        let s = render_to_string(&app);
        assert!(s.contains("Alpha"), "must show first scene; got: {s:?}");
        assert!(s.contains("Beta"), "must show second scene; got: {s:?}");
    }

    #[test]
    fn render_shows_missing_label_after_validate() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let scene = app.set.capture_scene("Test".to_string());
        app.set.scenes.push(scene);
        // Inject a missing issue directly.
        app.scene_issues = vec![0];
        app.mode = Mode::Scenes;
        let s = render_to_string(&app);
        assert!(
            s.contains("MISSING") || s.contains("missing"),
            "must show MISSING for bad lane; got: {s:?}"
        );
    }

    #[test]
    fn render_scenes_title_shows_count() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        let scene = app.set.capture_scene("One".to_string());
        app.set.scenes.push(scene);
        app.mode = Mode::Scenes;
        let s = render_to_string(&app);
        assert!(
            s.contains("1 scene") || s.contains("SCENES"),
            "title must show scene count; got: {s:?}"
        );
    }
}
