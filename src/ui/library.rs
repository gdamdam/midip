//! Library browser: genre → pattern → preview, three columns.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, HitCell, HitTarget, LibCol};
use crate::devices::profiles::{drum_label, resolve_melodic_pitch, T8_DRUMS};
use crate::pattern::library::{GenreMap, LibRole};
#[cfg(test)]
use crate::pattern::model::TrigCond;
use crate::pattern::model::{Pattern, PatternData};
use crate::ui::theme;

fn map_for_role(app: &App, role: LibRole) -> &GenreMap {
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
    &s[..s[end..]
        .char_indices()
        .nth(1)
        .map(|(j, _)| end + j)
        .unwrap_or(s.len())]
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
        lines.push(Line::from(Span::raw(
            truncate(&pattern.desc, w).to_string(),
        )));
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
                    let hit = steps
                        .get(i)
                        .and_then(|st| st.iter().find(|h| h.note == *note));
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
                // Mono preview: read the step's primary note for the on/off+slide strip.
                match steps.get(i).and_then(|s| s.first()) {
                    Some(n) if n.slide => strip.push('~'),
                    Some(_) => strip.push('●'),
                    _ => strip.push('·'),
                }
            }
            lines.push(Line::from(Span::raw(strip)));

            // Note names for active steps (root=45, semi from step, no transpose/octave).
            let notes: Vec<String> = steps
                .iter()
                .filter_map(|s| s.first())
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

/// Build the `PatternRef` for an entry in a genre map at a given index.
/// Returns `None` if the index is out of bounds.
fn pattern_ref_for_entry(
    app: &App,
    genre: &str,
    pat: &crate::pattern::model::Pattern,
) -> crate::pattern::refs::PatternRef {
    // Delegate to the shared accessor so the star display, the favorites filter,
    // and selection/actions all resolve refs through one code path.
    app.pattern_ref_for(genre, pat)
}

