//! 3-lane overview (groovebox mixer row).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::pattern::model::{Lane, PatternData};
use crate::ui::theme::{lane_color, playhead_style};

/// 16-cell activity strip: `●` for a step with content, `·` for empty.
fn activity_strip(lane: &Lane) -> String {
    let mut s = String::with_capacity(16);
    match &lane.pattern.data {
        PatternData::Drums(steps) => {
            for i in 0..16 {
                let filled = steps.get(i).map(|s| !s.is_empty()).unwrap_or(false);
                s.push(if filled { '●' } else { '·' });
            }
        }
        PatternData::Melodic(steps) => {
            for i in 0..16 {
                let filled = steps.get(i).map(|s| s.is_some()).unwrap_or(false);
                s.push(if filled { '●' } else { '·' });
            }
        }
    }
    s
}

fn lane_line(idx: usize, app: &App) -> Line<'static> {
    let lane = &app.set.lanes[idx];
    let focused = app.focus == idx;
    let marker = if focused { "▸" } else { " " };

    // Polymeter: this lane's LOCAL playhead position (spec §8). `app.playhead` is the
    // absolute step; each lane wraps independently by its own length.
    let local_playhead = if lane.pattern.length == 0 {
        0
    } else {
        app.playhead % lane.pattern.length
    };

    let (connected, _port) = app
        .device_status
        .get(idx)
        .cloned()
        .unwrap_or((false, String::new()));
    let conn = if connected { '●' } else { '○' };

    let chan = lane.profile.channel + 1; // display 1-indexed
    let m = if lane.mute { "[M]" } else { "[ ]" };
    let s = if lane.solo { "[S]" } else { "[ ]" };

    // Additive accent: each lane wears its static color (spec §7); the focused lane is
    // also BOLD. Text content is unchanged (substring render tests still pass); degrades
    // to monochrome automatically without color support.
    let mut style = Style::default().fg(lane_color(lane.profile.id));
    if focused {
        style = style.add_modifier(Modifier::BOLD);
    }

    // Prefix (everything up to the activity strip) wears the lane accent.
    let prefix = format!(
        "{marker}{n} {label:<10} {pat:<12} {conn} ch{chan:>2}  {m}{s}  ",
        n = idx + 1,
        label = lane.profile.label,
        pat = lane.pattern.name,
    );

    // Activity strip: one cell per step. While playing, the lane's LOCAL playhead cell is
    // highlighted, so polymeter is visible (each lane's playhead moves at its own period).
    let mut spans = vec![Span::styled(prefix, style)];
    for (i, ch) in activity_strip(lane).chars().enumerate() {
        let cell_style = if app.playing && i == local_playhead {
            playhead_style()
        } else {
            style
        };
        spans.push(Span::styled(ch.to_string(), cell_style));
    }
    Line::from(spans)
}

/// Render the lane overview into `area`.
pub fn render_lanes(f: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = (0..app.set.lanes.len()).map(|i| lane_line(i, app)).collect();
    let block = Block::default().borders(Borders::ALL).title(" LANES ");
    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
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
    fn lanes_show_labels_and_focus_marker() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 1; // focus T-8 BASS

        let backend = TestBackend::new(92, 5);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 5);
        term.draw(|f| render_lanes(f, area, &app)).unwrap();

        let buf = term.backend().buffer();
        let whole: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(whole.contains("T-8 DRUM"), "got: {whole:?}");
        assert!(whole.contains("T-8 BASS"), "got: {whole:?}");
        assert!(whole.contains("S-1 SYNTH"), "got: {whole:?}");

        // The focused lane row (index 1) carries the ▸ marker; the others do not.
        // Rows are offset by 1 for the block/title; find the row that contains BASS.
        let bass_row = (0..5)
            .map(|y| row_string(buf, area, y))
            .find(|r| r.contains("T-8 BASS"))
            .expect("bass row");
        assert!(bass_row.contains('▸'), "focus marker on bass row: {bass_row:?}");
    }

    #[test]
    fn focused_lane_label_uses_its_lane_color() {
        use crate::ui::theme::lane_color;
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.focus = 1; // T-8 BASS, profile id "t8-bass"

        let backend = TestBackend::new(92, 5);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, 92, 5);
        term.draw(|f| render_lanes(f, area, &app)).unwrap();

        // Find a cell on the BASS row and confirm it carries the bass lane color.
        let buf = term.backend().buffer();
        let want = lane_color("t8-bass");
        let bass_y = (0..5)
            .find(|&y| row_string(buf, area, y).contains("T-8 BASS"))
            .expect("bass row");
        let colored = (area.left()..area.right())
            .any(|x| buf[(x, bass_y)].style().fg == Some(want));
        assert!(colored, "bass row should use the t8-bass lane color");
    }
}
