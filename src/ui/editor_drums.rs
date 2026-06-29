//! TR-style drum editor: voices (rows) × steps (columns).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::devices::profiles::drum_label;
use crate::pattern::model::PatternData;
use crate::ui::theme::{cursor_style, lane_color, playhead_style, vel_color, vel_glyph};

/// Velocity of a hit on `note` at `step`, or 0 if none.
fn hit_vel(steps: &[Vec<crate::pattern::model::DrumHit>], step: usize, note: u8) -> u8 {
    steps
        .get(step)
        .and_then(|s| s.iter().find(|h| h.note == note))
        .map(|h| h.vel)
        .unwrap_or(0)
}

/// The DrumHit on `note` at `step`, if any.
fn hit_at(
    steps: &[Vec<crate::pattern::model::DrumHit>],
    step: usize,
    note: u8,
) -> Option<&crate::pattern::model::DrumHit> {
    steps
        .get(step)
        .and_then(|s| s.iter().find(|h| h.note == note))
}

/// Combined style when cursor and playhead coincide: keep playhead bg, add cursor modifiers.
fn combined_cursor_playhead_style() -> Style {
    // Fall back to DarkGray if the theme ever drops the playhead bg, so we never panic.
    let bg = playhead_style()
        .bg
        .unwrap_or(ratatui::style::Color::DarkGray);
    cursor_style().bg(bg)
}

