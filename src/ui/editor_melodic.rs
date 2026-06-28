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

/// 3-char length bar proportional to `len` (in steps), capped at 1 step for the cell.
fn len_cell(len: f32) -> String {
    // fraction of one step filled into 3 columns; len >= 1 fills all 3.
    let frac = len.clamp(0.0, 1.0);
    let filled = (frac * 3.0).round() as usize;
    let mut s = String::with_capacity(3);
    for i in 0..3 {
        s.push(if i < filled { '█' } else { '░' });
    }
    s
}

/// Render the melodic editor into `area`.
pub fn render_melodic_editor(f: &mut Frame, area: Rect, app: &App) {
    let lane = app.focused_lane();
    let len = lane.pattern.step_count();
    let root = lane.profile.root_note;

    // Polymeter: render the playhead at the focused lane's LOCAL step (spec §8).
    let local_playhead = if len == 0 { 0 } else { app.playhead % len };
    let accent = lane_color(lane.profile.id);

    let steps: &[Option<MelodicNote>] = match &lane.pattern.data {
        PatternData::Melodic(s) => s,
        PatternData::Drums(_) => &[],
    };

    let title = format!(
        " EDIT · {} · \"{}\" · {} steps · ch{} · root {} ",
        lane.profile.label,
        lane.pattern.name,
        len,
        lane.profile.channel + 1,
        note_name(root),
    );

    // step header
    let mut step_spans: Vec<Span> = vec![Span::raw("step ")];
    let mut note_spans: Vec<Span> = vec![Span::raw("note ")];
    let mut len_spans: Vec<Span> = vec![Span::raw("len  ")];
    let mut vel_spans: Vec<Span> = vec![Span::raw("vel  ")];

    for col in 0..len {
        let is_cursor = col == app.cur_col;
        let is_playhead = app.playing && col == local_playhead;
        // Cursor and playhead use the shared theme styles (spec §7).
        let cell_style = if is_cursor {
            cursor_style()
        } else if is_playhead {
            playhead_style()
        } else {
            Style::default()
        };

        step_spans.push(Span::raw(format!("{:<4}", col + 1)));

        match steps.get(col).and_then(|s| s.as_ref()) {
            Some(note) => {
                let pitch =
                    resolve_melodic_pitch(root, note.semi, lane.transpose, lane.octave);
                // slide notes get a leading ~ marker; otherwise the note name.
                let label = if note.slide {
                    format!("~{:<3}", note_name(pitch))
                } else {
                    format!("{:<4}", note_name(pitch))
                };
                note_spans.push(Span::styled(label, cell_style));
                len_spans.push(Span::raw(format!("{} ", len_cell(note.len))));
                // Velocity cell tinted by intensity (additive; the bar glyph is unchanged).
                let mv = melodic_velocity(note.vel);
                let bar = vel_bar(mv).to_string();
                vel_spans.push(Span::styled(
                    format!("{bar:<4}"),
                    Style::default().fg(vel_color(mv)),
                ));
            }
            None => {
                note_spans.push(Span::styled(format!("{:<4}", "·"), cell_style));
                len_spans.push(Span::raw("░░░ "));
                vel_spans.push(Span::raw(format!("{:<4}", "·")));
            }
        }
    }

    // cursor detail line
    let detail = match steps.get(app.cur_col).and_then(|s| s.as_ref()) {
        Some(note) => {
            let pitch =
                resolve_melodic_pitch(root, note.semi, lane.transpose, lane.octave);
            let slide = if note.slide { "  ~slide" } else { "" };
            format!(
                "▸ step {} {}  vel {:.2}  len {:.1}{}  prob {}%  ratchet ×{}",
                app.cur_col + 1,
                note_name(pitch),
                note.vel,
                note.len,
                slide,
                (note.prob * 100.0).round() as i32,
                note.ratchet
            )
        }
        None => format!("▸ step {}  (rest)  prob —  ratchet —", app.cur_col + 1),
    };

    let lines = vec![
        Line::from(step_spans),
        Line::from(note_spans),
        Line::from(len_spans),
        Line::from(vel_spans),
        Line::from(Span::raw(detail)),
    ];

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
    use crate::pattern::model::{MelodicNote, Pattern, PatternData, Set};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
    }

    #[test]
    fn melodic_editor_shows_note_name_and_slide_marker() {
        let mut set = Set::default_set(default_profiles());
        // Lane 1 is T-8 BASS (melodic), root_note 45 ("A2"). semi 0 -> A2.
        let mut steps: Vec<Option<MelodicNote>> = vec![None; 16];
        steps[0] = Some(MelodicNote { semi: 0, vel: 1.0, slide: false, len: 1.0, prob: 1.0, ratchet: 1 });
        steps[4] = Some(MelodicNote { semi: 7, vel: 1.0, slide: true, len: 1.0, prob: 1.0, ratchet: 1 });
        set.lanes[1].pattern = Pattern {
            name: "test".to_string(),
            desc: String::new(),
            length: 16,
            data: PatternData::Melodic(steps),
        };
        let mut app = App::new(set, empty_library());
        app.focus = 1;
        app.cur_col = 0;

        let backend = TestBackend::new(92, 8);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_melodic_editor(f, f.area(), &app)).unwrap();

        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("A2"), "expected note name A2, got: {whole:?}");
        assert!(whole.contains('~'), "expected slide marker ~, got: {whole:?}");
        // The cursor detail line shows prob and ratchet for the step-0 note.
        assert!(whole.contains("prob"), "expected detail prob, got: {whole:?}");
        assert!(whole.contains("ratchet"), "expected detail ratchet, got: {whole:?}");
    }
}
