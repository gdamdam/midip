//! Library browser: genre → pattern → preview, three columns.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::pattern::library::{GenreMap, LibRole};
use crate::pattern::model::PatternData;

fn map_for_role<'a>(app: &'a App, role: LibRole) -> &'a GenreMap {
    match role {
        LibRole::Drums => &app.library.drums,
        LibRole::Bass => &app.library.bass,
        LibRole::Synth => &app.library.synth,
    }
}

fn role_filename(role: LibRole) -> &'static str {
    match role {
        LibRole::Drums => "patterns-t8-drums.json",
        LibRole::Bass => "patterns-t8-bass.json",
        LibRole::Synth => "patterns-s1.json",
    }
}

/// 16-cell preview of a pattern's first active voice / monophonic line.
fn preview(pattern: &crate::pattern::model::Pattern) -> String {
    let mut s = String::with_capacity(16);
    match &pattern.data {
        PatternData::Drums(steps) => {
            for i in 0..16 {
                let on = steps.get(i).map(|st| !st.is_empty()).unwrap_or(false);
                s.push(if on { '●' } else { '·' });
            }
        }
        PatternData::Melodic(steps) => {
            for i in 0..16 {
                let on = steps.get(i).map(|st| st.is_some()).unwrap_or(false);
                s.push(if on { '●' } else { '·' });
            }
        }
    }
    s
}

/// Render the library browser into `area`.
pub fn render_library(f: &mut Frame, area: Rect, app: &App) {
    let map = map_for_role(app, app.lib_role);

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(format!(" LIBRARY · {} ", role_filename(app.lib_role)));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(35),
            Constraint::Percentage(35),
        ])
        .split(inner);

    // Column 1: genre list with counts.
    let genres: Vec<(&String, &Vec<crate::pattern::model::Pattern>)> = map.iter().collect();
    let genre_lines: Vec<Line> = genres
        .iter()
        .enumerate()
        .map(|(i, (name, pats))| {
            let marker = if i == app.lib_genre { "▸" } else { " " };
            let style = if i == app.lib_genre {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!("{marker}{:<14}({:>2})", name, pats.len()),
                style,
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(genre_lines), cols[0]);

    // Column 2: pattern list for the selected genre.
    let selected_patterns: &[crate::pattern::model::Pattern] = genres
        .get(app.lib_genre)
        .map(|(_, pats)| pats.as_slice())
        .unwrap_or(&[]);
    let pattern_lines: Vec<Line> = selected_patterns
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let marker = if i == app.lib_pattern { "▸" } else { " " };
            let style = if i == app.lib_pattern {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!("{marker}{:02} {}", i + 1, p.name), style))
        })
        .collect();
    f.render_widget(Paragraph::new(pattern_lines), cols[1]);

    // Column 3: preview strip for the selected pattern + load hint.
    let mut preview_lines: Vec<Line> = Vec::new();
    if let Some(p) = selected_patterns.get(app.lib_pattern) {
        preview_lines.push(Line::from(Span::raw(preview(p))));
    }
    preview_lines.push(Line::from(Span::raw("[enter] load → focused lane")));
    f.render_widget(Paragraph::new(preview_lines), cols[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library, LibRole};
    use crate::pattern::model::{DrumHit, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn library_with_techno() -> Library {
        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "four-on-floor".to_string(),
            length: 16,
            data: PatternData::Drums(vec![vec![DrumHit { note: 36, vel: 127, prob: 1.0, ratchet: 1 }]; 16]),
        };
        drums.insert("techno".to_string(), vec![pat]);
        Library { drums, bass: GenreMap::new(), synth: GenreMap::new() }
    }

    #[test]
    fn library_shows_genre_and_selection_marker() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_techno());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let backend = TestBackend::new(92, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_library(f, f.area(), &app)).unwrap();

        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("techno"), "expected genre techno, got: {whole:?}");
        assert!(whole.contains('▸'), "expected selection marker, got: {whole:?}");
    }
}
