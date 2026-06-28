//! Library browser: genre → pattern → preview, three columns.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, LibCol};
use crate::devices::profiles::{drum_label, resolve_melodic_pitch, T8_DRUMS};
use crate::pattern::library::{GenreMap, LibRole};
use crate::pattern::model::{Pattern, PatternData};
use crate::ui::theme;

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

/// Truncate a string to at most `max_chars` characters (char boundary safe).
fn truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let mut end = 0;
    for (i, _) in s.char_indices().take(max_chars) {
        end = i;
    }
    // advance past the last counted char
    &s[..s[end..].char_indices().nth(1).map(|(j, _)| end + j).unwrap_or(s.len())]
}

/// Build detailed preview lines for the selected pattern.
/// `width` is the available column width; `max_height` caps voice rows for drums.
fn build_preview_lines(pattern: &Pattern, width: usize, max_height: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let w = width.max(4);

    // Name
    lines.push(Line::from(Span::styled(
        truncate(&pattern.name, w).to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // Desc (truncate to width)
    if !pattern.desc.is_empty() {
        lines.push(Line::from(Span::raw(truncate(&pattern.desc, w).to_string())));
    }

    // Length
    lines.push(Line::from(Span::raw(format!("len:{}", pattern.length))));

    match &pattern.data {
        PatternData::Drums(steps) => {
            // Collect which notes appear in the pattern.
            let mut seen_notes: Vec<u8> = Vec::new();
            for step in steps {
                for hit in step {
                    if !seen_notes.contains(&hit.note) {
                        seen_notes.push(hit.note);
                    }
                }
            }
            // Sort by DRUM_VOICES order, then by note number for unknowns.
            seen_notes.sort_by_key(|&n| {
                T8_DRUMS
                    .drum_voices
                    .iter()
                    .position(|v| v.note == n)
                    .unwrap_or(usize::MAX)
            });

            let strip_len = pattern.length.min(16);
            // Cap voice rows to available height (leave room for name/desc/len lines already added).
            let rows_remaining = max_height.saturating_sub(lines.len());
            for note in seen_notes.iter().take(rows_remaining) {
                let label = drum_label(&T8_DRUMS, *note);
                let mut strip = String::new();
                for i in 0..strip_len {
                    let hit = steps.get(i).and_then(|st| st.iter().find(|h| h.note == *note));
                    if let Some(h) = hit {
                        strip.push(theme::vel_glyph(h.vel));
                    } else {
                        strip.push('·');
                    }
                }
                lines.push(Line::from(Span::raw(format!(
                    "{:<3}{}",
                    truncate(&label, 3),
                    strip
                ))));
            }
        }
        PatternData::Melodic(steps) => {
            // On/off+slide strip.
            let strip_len = pattern.length.min(16);
            let mut strip = String::new();
            for i in 0..strip_len {
                match steps.get(i) {
                    Some(Some(n)) if n.slide => strip.push('~'),
                    Some(Some(_)) => strip.push('●'),
                    _ => strip.push('·'),
                }
            }
            lines.push(Line::from(Span::raw(strip)));

            // Note names for active steps (root=45, semi from step, no transpose/octave).
            let notes: Vec<String> = steps
                .iter()
                .filter_map(|s| s.as_ref())
                .map(|n| theme::note_name(resolve_melodic_pitch(45, n.semi, 0, 0)))
                .collect();
            // Fit as many as available width allows (space-separated).
            let mut note_str = String::new();
            for name in &notes {
                if note_str.len() + name.len() + 1 > w {
                    break;
                }
                if !note_str.is_empty() {
                    note_str.push(' ');
                }
                note_str.push_str(name);
            }
            if !note_str.is_empty() {
                lines.push(Line::from(Span::raw(note_str)));
            }
        }
    }

    lines
}

/// Number of items visible in a genre/pattern list at once before scrolling.
const VISIBLE_HEIGHT: usize = 12;

/// Render the library browser into `area`.
pub fn render_library(f: &mut Frame, area: Rect, app: &App) {
    let map = map_for_role(app, app.lib_role);

    let genre_focused = app.lib_col == LibCol::Genre;
    let genre_title = if genre_focused { " ▸GENRE " } else { " GENRE " };
    let pattern_title = if !genre_focused { " ▸PATTERN " } else { " PATTERN " };

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

    // Column 1: genre list with counts + scroll window + position indicator.
    let genres: Vec<(&String, &Vec<crate::pattern::model::Pattern>)> = map.iter().collect();
    let genre_total = genres.len();
    let genre_scroll = app.lib_genre.saturating_sub(VISIBLE_HEIGHT / 2)
        .min(genre_total.saturating_sub(VISIBLE_HEIGHT));
    let mut genre_lines: Vec<Line> = Vec::new();
    genre_lines.push(Line::from(Span::styled(genre_title, Style::default().add_modifier(Modifier::BOLD))));
    for (i, (name, pats)) in genres.iter().enumerate().skip(genre_scroll).take(VISIBLE_HEIGHT) {
        let marker = if i == app.lib_genre { "▸" } else { " " };
        let style = if i == app.lib_genre {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        genre_lines.push(Line::from(Span::styled(
            format!("{marker}{:<14}({:>2})", name, pats.len()),
            style,
        )));
    }
    if genre_total > 0 {
        genre_lines.push(Line::from(Span::raw(format!("{}/{}", app.lib_genre + 1, genre_total))));
    }
    f.render_widget(Paragraph::new(genre_lines), cols[0]);

    // Column 2: pattern list for the selected genre + scroll window + position indicator.
    let selected_patterns: &[crate::pattern::model::Pattern] = genres
        .get(app.lib_genre)
        .map(|(_, pats)| pats.as_slice())
        .unwrap_or(&[]);
    let pat_total = selected_patterns.len();
    let pat_scroll = app.lib_pattern.saturating_sub(VISIBLE_HEIGHT / 2)
        .min(pat_total.saturating_sub(VISIBLE_HEIGHT));
    let mut pattern_lines: Vec<Line> = Vec::new();
    pattern_lines.push(Line::from(Span::styled(pattern_title, Style::default().add_modifier(Modifier::BOLD))));
    for (i, p) in selected_patterns.iter().enumerate().skip(pat_scroll).take(VISIBLE_HEIGHT) {
        let marker = if i == app.lib_pattern { "▸" } else { " " };
        let style = if i == app.lib_pattern {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        pattern_lines.push(Line::from(Span::styled(format!("{marker}{:02} {}", i + 1, p.name), style)));
    }
    if pat_total > 0 {
        pattern_lines.push(Line::from(Span::raw(format!("{}/{}", app.lib_pattern + 1, pat_total))));
    }
    f.render_widget(Paragraph::new(pattern_lines), cols[1]);

    // Column 3: detailed preview for the selected pattern + audition badge + load hint.
    let preview_width = cols[2].width as usize;
    let preview_height = cols[2].height as usize;
    let auditioning = app.audition.is_some();
    // Reserve lines: 1 for the hint (or 2 when auditioning for the badge).
    let reserved = if auditioning { 2 } else { 1 };
    let mut preview_lines: Vec<Line> = Vec::new();
    if let Some(p) = selected_patterns.get(app.lib_pattern) {
        let max_h = preview_height.saturating_sub(reserved);
        preview_lines.extend(build_preview_lines(p, preview_width, max_h));
    }
    if auditioning {
        preview_lines.push(Line::from(Span::styled(
            "[ AUDITION ]",
            Style::default().add_modifier(Modifier::BOLD).add_modifier(Modifier::REVERSED),
        )));
        preview_lines.push(Line::from(Span::raw("[enter] keep  [esc] revert")));
    } else {
        preview_lines.push(Line::from(Span::raw("[a] audition  [enter] load → focused lane")));
    }
    f.render_widget(Paragraph::new(preview_lines), cols[2]);
}

/// Render the saved-set browser into `area`.
pub fn render_set_browser(f: &mut Frame, area: Rect, app: &App) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" OPEN SET ");
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::new();

    if app.set_files.is_empty() {
        lines.push(Line::from(Span::raw("No saved sets — press s to save")));
    } else {
        let total = app.set_files.len();
        let scroll = app.set_sel.saturating_sub(VISIBLE_HEIGHT / 2)
            .min(total.saturating_sub(VISIBLE_HEIGHT));

        for (i, path) in app.set_files.iter().enumerate().skip(scroll).take(VISIBLE_HEIGHT) {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            let marker = if i == app.set_sel { "▸" } else { " " };
            let style = if i == app.set_sel {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(format!("{marker}{}", stem), style)));
        }
        lines.push(Line::from(Span::raw(format!("{}/{}", app.set_sel + 1, total))));
    }
    lines.push(Line::from(Span::raw("[enter] load  [esc/o] cancel")));
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library, LibRole};
    use crate::pattern::model::{DrumHit, MelodicNote, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn library_with_drums() -> Library {
        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "Four on Floor".to_string(),
            desc: "Classic 4/4 kick, snare on 2&4, 8th closed hats".to_string(),
            length: 16,
            data: PatternData::Drums(vec![vec![DrumHit { note: 36, vel: 127, prob: 1.0, ratchet: 1 }]; 16]),
        };
        drums.insert("techno".to_string(), vec![pat]);
        Library { drums, bass: GenreMap::new(), synth: GenreMap::new() }
    }

    fn library_with_melodic() -> Library {
        let mut synth = GenreMap::new();
        let steps = vec![
            Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 0.9, prob: 1.0, ratchet: 1 }),
            None,
            Some(MelodicNote { semi: 7, vel: 1.0, slide: true, len: 0.9, prob: 1.0, ratchet: 1 }),
        ];
        let pat = Pattern {
            name: "Iron Grid".to_string(),
            desc: "8th-note root pulse".to_string(),
            length: 3,
            data: PatternData::Melodic(steps),
        };
        synth.insert("techno".to_string(), vec![pat]);
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_library(f, f.area(), app)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn library_shows_genre_and_selection_marker() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        assert!(whole.contains("techno"), "expected genre techno in: {whole:?}");
        assert!(whole.contains('▸'), "expected selection marker in: {whole:?}");
    }

    #[test]
    fn preview_shows_drum_pattern_name_desc_and_voice_label() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        assert!(whole.contains("Four on Floor"), "expected pattern name in: {whole:?}");
        assert!(whole.contains("Classic"), "expected desc text in: {whole:?}");
        assert!(whole.contains("BD"), "expected BD voice label in: {whole:?}");
    }

    fn render_set_browser_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_set_browser(f, f.area(), app)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn render_library_shows_position_indicator() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        // With 1 genre and 1 pattern, position indicators "1/1" should appear.
        assert!(whole.contains("1/1"), "expected position indicator '1/1' in: {whole:?}");
    }

    #[test]
    fn render_set_browser_shows_filename_and_indicator() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.set_files = vec![
            std::path::PathBuf::from("/tmp/my-set.json"),
            std::path::PathBuf::from("/tmp/another.json"),
        ];
        app.set_sel = 0;

        let whole = render_set_browser_to_string(&app);
        assert!(whole.contains("my-set"), "expected file stem in: {whole:?}");
        assert!(whole.contains("1/2"), "expected position indicator '1/2' in: {whole:?}");
    }

    #[test]
    fn render_set_browser_empty_state() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, library_with_drums());
        // set_files is empty by default

        let whole = render_set_browser_to_string(&app);
        assert!(
            whole.contains("No saved sets"),
            "expected empty-state message in: {whole:?}"
        );
    }

    #[test]
    fn preview_shows_melodic_pattern_name_desc_and_note_names() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_melodic());
        app.lib_role = LibRole::Synth;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        assert!(whole.contains("Iron Grid"), "expected pattern name in: {whole:?}");
        assert!(whole.contains("8th-note"), "expected desc text in: {whole:?}");
        // root=45 (A2), semi=0 → A2; semi=7 → E3
        assert!(whole.contains("A2"), "expected note name A2 in: {whole:?}");
    }

    #[test]
    fn render_library_shows_audition_badge_when_auditioning() {
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "Test Beat".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![vec![DrumHit { note: 36, vel: 100, prob: 1.0, ratchet: 1 }]; 16]),
        };
        drums.insert("techno".to_string(), vec![pat]);
        let library = Library { drums, bass: GenreMap::new(), synth: GenreMap::new() };

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library);
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        // Without audition: no badge, shows the standard hint.
        let whole = render_to_string(&app);
        assert!(!whole.contains("AUDITION"), "no badge before audition: {whole:?}");
        assert!(whole.contains("[a] audition"), "standard hint before audition: {whole:?}");

        // Simulate audition active by setting the field directly.
        use crate::pattern::model::PatternData as PD;
        app.audition = Some(crate::pattern::model::Pattern {
            name: "original".to_string(),
            desc: String::new(),
            length: 16,
            data: PD::Drums(vec![Vec::new(); 16]),
        });

        let whole = render_to_string(&app);
        assert!(whole.contains("AUDITION"), "badge must appear when auditioning: {whole:?}");
        assert!(whole.contains("[enter] keep"), "keep hint must appear: {whole:?}");
        assert!(whole.contains("[esc] revert"), "revert hint must appear: {whole:?}");
    }
}
