//! 3-lane overview (groovebox mixer row).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::pattern::model::{Lane, PatternData};
use crate::ui::theme::{lane_color, playhead_style};

/// Compact label derived from profile id: "T-8 DRUM" -> "DRUM", "T-8 BASS" -> "BASS",
/// "S-1 SYNTH" -> "SYNTH". Falls back to the raw label for unknown profiles.
fn short_label(profile_id: &str) -> &'static str {
    match profile_id {
        "t8-drums" => "DRUM",
        "t8-bass" => "BASS",
        "s1" => "SYNTH",
        _ => "?",
    }
}

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

    // Mute/solo: compact glyphs. Off = dash, on = filled circle.
    let m_glyph = if lane.mute { "M●" } else { "M–" };
    let s_glyph = if lane.solo { "S●" } else { "S–" };

    // Additive accent: each lane wears its static color (spec §7); the focused lane is
    // also BOLD. A muted lane is DIM so it reads as inactive. Text content is unchanged
    // (substring render tests still pass); degrades to monochrome without color support.
    let mut base_style = Style::default().fg(lane_color(lane.profile.id));
    if focused {
        base_style = base_style.add_modifier(Modifier::BOLD);
    }
    if lane.mute {
        base_style = base_style.add_modifier(Modifier::DIM);
    }

    let label = short_label(lane.profile.id);
    let prefix = format!(
        "{marker}{n} {label:<5} {pat:<12} {conn}  {m_glyph}  ",
        n = idx + 1,
        pat = lane.pattern.name,
    );

    // S glyph gets a brighter accent when solo is active.
    let s_style = if lane.solo {
        Style::default()
            .fg(Color::Rgb(0xFF, 0xFF, 0x80))
            .add_modifier(Modifier::BOLD)
    } else {
        base_style
    };

    // Activity strip: one cell per step. While playing, the lane's LOCAL playhead cell is
    // highlighted, so polymeter is visible (each lane's playhead moves at its own period).
    let mut spans = vec![
        Span::styled(prefix, base_style),
        Span::styled(s_glyph.to_string(), s_style),
        Span::styled("  [".to_string(), base_style),
    ];
    for (i, ch) in activity_strip(lane).chars().enumerate() {
        let cell_style = if app.playing && i == local_playhead {
            playhead_style()
        } else {
            base_style
        };
        spans.push(Span::styled(ch.to_string(), cell_style));
    }
    spans.push(Span::styled("]".to_string(), base_style));
    Line::from(spans)
}

