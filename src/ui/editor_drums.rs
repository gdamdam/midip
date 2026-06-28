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
fn hit_at<'a>(
    steps: &'a [Vec<crate::pattern::model::DrumHit>],
    step: usize,
    note: u8,
) -> Option<&'a crate::pattern::model::DrumHit> {
    steps.get(step).and_then(|s| s.iter().find(|h| h.note == note))
}

/// Render the drum editor into `area`.
pub fn render_drum_editor(f: &mut Frame, area: Rect, app: &App) {
    let lane = app.focused_lane();
    let voices = lane.profile.drum_voices;
    let len = lane.pattern.step_count();

    // Polymeter: render the playhead at the focused lane's LOCAL step (spec §8).
    let local_playhead = if len == 0 { 0 } else { app.playhead % len };
    let accent = lane_color(lane.profile.id);

    let steps: &[Vec<crate::pattern::model::DrumHit>] = match &lane.pattern.data {
        PatternData::Drums(s) => s,
        // A melodic lane should never reach here (caller routes by kind), but be safe.
        PatternData::Melodic(_) => &[],
    };

    let title = format!(
        " EDIT · {} · \"{}\" · {} steps · ch{} ",
        lane.profile.label,
        lane.pattern.name,
        len,
        lane.profile.channel + 1
    );

    let mut lines: Vec<Line> = Vec::with_capacity(voices.len() + 1);

    // Header row: step group numbers (1,5,9,13...).
    {
        let mut spans: Vec<Span> = vec![Span::raw("        │ ")];
        for col in 0..len {
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
        let mut spans: Vec<Span> =
            vec![Span::raw(format!("{marker}{:<3} {:>2} │ ", voice.label, voice.note))];

        for col in 0..len {
            let vel = hit_vel(steps, col, voice.note);
            let glyph = vel_glyph(vel).to_string();
            let is_cursor = focused_row && col == app.cur_col;
            let is_playhead = app.playing && col == local_playhead;

            // Cursor and playhead use the shared theme styles; otherwise hit cells are
            // tinted by velocity intensity (additive — the glyph text is unchanged).
            let style = if is_cursor {
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

    // Euclid indicator for the focused voice: E(pulses,steps) r<rotation>, where pulses =
    // count of steps where the focused voice's note is present.
    let focused_note = voices.get(app.cur_row).map(|v| v.note).unwrap_or(0);
    let pulses = steps
        .iter()
        .filter(|s| s.iter().any(|h| h.note == focused_note))
        .count();
    lines.push(Line::from(Span::raw(format!(
        "  E({},{}) r{}",
        pulses, len, app.euclid_rotation
    ))));

    // Cursor detail line: the focused voice's hit at the cursor step (vel / prob% / ratchet).
    let detail = match hit_at(steps, app.cur_col, focused_note) {
        Some(h) => format!(
            "  ▸ step {}  {}  vel {}  prob {}%  ratchet ×{}",
            app.cur_col + 1,
            drum_label(&lane.profile, focused_note),
            h.vel,
            (h.prob * 100.0).round() as i32,
            h.ratchet
        ),
        None => format!(
            "  ▸ step {}  {}  (empty)  prob —  ratchet —",
            app.cur_col + 1,
            drum_label(&lane.profile, focused_note),
        ),
    };
    lines.push(Line::from(Span::raw(detail)));

    // Title accent uses the lane's static color (additive; degrades to monochrome).
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
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
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
        // Lane 0 is T-8 DRUM. Build a pattern with a BD (note 36) hit at step 0.
        let mut steps: Vec<Vec<DrumHit>> = vec![Vec::new(); 16];
        steps[0] = vec![DrumHit { note: 36, vel: 127, prob: 1.0, ratchet: 1 }];
        set.lanes[0].pattern = Pattern {
            name: "test".to_string(),
            length: 16,
            data: PatternData::Drums(steps),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 0;

        let backend = TestBackend::new(92, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let whole: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(whole.contains("BD"), "expected BD voice label, got: {whole:?}");

        // The BD row must contain the full-velocity glyph for the step-0 hit.
        let bd_row = (0..16)
            .map(|y| row_string(buf, area, y))
            .find(|r| r.contains("BD"))
            .expect("BD row");
        assert!(bd_row.contains('█'), "expected hit glyph on BD row: {bd_row:?}");

        // The euclid indicator and the cursor detail line are rendered.
        assert!(whole.contains("E("), "expected euclid indicator E(...), got: {whole:?}");
        assert!(whole.contains("prob"), "expected cursor detail prob, got: {whole:?}");
        assert!(whole.contains("ratchet"), "expected cursor detail ratchet, got: {whole:?}");
    }

    #[test]
    fn playhead_renders_at_local_step_under_polymeter() {
        use crate::ui::theme::playhead_style;
        // Focused lane has length 16; absolute playhead 20 -> local column 20 % 16 = 4.
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 0;
        app.playing = true;
        app.playhead = 20; // ABSOLUTE step

        let backend = TestBackend::new(92, 16);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 16);
        term.draw(|f| render_drum_editor(f, area, &app)).unwrap();

        // The playhead background must appear somewhere on the grid; if the absolute step
        // 20 were used (out of the 16-step range) nothing would be highlighted. Its
        // presence proves the local `% length` wrap. (Local column 4 falls inside group 2.)
        let buf = term.backend().buffer();
        let want_bg = playhead_style().bg.expect("playhead style has a bg");
        let highlighted = (0..16u16).any(|y| {
            (area.left()..area.right()).any(|x| buf[(x, y)].style().bg == Some(want_bg))
        });
        assert!(highlighted, "expected the local playhead column (4) highlighted under polymeter");
    }
}
