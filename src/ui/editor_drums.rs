//! TR-style drum editor: voices (rows) × steps (columns).

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, HitCell, HitTarget};
use crate::devices::profiles::drum_label;
use crate::pattern::model::{PatternData, TrigCond};
use crate::ui::theme::{
    cursor_style, lane_color, playhead_style, step_attr_marker, vel_bar, vel_color, vel_glyph,
};

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
    // Fall back to EMBER.dim if the theme ever drops the playhead bg, so we never panic.
    let bg = playhead_style().bg.unwrap_or(EMBER.dim);
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

    // Feature #1: rebuild the mouse hit-map for this frame. Geometry below MUST match
    // the span widths emitted while drawing, so clicks land on the cell they point at.
    // Content starts at area.x+1 / area.y+1 (inside the border). Each voice-row label
    // prefix is 11 cells wide; voice rows begin at content line index 2 (after the EDIT
    // header + step-number row). Each step cell is 2 cells (glyph + marker), plus a
    // "│ " bar separator after every 4th step.
    // Cleared once per frame in `ui::render`; here we only append this pane's cells.
    let mut hits = app.hits.borrow_mut();
    let grid_x0 = area.x + 1 + 11;

    // Use app.visible_step_range() for the paged window.
    let (start, end) = app.visible_step_range();
    let visible_cols = start..end;

    // M15: vertical voice-row window. At small terminal sizes not every voice row
    // fits in the pane; page the rows around the cursor (same idiom as the
    // horizontal step paging) so the cursor can never sit on an invisible row.
    // Fixed chrome: 2 border rows + EDIT header + step numbers + accent + euclid + detail.
    const CHROME_ROWS: usize = 7;
    let max_rows = (area.height as usize).saturating_sub(CHROME_ROWS).max(1);
    let (row_start, row_end) = if voices.len() <= max_rows {
        (0, voices.len())
    } else {
        let rs = ((app.cur_row / max_rows) * max_rows).min(voices.len().saturating_sub(1));
        (rs, (rs + max_rows).min(voices.len()))
    };

    let scroll_indicator = if len > 16 {
        format!("  steps {}-{}/{}", start + 1, end, len)
    } else {
        String::new()
    };
    let row_indicator = if row_end - row_start < voices.len() {
        format!("  voices {}-{}/{}", row_start + 1, row_end, voices.len())
    } else {
        String::new()
    };

    let lane_extras = {
        let mut extras = String::new();
        if let Some(sw) = lane.swing {
            extras.push_str(&format!(" sw{:.2}", sw));
        }
        if let Some(d) = lane.clock_div {
            extras.push_str(&format!(" /{}", d));
        }
        extras
    };
    let title = format!(
        " EDIT · {} · \"{}\" · {} steps · ch{}{}{}{} ",
        lane.profile.label,
        pattern.name,
        len,
        lane.profile.channel + 1,
        lane_extras,
        scroll_indicator,
        row_indicator,
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

    for (ri, voice) in voices.iter().enumerate().take(row_end).skip(row_start) {
        let focused_row = ri == app.cur_row;
        let marker = if focused_row { "▸" } else { " " };
        let voice_muted = lane.muted_voices.contains(&voice.note);
        let mute_marker = if voice_muted { "M" } else { " " };
        let label_style = if voice_muted {
            Style::default().fg(EMBER.dim)
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

        // Absolute y of this voice row (content line index 2 + its offset in the window).
        let y = area.y + 1 + 2 + (ri - row_start) as u16;
        let mut x = grid_x0;
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

            // Feature #2: the cell's trailing separator carries the step's most salient
            // generative attribute (ratchet / probability / cond / microtiming), so it's
            // visible at rest instead of only in the cursor detail line. Plain hits and
            // empty cells keep a blank separator, so default patterns look unchanged.
            let sep = if vel > 0 {
                hit_at(steps, col, voice.note)
                    .map(|h| {
                        step_attr_marker(h.prob, h.ratchet, h.micro, h.cond == TrigCond::Always)
                    })
                    .unwrap_or(' ')
            } else {
                ' '
            };
            if sep != ' ' {
                spans.push(Span::styled(
                    sep.to_string(),
                    Style::default().fg(EMBER.dim),
                ));
            } else {
                spans.push(Span::raw(" "));
            }

            // Feature #1: record the clickable region (glyph + separator = 2 cells).
            hits.push(HitCell {
                x0: x,
                x1: x + 1,
                y0: y,
                y1: y,
                target: HitTarget::Step {
                    row: ri,
                    col,
                    is_drums: true,
                },
            });
            x += 2;
            if col % 4 == 3 {
                spans.push(Span::raw("│ "));
                x += 2;
            }
        }
        lines.push(Line::from(spans));
    }

    // Feature #3: accent histogram — per visible step, the loudest hit across all voices
    // as an 8-level bar, so the groove's dynamic shape reads at a glance. Empty steps
    // stay blank (no phantom baseline bar). Prefix width matches the voice-row grid so
    // the bars align under the step columns.
    {
        let mut spans: Vec<Span> = vec![Span::raw(format!("{:<9}│ ", "acc"))];
        for col in visible_cols.clone() {
            let acc = steps
                .get(col)
                .map(|s| s.iter().map(|h| h.vel).max().unwrap_or(0))
                .unwrap_or(0);
            let glyph = if acc == 0 {
                ' '.to_string()
            } else {
                vel_bar(acc).to_string()
            };
            spans.push(Span::styled(glyph, Style::default().fg(vel_color(acc))));
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
        Some(h) => {
            let micro_str = if h.micro != 0 {
                format!(" · µ{:+}", h.micro)
            } else {
                String::new()
            };
            let cond_str = if h.cond != crate::pattern::model::TrigCond::Always {
                format!(" · cond:{}", crate::app::format_cond(&h.cond))
            } else {
                String::new()
            };
            let cc_locks = pattern.step_cc(app.cur_col);
            let cc_str = if !cc_locks.is_empty() {
                let cc_list: Vec<String> = cc_locks
                    .iter()
                    .map(|c| format!("cc{}={}", c.cc, c.val))
                    .collect();
                format!(" · {}", cc_list.join(","))
            } else {
                String::new()
            };
            format!(
                "Step {} · {} · Velocity {} [-/+] · Probability {}% [p/P] · Ratchet x{} [y/Y]{}{}{}",
                app.cur_col + 1,
                drum_label(&lane.profile, focused_note),
                h.vel,
                (h.prob * 100.0).round() as i32,
                h.ratchet,
                micro_str,
                cond_str,
                cc_str,
            )
        }
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
    use crate::pattern::model::{DrumHit, Pattern, PatternData, Set, TrigCond};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
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
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "test".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "test32".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "hdr".to_string(),
            desc: String::new(),
            length: 32,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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
            cc: Default::default(),
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
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "det".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
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

    /// M15: at the minimum terminal size, the voice rows are windowed around the
    /// cursor row — the cursor can never sit on a clipped, invisible row.
    #[test]
    fn drum_editor_windows_rows_around_cursor_at_min_size() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        let voices = app.focused_lane().profile.drum_voices;
        let last = voices.len() - 1;
        let last_label = voices[last].label;
        app.cur_row = last;
        assert!(
            voices.len() > 6,
            "test premise: more voices than the window"
        );

        // 12 rows: 6 chrome lines leave a 6-row voice window — smaller than the
        // voice count, so the window MUST page to keep the cursor row visible.
        let backend = TestBackend::new(60, 12);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 60, 12);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let cursor_row = (0..12)
            .map(|y| row_string(buf, area, y))
            .find(|r| r.contains('▸'))
            .expect("cursor row marker must be visible at 60x12");
        assert!(
            cursor_row.contains(last_label),
            "cursor row must show the last voice '{last_label}': {cursor_row:?}"
        );
        let whole: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            whole.contains("voices "),
            "windowed rows must show the voices indicator; got: {whole:?}"
        );
    }

    /// Feature #3: the accent histogram row is drawn beneath the grid.
    #[test]
    fn drum_editor_shows_accent_histogram_row() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit {
            note: 36,
            vel: 127,
            prob: 1.0,
            ratchet: 1,
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "acc".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        let backend = TestBackend::new(92, 18);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 18);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains("acc"),
            "expected accent histogram row label 'acc', got: {whole:?}"
        );
        // A full-velocity hit produces the top bar glyph somewhere in the pane.
        assert!(
            whole.contains('█'),
            "expected an accent bar glyph, got: {whole:?}"
        );
    }

    /// Feature #2: a step with a non-default attribute (here ratchet x2) shows its
    /// marker in the grid at rest, not only in the cursor detail line.
    #[test]
    fn drum_editor_shows_ratchet_marker_in_grid() {
        let mut set = Set::default_set(default_profiles());
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit {
            note: 36,
            vel: 100,
            prob: 1.0,
            ratchet: 2,
            micro: 0,
            cond: TrigCond::Always,
        }];
        set.lanes[0].pattern = Pattern {
            name: "rat".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(steps),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        // Move the cursor off step 0 so the marker isn't obscured by the cursor cell.
        app.cur_col = 8;
        let backend = TestBackend::new(92, 18);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 18);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            whole.contains('²'),
            "expected ratchet-x2 marker ² in the grid, got: {whole:?}"
        );
    }

    /// Feature #1: a left-click on a drum cell toggles that step, exactly as the
    /// keyboard would. Uses the render-time hit-map, so it exercises the real geometry.
    #[test]
    fn mouse_click_toggles_drum_step_via_hitmap() {
        fn present(app: &App, note: u8, col: usize) -> bool {
            match &app.set.lanes[0].pattern.data {
                PatternData::Drums(steps) => steps
                    .get(col)
                    .map(|s| s.iter().any(|h| h.note == note))
                    .unwrap_or(false),
                _ => false,
            }
        }
        let mut set = Set::default_set(default_profiles());
        set.lanes[0].pattern = Pattern {
            name: "click".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Drums(vec![Vec::new(); 16]),
            id: crate::persist::Id::nil(),
            cc: Default::default(),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;

        let area = Rect::new(0, 0, 92, 18);
        let backend = TestBackend::new(92, 18);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        // Find the recorded cell for voice row 0, step 5.
        let cell = app
            .hits
            .borrow()
            .iter()
            .find(|c| {
                matches!(
                    c.target,
                    crate::app::HitTarget::Step {
                        row: 0,
                        col: 5,
                        is_drums: true
                    }
                )
            })
            .cloned()
            .expect("hit-map must contain voice 0 / step 5");
        let note = crate::devices::profiles::DRUM_VOICES[0].note;
        let cx = (cell.x0 + cell.x1) / 2;

        assert!(!present(&app, note, 5), "step starts empty");
        let cmds = app.mouse_press(cx, cell.y0, false);
        assert!(
            present(&app, note, 5),
            "left-click must place a hit at the clicked cell"
        );
        assert!(
            !cmds.is_empty(),
            "toggling must emit an engine reload command"
        );
        assert_eq!(app.cur_col, 5, "click moves the cursor to the clicked step");

        // A fresh click on the same cell toggles it back off.
        app.mouse_release();
        app.mouse_press(cx, cell.y0, false);
        assert!(!present(&app, note, 5), "second click removes the hit");
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