/// Render the drum editor into `area`.
pub fn render_drum_editor(f: &mut Frame, area: Rect, app: &App) {
    let lane = app.focused_lane();
    // During an audition the focused lane shows the cued (preview) pattern; otherwise
    // the committed lane pattern. The lane's profile/octave still drive layout.
    let pattern = app.display_pattern(app.focus);
    let voices = lane.profile.drum_voices;
    let len = pattern.step_count();

    // Polymeter: render the playhead at the focused lane's LOCAL step.
    let local_playhead = if len == 0 { 0 } else { app.playhead % len };
    let accent = lane_color(lane.profile.id);

    let steps: &[Vec<crate::pattern::model::DrumHit>] = match &pattern.data {
        PatternData::Drums(s) => s,
        PatternData::Melodic(_) => &[],
    };

    // Use app.visible_step_range() for the paged window.
    let (start, end) = app.visible_step_range();
    let visible_cols = start..end;

    let scroll_indicator = if len > 16 {
        format!("  steps {}-{}/{}", start + 1, end, len)
    } else {
        String::new()
    };

    let title = format!(
        " EDIT · {} · \"{}\" · {} steps · ch{}{} ",
        lane.profile.label,
        pattern.name,
        len,
        lane.profile.channel + 1,
        scroll_indicator,
    );

    let mut lines: Vec<Line> = Vec::with_capacity(voices.len() + 4);

    // EDIT header line (feature #2).
    let playhead_display = if len == 0 { 1 } else { app.playhead % len + 1 };
    let header_line = format!(
        "{} | Steps {}-{} of {} | Cursor {} | Playhead {}",
        app.context_label(),
        start + 1,
        end,
        len,
        app.cur_col + 1,
        playhead_display,
    );
    lines.push(Line::from(Span::raw(header_line)));

    // Step-number header row.
    {
        let mut spans: Vec<Span> = vec![Span::raw("        │ ")];
        for col in visible_cols.clone() {
            let label = if col % 4 == 0 {
                format!("{:<2}", col + 1)
            } else {
                "· ".to_string()
            };
            spans.push(Span::raw(label));
            if col % 4 == 3 {
                spans.push(Span::raw("│ "));
            }
        }
        lines.push(Line::from(spans));
    }

    for (ri, voice) in voices.iter().enumerate() {
        let focused_row = ri == app.cur_row;
        let marker = if focused_row { "▸" } else { " " };
        let voice_muted = lane.muted_voices.contains(&voice.note);
        let mute_marker = if voice_muted { "M" } else { " " };
        let label_style = if voice_muted {
            Style::default().fg(ratatui::style::Color::DarkGray)
        } else {
            Style::default()
        };
        let mut spans: Vec<Span> = vec![Span::styled(
            format!(
                "{marker}{mute_marker}{:<3} {:>2} │ ",
                voice.label, voice.note
            ),
            label_style,
        )];

        for col in visible_cols.clone() {
            let vel = hit_vel(steps, col, voice.note);
            let glyph = vel_glyph(vel).to_string();
            let is_cursor = focused_row && col == app.cur_col;
            let is_playhead = app.playing && col == local_playhead;

            // Feature #3: coincident cursor + playhead gets combined style.
            let style = if is_cursor && is_playhead {
                combined_cursor_playhead_style()
            } else if is_cursor {
                cursor_style()
            } else if is_playhead {
                playhead_style()
            } else if vel > 0 {
                Style::default().fg(vel_color(vel))
            } else {
                Style::default()
            };
            spans.push(Span::styled(glyph, style));
            spans.push(Span::raw(" "));
            if col % 4 == 3 {
                spans.push(Span::raw("│ "));
            }
        }
        lines.push(Line::from(spans));
    }

    // Euclid indicator for the focused voice.
    let focused_note = voices.get(app.cur_row).map(|v| v.note).unwrap_or(0);
    let pulses = steps
        .iter()
        .filter(|s| s.iter().any(|h| h.note == focused_note))
        .count();
    lines.push(Line::from(Span::raw(format!(
        "  E({},{}) r{}",
        pulses, len, app.euclid_rotation
    ))));

    // Feature #4: cursor detail line with exact format.
    let focused_voice_label = voices.get(app.cur_row).map(|v| v.label).unwrap_or("?");
    let detail = match hit_at(steps, app.cur_col, focused_note) {
        Some(h) => format!(
            "Step {} · {} · Velocity {} [-/+] · Probability {}% [p/P] · Ratchet x{} [y/Y]",
            app.cur_col + 1,
            drum_label(&lane.profile, focused_note),
            h.vel,
            (h.prob * 100.0).round() as i32,
            h.ratchet
        ),
        None => format!(
            "Step {} · {} · (empty)",
            app.cur_col + 1,
            focused_voice_label,
        ),
    };
    lines.push(Line::from(Span::raw(detail)));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(Style::default().fg(accent));
    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::{DrumHit, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn row_string(buf: &Buffer, area: Rect, y: u16) -> String {
        let mut s = String::new();
        for x in area.left()..area.right() {
            s.push_str(buf[(x, y)].symbol());
        }
        s
    }

    #[test]
    fn drum_editor_shows_voice_label_and_hit_glyph() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit {
            note: 36,
            vel: 127,
            prob: 1.0,
            ratchet: 1,
        }];
        set.lanes[0].pattern = Pattern {
            name: "test".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;

        let backend = TestBackend::new(92, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let whole: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            whole.contains("BD"),
            "expected BD voice label, got: {whole:?}"
        );

        let bd_row = (0..16)
            .map(|y| row_string(buf, area, y))
            .find(|r| r.contains("BD"))
            .expect("BD row");
        assert!(
            bd_row.contains('█'),
            "expected hit glyph on BD row: {bd_row:?}"
        );

        assert!(
            whole.contains("E("),
            "expected euclid indicator E(...), got: {whole:?}"
        );
        assert!(
            whole.contains("Probability"),
            "expected cursor detail Probability, got: {whole:?}"
        );
        assert!(
            whole.contains("Ratchet"),
            "expected cursor detail Ratchet, got: {whole:?}"
        );
    }

    #[test]
    fn drum_editor_shows_hit_at_step_20_after_scrolling() {
        use crate::app::Action;
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 32];
        steps[20] = vec![DrumHit {
            note: 36,
            vel: 127,
            prob: 1.0,
            ratchet: 1,
        }];
        set.lanes[0].pattern = Pattern {
            name: "test32".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.apply(Action::MoveCursor(0, 20));
        assert_eq!(app.cur_col, 20);
        assert!(app.step_scroll > 0, "scroll must have advanced");

        let backend = TestBackend::new(120, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 120, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let whole: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            whole.contains("steps "),
            "expected scroll indicator, got: {whole:?}"
        );
        assert!(
            whole.contains("21"),
            "expected step 21 label visible after scroll, got: {whole:?}"
        );
    }

    #[test]
    fn playhead_renders_at_local_step_under_polymeter() {
        use crate::ui::theme::playhead_style;
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.playing = true;
        app.playhead = 20;

        let backend = TestBackend::new(92, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let want_bg = playhead_style().bg.expect("playhead style has a bg");
        let highlighted = (0..16u16)
            .any(|y| (area.left()..area.right()).any(|x| buf[(x, y)].style().bg == Some(want_bg)));
        assert!(
            highlighted,
            "expected the local playhead column (4) highlighted under polymeter"
        );
    }

    #[test]
    fn drum_header_shows_steps_cursor_playhead() {
        use crate::app::Action;
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 32];
        steps[20] = vec![DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 1,
        }];
        set.lanes[0].pattern = Pattern {
            name: "hdr".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.apply(Action::MoveCursor(0, 20));
        app.playing = true;
        app.playhead = 24;
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 120, 20);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("Steps"), "header must contain 'Steps'");
        assert!(whole.contains("Cursor"), "header must contain 'Cursor'");
        assert!(whole.contains("Playhead"), "header must contain 'Playhead'");
        // cur_col=20 -> Cursor 21
        assert!(whole.contains("21"), "header must show cursor col 21");
    }

    #[test]
    fn drum_coincident_cursor_and_playhead_both_visible() {
        use crate::ui::theme::playhead_style;
        let mut set = Set::default_set(default_profiles());
        let steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        set.lanes[0].pattern = Pattern {
            name: "co".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.cur_col = 3;
        app.playing = true;
        app.playhead = 3; // 3 % 16 == 3 == cur_col
        let backend = TestBackend::new(92, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let buf = term.backend().buffer();
        let want_bg = playhead_style().bg.expect("playhead style has bg");
        let has_playhead_bg = (0..16u16)
            .any(|y| (area.left()..area.right()).any(|x| buf[(x, y)].style().bg == Some(want_bg)));
        assert!(
            has_playhead_bg,
            "coincident cursor+playhead must show playhead bg"
        );
    }

    #[test]
    fn drum_detail_line_contains_velocity_probability_ratchet() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit {
            note: 36,
            vel: 80,
            prob: 0.75,
            ratchet: 2,
        }];
        set.lanes[0].pattern = Pattern {
            name: "det".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.cur_col = 0;
        let backend = TestBackend::new(120, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 120, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("Velocity"), "detail must contain 'Velocity'");
        assert!(
            whole.contains("Probability"),
            "detail must contain 'Probability'"
        );
        assert!(whole.contains("Ratchet"), "detail must contain 'Ratchet'");
    }

    /// §2.6: a muted voice row shows the 'M' marker in its label column.
    #[test]
    fn drum_editor_shows_muted_voice_marker() {
        use crate::devices::profiles::DRUM_VOICES;

        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.cur_row = 0;
        app.cur_col = 0;

        // Mute the first voice (DRUM_VOICES[0]).
        let muted_note = DRUM_VOICES[0].note;
        app.set.lanes[0].muted_voices = vec![muted_note];

        let backend = TestBackend::new(120, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 120, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains('M'),
            "muted voice row must show 'M' marker; got: {whole:?}"
        );
    }
}
