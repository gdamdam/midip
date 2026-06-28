//! Melodic editor: monophonic note/length/velocity step lane (bass, synth).

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::devices::profiles::{melodic_velocity, resolve_melodic_pitch};
use crate::pattern::model::{MelodicNote, PatternData};
use crate::ui::theme::{cursor_style, lane_color, note_name, playhead_style, vel_bar, vel_color};

/// Combined style when cursor and playhead coincide: keep playhead bg, add cursor modifiers.
fn combined_cursor_playhead_style() -> Style {
    // Fall back to DarkGray if the theme ever drops the playhead bg, so we never panic.
    let bg = playhead_style()
        .bg
        .unwrap_or(ratatui::style::Color::DarkGray);
    cursor_style().bg(bg)
}

/// Render the melodic editor into `area`.
pub fn render_melodic_editor(f: &mut Frame, area: Rect, app: &App) {
    let lane = app.focused_lane();
    // During an audition the focused lane shows the cued (preview) pattern; otherwise
    // the committed lane pattern. The lane's profile/octave/transpose still drive layout.
    let pattern = app.display_pattern(app.focus);
    let len = pattern.step_count();
    let root = lane.profile.root_note;

    // Polymeter: render the playhead at the focused lane's LOCAL step.
    let local_playhead = if len == 0 { 0 } else { app.playhead % len };
    let accent = lane_color(lane.profile.id);

    let steps: &[Option<MelodicNote>] = match &pattern.data {
        PatternData::Melodic(s) => s,
        PatternData::Drums(_) => &[],
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
        " EDIT · {} · \"{}\" · {} steps · ch{} · root {}{} ",
        lane.profile.label,
        pattern.name,
        len,
        lane.profile.channel + 1,
        note_name(root),
        scroll_indicator,
    );

    // Feature #2: EDIT header line.
    let playhead_display = if len == 0 { 1 } else { app.playhead % len + 1 };
    let header_line = format!(
        "{} | Steps {}-{} of {} | Cursor {} | Playhead {} | Oct {:+} | Transpose {:+} st",
        app.context_label(),
        start + 1,
        end,
        len,
        app.cur_col + 1,
        playhead_display,
        lane.octave,
        lane.transpose,
    );

    let mut step_spans: Vec<Span> = vec![Span::raw("step ")];
    let mut note_spans: Vec<Span> = vec![Span::raw("note ")];
    let mut len_spans: Vec<Span> = vec![Span::raw("len  ")];
    let mut vel_spans: Vec<Span> = vec![Span::raw("vel  ")];

    // Feature #5: non-lossy length — track active sustain across columns.
    // sustain_end is the exclusive end (absolute step index) of the current sustain.
    let mut sustain_end: usize = 0;
    // Feature #6: track the previous column's note so a slide can draw a glide tie
    // that bridges INTO it from the note on its left, not just a marker on its own cell.
    let mut prev_was_note = false;

    for col in visible_cols {
        let is_cursor = col == app.cur_col;
        let is_playhead = app.playing && col == local_playhead;

        // Feature #3: combined style when both coincide.
        let cell_style = if is_cursor && is_playhead {
            combined_cursor_playhead_style()
        } else if is_cursor {
            cursor_style()
        } else if is_playhead {
            playhead_style()
        } else {
            Style::default()
        };

        step_spans.push(Span::raw(format!("{:<4}", col + 1)));

        match steps.get(col).and_then(|s| s.as_ref()) {
            Some(note) => {
                let pitch = resolve_melodic_pitch(root, note.semi, lane.transpose, lane.octave);
                // Feature #6: a slide note draws a glide tie. When the previous step also
                // held a note, lead the cell with a connecting line (`─╴`) so the eye reads
                // a bridge FROM the left note INTO this one; otherwise keep the `~` marker.
                let label = if note.slide {
                    if prev_was_note {
                        format!("─╴{:<2}", note_name(pitch))
                    } else {
                        format!("~{:<3}", note_name(pitch))
                    }
                } else {
                    format!("{:<4}", note_name(pitch))
                };
                note_spans.push(Span::styled(label, cell_style));

                // Feature #5/#6: len row. A slide leads with a connector reaching left
                // (`──▶`) so the glide tie is visible on the length lane too.
                let note_head = if note.len >= 1.0 {
                    // Set sustain for columns after this one.
                    sustain_end = col + note.len.floor() as usize;
                    if note.slide {
                        "──▶ ".to_string()
                    } else {
                        "▶   ".to_string()
                    }
                } else {
                    // Sub-step: partial head, no sustain from this note.
                    // Don't extend sustain_end.
                    if note.slide {
                        "──▷ ".to_string()
                    } else {
                        "▷   ".to_string()
                    }
                };
                len_spans.push(Span::raw(note_head));

                let mv = melodic_velocity(note.vel);
                let bar = vel_bar(mv).to_string();
                vel_spans.push(Span::styled(
                    format!("{bar:<4}"),
                    Style::default().fg(vel_color(mv)),
                ));
                prev_was_note = true;
            }
            None => {
                note_spans.push(Span::styled(format!("{:<4}", "·"), cell_style));

                // Feature #5: sustain continuation or empty.
                if col < sustain_end {
                    len_spans.push(Span::raw("────"));
                } else {
                    len_spans.push(Span::raw("    "));
                }

                vel_spans.push(Span::raw(format!("{:<4}", "·")));
                prev_was_note = false;
            }
        }
    }

    // Feature #4: cursor detail line with exact format.
    let detail = match steps.get(app.cur_col).and_then(|s| s.as_ref()) {
        Some(note) => {
            let pitch = resolve_melodic_pitch(root, note.semi, lane.transpose, lane.octave);
            let slide_indicator = if note.slide { " · ~slide" } else { "" };
            format!(
                "Step {} · {} · vel {:.2} · len {:.1}{} · Probability {}% [p/P] · Ratchet x{} [y/Y]",
                app.cur_col + 1,
                note_name(pitch),
                note.vel,
                note.len,
                slide_indicator,
                (note.prob * 100.0).round() as i32,
                note.ratchet
            )
        }
        None => format!("Step {} · (rest)", app.cur_col + 1),
    };

    let lines = vec![
        Line::from(Span::raw(header_line)),
        Line::from(step_spans),
        Line::from(note_spans),
        Line::from(len_spans),
        Line::from(vel_spans),
        Line::from(Span::raw(detail)),
    ];

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
    use crate::pattern::model::{MelodicNote, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    #[test]
    fn melodic_editor_shows_note_at_step_20_after_scrolling() {
        use crate::app::Action;
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 32];
        steps[20] = Some(MelodicNote {
            semi: 5,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "test32".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.apply(Action::MoveCursor(0, 20));
        assert_eq!(app.cur_col, 20);
        assert!(app.step_scroll > 0, "scroll must have advanced");

        let backend = TestBackend::new(120, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();

        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains("steps "),
            "expected scroll indicator, got: {whole:?}"
        );
        assert!(
            whole.contains("21"),
            "expected step 21 visible after scroll, got: {whole:?}"
        );
    }

    #[test]
    fn melodic_editor_shows_note_name_and_slide_marker() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[0] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        steps[4] = Some(MelodicNote {
            semi: 7,
            vel: 1.0,
            slide: true,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "test".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 0;

        let backend = TestBackend::new(92, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();

        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains("A2"),
            "expected note name A2, got: {whole:?}"
        );
        assert!(
            whole.contains('~'),
            "expected slide marker ~, got: {whole:?}"
        );
        assert!(
            whole.contains("Probability"),
            "expected detail Probability, got: {whole:?}"
        );
        assert!(
            whole.contains("Ratchet"),
            "expected detail Ratchet, got: {whole:?}"
        );
    }

    #[test]
    fn melodic_header_shows_steps_cursor_playhead() {
        use crate::app::Action;
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 32];
        steps[20] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "hdr".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.apply(Action::MoveCursor(0, 20));
        app.playing = true;
        app.playhead = 24;
        let backend = TestBackend::new(120, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
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
    }

    #[test]
    fn melodic_coincident_cursor_and_playhead_both_visible() {
        use crate::ui::theme::playhead_style;
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[3] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "co".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 3;
        app.playing = true;
        app.playhead = 3; // 3 % 16 == 3 == cur_col
        let backend = TestBackend::new(92, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
        let buf = term.backend().buffer();
        let want_bg = playhead_style().bg.expect("playhead style has bg");
        let has_playhead_bg =
            (0..10u16).any(|y| (0..92u16).any(|x| buf[(x, y)].style().bg == Some(want_bg)));
        assert!(
            has_playhead_bg,
            "coincident cursor+playhead must show playhead bg"
        );
    }

    #[test]
    fn melodic_length_non_lossy() {
        let mut set1 = Set::default_set(default_profiles());
        let mut steps1: Vec<Option<MelodicNote>> = vec![None; 16];
        steps1[0] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set1.lanes[1].pattern = Pattern {
            name: "short".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps1),
            id: crate::persist::Id::nil(),
        };
        let mut app1 = App::new(set1, empty_library());
        app1.focus = 1;
        let backend1 = TestBackend::new(120, 8);
        let mut term1 = Terminal::new(backend1).unwrap();
        term1
            .draw(|f| render_melodic_editor(f, f.area(), &app1))
            .unwrap();
        let whole1: String = term1
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        let mut set2 = Set::default_set(default_profiles());
        let mut steps2: Vec<Option<MelodicNote>> = vec![None; 16];
        steps2[0] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 8.0,
            prob: 1.0,
            ratchet: 1,
        });
        set2.lanes[1].pattern = Pattern {
            name: "long".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps2),
            id: crate::persist::Id::nil(),
        };
        let mut app2 = App::new(set2, empty_library());
        app2.focus = 1;
        let backend2 = TestBackend::new(120, 8);
        let mut term2 = Terminal::new(backend2).unwrap();
        term2
            .draw(|f| render_melodic_editor(f, f.area(), &app2))
            .unwrap();
        let whole2: String = term2
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        assert_ne!(whole1, whole2, "len=1 and len=8 must render differently");
    }

    #[test]
    fn melodic_slide_shows_connector_between_adjacent_notes() {
        // Two adjacent notes (step 0, step 1 slide) must render a CONNECTOR bridging
        // them — a glide tie reaching from the left note into the slide note — not just
        // a lone `~` marker on the slide cell.
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[0] = Some(MelodicNote {
            semi: 0,
            vel: 1.0,
            slide: false,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        steps[1] = Some(MelodicNote {
            semi: 2,
            vel: 1.0,
            slide: true,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "sl".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 1;
        let backend = TestBackend::new(120, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // The connecting line glyph bridges the two adjacent notes (distinct from `~`).
        assert!(
            whole.contains('─'),
            "adjacent slide must render a connecting line ─ between the notes, got: {whole:?}"
        );
        assert!(
            whole.contains('╴'),
            "slide note row must lead with a left-joining tie ╴, got: {whole:?}"
        );
    }

    #[test]
    fn melodic_lone_slide_falls_back_to_tilde_marker() {
        // A slide note with NO preceding note still gets the `~` slide marker (no left
        // neighbour to connect to).
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[2] = Some(MelodicNote {
            semi: 2,
            vel: 1.0,
            slide: true,
            len: 1.0,
            prob: 1.0,
            ratchet: 1,
        });
        set.lanes[1].pattern = Pattern {
            name: "lone".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 2;
        let backend = TestBackend::new(120, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains('~'),
            "lone slide note must show ~ marker, got: {whole:?}"
        );
    }

    #[test]
    fn melodic_header_shows_octave_and_transpose() {
        let mut set = Set::default_set(default_profiles());
        set.lanes[1].pattern = Pattern {
            name: "pitch".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(vec![None; 16]),
            id: crate::persist::Id::nil(),
        };
        // Set non-zero octave and transpose so the signed display is unambiguous.
        set.lanes[1].octave = 2;
        set.lanes[1].transpose = -3;
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        let backend = TestBackend::new(120, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains("Oct"),
            "header must contain 'Oct', got: {whole:?}"
        );
        assert!(
            whole.contains("+2"),
            "header must show octave +2, got: {whole:?}"
        );
        assert!(
            whole.contains("Transpose"),
            "header must contain 'Transpose', got: {whole:?}"
        );
        assert!(
            whole.contains("-3"),
            "header must show transpose -3 st, got: {whole:?}"
        );
    }

    #[test]
    fn melodic_detail_contains_prob_and_ratchet() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[0] = Some(MelodicNote {
            semi: 5,
            vel: 0.8,
            slide: false,
            len: 2.0,
            prob: 0.5,
            ratchet: 3,
        });
        set.lanes[1].pattern = Pattern {
            name: "det".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
            id: crate::persist::Id::nil(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 0;
        let backend = TestBackend::new(120, 10);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app))
            .unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains("Probability"),
            "detail must contain 'Probability'"
        );
        assert!(whole.contains("Ratchet"), "detail must contain 'Ratchet'");
    }
}