/// Render the lane overview into `area`.
pub fn render_lanes(f: &mut Frame, area: Rect, app: &App) {
    let lines: Vec<Line> = (0..app.set.lanes.len())
        .map(|i| lane_line(i, app))
        .collect();
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
        Library {
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn make_app() -> App {
        let set = Set::default_set(default_profiles());
        App::new(set, empty_library())
    }

    fn row_string(buf: &Buffer, area: Rect, y: u16) -> String {
        let mut s = String::new();
        for x in area.left()..area.right() {
            s.push_str(buf[(x, y)].symbol());
        }
        s
    }

    fn all_rows(buf: &Buffer, area: Rect) -> Vec<String> {
        (0..area.height).map(|y| row_string(buf, area, y)).collect()
    }

    fn render(app: &App, width: u16, height: u16) -> (Terminal<TestBackend>, Rect) {
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        let area = Rect::new(0, 0, width, height);
        term.draw(|f| render_lanes(f, area, app)).unwrap();
        (term, area)
    }

    // --- short label tests -----------------------------------------------

    #[test]
    fn short_label_drum_is_drum() {
        assert_eq!(short_label("t8-drums"), "DRUM");
    }

    #[test]
    fn short_label_bass_is_bass() {
        assert_eq!(short_label("t8-bass"), "BASS");
    }

    #[test]
    fn short_label_synth_is_synth() {
        assert_eq!(short_label("s1"), "SYNTH");
    }

    #[test]
    fn lanes_render_short_labels_drum_bass_synth() {
        let app = make_app();
        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let whole: String = rows.join("");
        assert!(whole.contains("DRUM"), "expected DRUM in: {whole:?}");
        assert!(whole.contains("BASS"), "expected BASS in: {whole:?}");
        assert!(whole.contains("SYNTH"), "expected SYNTH in: {whole:?}");
        // full labels must NOT appear (we show compact forms)
        assert!(
            !whole.contains("T-8 DRUM"),
            "must not show full label: {whole:?}"
        );
        assert!(
            !whole.contains("T-8 BASS"),
            "must not show full label: {whole:?}"
        );
        assert!(
            !whole.contains("S-1 SYNTH"),
            "must not show full label: {whole:?}"
        );
    }

    // --- focus marker test -----------------------------------------------

    #[test]
    fn focus_marker_appears_only_on_focused_lane() {
        let mut app = make_app();
        app.focus = 1; // focus BASS lane

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);

        // Find the row containing BASS and confirm it has ▸
        let bass_row = rows.iter().find(|r| r.contains("BASS")).expect("BASS row");
        assert!(
            bass_row.contains('▸'),
            "focus marker on BASS row: {bass_row:?}"
        );

        // The other rows must not have ▸
        for row in rows.iter().filter(|r| !r.contains("BASS")) {
            assert!(
                !row.contains('▸'),
                "no focus marker on non-focused row: {row:?}"
            );
        }
    }

    // --- mute/solo indicator tests ----------------------------------------

    #[test]
    fn unmuted_lane_shows_mute_off_glyph() {
        let mut app = make_app();
        app.set.lanes[0].mute = false;

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains("M–"),
            "unmuted should show M–: {drum_row:?}"
        );
    }

    #[test]
    fn muted_lane_shows_mute_on_glyph() {
        let mut app = make_app();
        app.set.lanes[0].mute = true;

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains("M●"),
            "muted should show M●: {drum_row:?}"
        );
    }

    #[test]
    fn unsoloed_lane_shows_solo_off_glyph() {
        let mut app = make_app();
        app.set.lanes[1].solo = false;

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let bass_row = rows.iter().find(|r| r.contains("BASS")).expect("BASS row");
        assert!(
            bass_row.contains("S–"),
            "unsoloed should show S–: {bass_row:?}"
        );
    }

    #[test]
    fn soloed_lane_shows_solo_on_glyph() {
        let mut app = make_app();
        app.set.lanes[1].solo = true;

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let bass_row = rows.iter().find(|r| r.contains("BASS")).expect("BASS row");
        assert!(
            bass_row.contains("S●"),
            "soloed should show S●: {bass_row:?}"
        );
    }

    // --- connection indicator tests ---------------------------------------

    #[test]
    fn connected_lane_shows_filled_circle() {
        let mut app = make_app();
        app.device_status[0] = (true, "T-8 port".to_string());

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains('●'),
            "connected should show ●: {drum_row:?}"
        );
    }

    #[test]
    fn disconnected_lane_shows_open_circle() {
        let mut app = make_app();
        app.device_status[2] = (false, String::new());

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let synth_row = rows
            .iter()
            .find(|r| r.contains("SYNTH"))
            .expect("SYNTH row");
        assert!(
            synth_row.contains('○'),
            "disconnected should show ○: {synth_row:?}"
        );
    }

    // --- color test (kept from original) ---------------------------------

    #[test]
    fn focused_lane_label_uses_its_lane_color() {
        use crate::ui::theme::lane_color;
        let mut app = make_app();
        app.focus = 1; // BASS, profile id "t8-bass"

        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let want = lane_color("t8-bass");
        let bass_y = (0..5)
            .find(|&y| row_string(buf, area, y).contains("BASS"))
            .expect("bass row");
        let colored =
            (area.left()..area.right()).any(|x| buf[(x, bass_y)].style().fg == Some(want));
        assert!(colored, "bass row should use the t8-bass lane color");
    }
}