/// Render the library browser into `area`.
pub fn render_library(f: &mut Frame, area: Rect, app: &App) {
    let map = map_for_role(app, app.lib_role);

    let genre_focused = app.lib_col == LibCol::Genre;
    let genre_title = if genre_focused {
        " ▸GENRE "
    } else {
        " GENRE "
    };
    let pattern_title = if app.fav_filter {
        if !genre_focused {
            " ▸PATTERN ★only "
        } else {
            " PATTERN ★only "
        }
    } else if !genre_focused {
        " ▸PATTERN "
    } else {
        " PATTERN "
    };

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
    let genre_scroll = app
        .lib_genre
        .saturating_sub(VISIBLE_HEIGHT / 2)
        .min(genre_total.saturating_sub(VISIBLE_HEIGHT));
    let mut genre_lines: Vec<Line> = Vec::new();
    genre_lines.push(Line::from(Span::styled(
        genre_title,
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (i, (name, pats)) in genres
        .iter()
        .enumerate()
        .skip(genre_scroll)
        .take(VISIBLE_HEIGHT)
    {
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
        genre_lines.push(Line::from(Span::raw(format!(
            "{}/{}",
            app.lib_genre + 1,
            genre_total
        ))));
    }
    f.render_widget(Paragraph::new(genre_lines), cols[0]);

    // Column 2: pattern list for the selected genre.
    // When fav_filter is on, only show patterns that are in favorites.
    let selected_genre_name: &str = genres
        .get(app.lib_genre)
        .map(|(name, _)| name.as_str())
        .unwrap_or("");

    // Build the filtered view: (original_index, pattern) pairs.
    // Use the shared accessor so render and actions index the SAME filtered list.
    let visible_patterns: Vec<(usize, &crate::pattern::model::Pattern)> =
        app.visible_lib_patterns();

    let pat_total = visible_patterns.len();
    // lib_pattern indexes into visible_patterns (the filtered list) everywhere.
    let pat_scroll = app
        .lib_pattern
        .saturating_sub(VISIBLE_HEIGHT / 2)
        .min(pat_total.saturating_sub(VISIBLE_HEIGHT));
    let mut pattern_lines: Vec<Line> = Vec::new();
    pattern_lines.push(Line::from(Span::styled(
        pattern_title,
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (display_i, (orig_i, p)) in visible_patterns
        .iter()
        .enumerate()
        .skip(pat_scroll)
        .take(VISIBLE_HEIGHT)
    {
        let marker = if display_i == app.lib_pattern {
            "▸"
        } else {
            " "
        };
        let style = if display_i == app.lib_pattern {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let r = pattern_ref_for_entry(app, selected_genre_name, p);
        let star = if app.favorites.contains(&r) {
            "\u{2605}"
        } else {
            " "
        };
        {
            // Feature: mouse hit region for this pattern row (pattern column only;
            // clicking selects the pattern and moves column focus there).
            let y = cols[1].y + pattern_lines.len() as u16;
            if y < cols[1].y + cols[1].height && cols[1].width > 0 {
                app.hits.borrow_mut().push(HitCell {
                    x0: cols[1].x,
                    x1: cols[1].x + cols[1].width - 1,
                    y0: y,
                    y1: y,
                    target: HitTarget::ListRow(display_i),
                });
            }
        }
        pattern_lines.push(Line::from(Span::styled(
            format!("{marker}{star}{:02} {}", orig_i + 1, p.name),
            style,
        )));
    }
    if pat_total > 0 {
        pattern_lines.push(Line::from(Span::raw(format!(
            "{}/{}",
            app.lib_pattern + 1,
            pat_total
        ))));
    }
    f.render_widget(Paragraph::new(pattern_lines), cols[1]);

    // Resolve the selected pattern from the visible (possibly filtered) list.
    let selected_pattern_in_col2: Option<&crate::pattern::model::Pattern> =
        visible_patterns.get(app.lib_pattern).map(|(_, p)| *p);

    // Column 3: detailed preview for the selected pattern + audition badge + load hint.
    let preview_width = cols[2].width as usize;
    let preview_height = cols[2].height as usize;
    let auditioning = app.audition.is_some();
    // Reserve lines: 1 for the hint (or 2 when auditioning for the badge).
    let reserved = if auditioning { 2 } else { 1 };
    let mut preview_lines: Vec<Line> = Vec::new();
    if let Some(p) = selected_pattern_in_col2 {
        let max_h = preview_height.saturating_sub(reserved);
        preview_lines.extend(build_preview_lines(p, preview_width, max_h));
    }
    if auditioning {
        preview_lines.push(Line::from(Span::styled(
            "[ AUDITION ]",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED),
        )));
        preview_lines.push(Line::from(Span::raw("[enter] keep  [esc] revert")));
    } else {
        preview_lines.push(Line::from(Span::raw(
            "[a] audition  [enter] load → focused lane",
        )));
    }
    f.render_widget(Paragraph::new(preview_lines), cols[2]);
}

/// Render the saved-set browser into `area`.
pub fn render_set_browser(f: &mut Frame, area: Rect, app: &App) {
    let outer = Block::default().borders(Borders::ALL).title(" OPEN SET ");
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines: Vec<Line> = Vec::new();

    if app.set_files.is_empty() {
        lines.push(Line::from(Span::raw("No saved sets — press s to save")));
    } else {
        let total = app.set_files.len();
        let scroll = app
            .set_sel
            .saturating_sub(VISIBLE_HEIGHT / 2)
            .min(total.saturating_sub(VISIBLE_HEIGHT));

        for (i, path) in app
            .set_files
            .iter()
            .enumerate()
            .skip(scroll)
            .take(VISIBLE_HEIGHT)
        {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            let marker = if i == app.set_sel { "▸" } else { " " };
            let style = if i == app.set_sel {
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
            lines.push(Line::from(Span::styled(format!("{marker}{}", stem), style)));
        }
        lines.push(Line::from(Span::raw(format!(
            "{}/{}",
            app.set_sel + 1,
            total
        ))));
    }
    lines.push(Line::from(Span::raw(
        "[enter]load  [r]rename  [a/S]save-as  [D]duplicate  [d]delete  [n]new  [esc/o]cancel",
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, LibRole, Library};
    use crate::pattern::model::{DrumHit, MelodicNote, MelodicStep, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn library_with_drums() -> Library {
        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "Four on Floor".to_string(),
            desc: "Classic 4/4 kick, snare on 2&4, 8th closed hats".to_string(),
            length: 16,
            data: PatternData::Drums(vec![
                vec![DrumHit {
                    note: 36,
                    vel: 127,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }];
                16
            ]),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        drums.insert("techno".to_string(), vec![pat]);
        Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn library_with_melodic() -> Library {
        let mut synth = GenreMap::new();
        let steps = vec![
            MelodicStep::from(vec![MelodicNote {
                semi: 0,
                vel: 1.0,
                slide: false,
                len: 0.9,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }]),
            MelodicStep::default(),
            MelodicStep::from(vec![MelodicNote {
                semi: 7,
                vel: 1.0,
                slide: true,
                len: 0.9,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }]),
        ];
        let pat = Pattern {
            name: "Iron Grid".to_string(),
            desc: "8th-note root pulse".to_string(),
            length: 3,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        synth.insert("techno".to_string(), vec![pat]);
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth,
        }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_library(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn library_shows_genre_and_selection_marker() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        assert!(
            whole.contains("techno"),
            "expected genre techno in: {whole:?}"
        );
        assert!(
            whole.contains('▸'),
            "expected selection marker in: {whole:?}"
        );
    }

    #[test]
    fn preview_shows_drum_pattern_name_desc_and_voice_label() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        let whole = render_to_string(&app);
        assert!(
            whole.contains("Four on Floor"),
            "expected pattern name in: {whole:?}"
        );
        assert!(
            whole.contains("Classic"),
            "expected desc text in: {whole:?}"
        );
        assert!(
            whole.contains("BD"),
            "expected BD voice label in: {whole:?}"
        );
    }

    fn render_set_browser_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_set_browser(f, f.area(), app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
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
        assert!(
            whole.contains("1/1"),
            "expected position indicator '1/1' in: {whole:?}"
        );
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
        assert!(
            whole.contains("1/2"),
            "expected position indicator '1/2' in: {whole:?}"
        );
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
        assert!(
            whole.contains("Iron Grid"),
            "expected pattern name in: {whole:?}"
        );
        assert!(
            whole.contains("8th-note"),
            "expected desc text in: {whole:?}"
        );
        // root=45 (A2), semi=0 → A2; semi=7 → E3
        assert!(whole.contains("A2"), "expected note name A2 in: {whole:?}");
    }

    // ── M4a Task 3: favorites UI tests ────────────────────────────────────────

    #[test]
    fn render_library_shows_star_for_favorited_pattern() {
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::Favorites;

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library_with_drums());
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        // Favorite the "Four on Floor" pattern in techno.
        let r = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "Four on Floor".to_string(),
        };
        let mut favs = Favorites::default();
        favs.toggle(r);
        app.favorites = favs;

        let whole = render_to_string(&app);
        assert!(
            whole.contains('\u{2605}'),
            "favorited pattern must show ★ in library browser: {whole:?}"
        );
    }

    #[test]
    fn render_library_fav_filter_shows_only_favorites() {
        use crate::pattern::model::{DrumHit, Pattern, PatternData};
        use crate::pattern::refs::PatternRef;
        use crate::pattern::store::Favorites;

        // Build a library with two patterns in techno: only "Four on Floor" favorited.
        let mut drums = GenreMap::new();
        let pat1 = Pattern {
            name: "Four on Floor".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![
                vec![DrumHit {
                    note: 36,
                    vel: 127,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }];
                16
            ]),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        let pat2 = Pattern {
            name: "Off Beat".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![Vec::new(); 16]),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        drums.insert("techno".to_string(), vec![pat1, pat2]);
        let library = Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        };

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library);
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        // Favorite only "Four on Floor".
        let r = PatternRef::Vendored {
            role: "drums".to_string(),
            genre: "techno".to_string(),
            name: "Four on Floor".to_string(),
        };
        let mut favs = Favorites::default();
        favs.toggle(r);
        app.favorites = favs;
        app.fav_filter = true;

        let whole = render_to_string(&app);
        // Favorited pattern must appear.
        assert!(
            whole.contains("Four on Floor"),
            "favorited pattern must appear when fav_filter on: {whole:?}"
        );
        // Non-favorited pattern must NOT appear.
        assert!(
            !whole.contains("Off Beat"),
            "non-favorited pattern must be hidden when fav_filter on: {whole:?}"
        );
        // Filter indicator must appear.
        assert!(
            whole.contains("\u{2605}only") || whole.contains("★only"),
            "fav_filter indicator must appear in library title: {whole:?}"
        );
    }

    #[test]
    fn render_library_shows_audition_badge_when_auditioning() {
        use crate::pattern::model::{DrumHit, Pattern, PatternData};

        let mut drums = GenreMap::new();
        let pat = Pattern {
            name: "Test Beat".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![
                vec![DrumHit {
                    note: 36,
                    vel: 100,
                    prob: 1.0,
                    ratchet: 1,
                    micro: 0,
                    cond: TrigCond::Always,
                }];
                16
            ]),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        drums.insert("techno".to_string(), vec![pat]);
        let library = Library {
            drums,
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        };

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, library);
        app.lib_role = LibRole::Drums;
        app.lib_genre = 0;
        app.lib_pattern = 0;

        // Without audition: no badge, shows the standard hint.
        let whole = render_to_string(&app);
        assert!(
            !whole.contains("AUDITION"),
            "no badge before audition: {whole:?}"
        );
        assert!(
            whole.contains("[a] audition"),
            "standard hint before audition: {whole:?}"
        );

        // Simulate audition active by setting the isolated preview directly.
        use crate::pattern::model::PatternData as PD;
        app.audition = Some(crate::app::AuditionPreview {
            lane: 0,
            pattern: crate::pattern::model::Pattern {
                name: "preview".to_string(),
                desc: String::new(),
                length: 16,
                data: PD::Drums(vec![Vec::new(); 16]),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
        });

        let whole = render_to_string(&app);
        assert!(
            whole.contains("AUDITION"),
            "badge must appear when auditioning: {whole:?}"
        );
        assert!(
            whole.contains("[enter] keep"),
            "keep hint must appear: {whole:?}"
        );
        assert!(
            whole.contains("[esc] revert"),
            "revert hint must appear: {whole:?}"
        );
    }
}
