//! 3-lane overview (groovebox mixer row).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
#[cfg(test)]
use crate::pattern::model::TrigCond;
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
///
/// For patterns with ≤16 steps each cell maps 1:1 to a step (same as before).
/// For longer patterns the strip downsamples: cell `c` represents step
/// `c * length / 16`, so all content — including steps beyond position 15 —
/// is visible in the strip.
fn activity_strip(lane: &Lane) -> String {
    const CELLS: usize = 16;
    let length = lane.pattern.length.max(1);
    let mut s = String::with_capacity(CELLS);
    match &lane.pattern.data {
        PatternData::Drums(steps) => {
            for c in 0..CELLS {
                let i = c * length / CELLS;
                let filled = steps.get(i).map(|s| !s.is_empty()).unwrap_or(false);
                s.push(if filled { '●' } else { '·' });
            }
        }
        PatternData::Melodic(steps) => {
            for c in 0..CELLS {
                let i = c * length / CELLS;
                let filled = steps.get(i).map(|s| !s.is_empty()).unwrap_or(false);
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

    // Show the active (committed) pattern name here; a pending launch is shown
    // separately as a QUEUED⟶ marker after the activity strip (below).
    let queued_name: Option<String> = app.queued.get(idx).and_then(|q| q.clone());
    let pat_display = lane.pattern.name.clone();
    let prefix = format!(
        "{marker}{n} {label:<5} {pat:<12} {conn}  {m_glyph}  ",
        n = idx + 1,
        pat = pat_display,
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
    // For patterns longer than 16 steps the strip downsamples (cell = step*16/length),
    // so we convert the step-based playhead to a cell index for the highlight.
    const STRIP_CELLS: usize = 16;
    let length = lane.pattern.length.max(1);
    let playhead_cell = local_playhead * STRIP_CELLS / length;
    let mut spans = vec![
        Span::styled(prefix, base_style),
        Span::styled(s_glyph.to_string(), s_style),
        Span::styled("  [".to_string(), base_style),
    ];
    for (i, ch) in activity_strip(lane).chars().enumerate() {
        let cell_style = if app.playing && i == playhead_cell {
            playhead_style()
        } else {
            base_style
        };
        spans.push(Span::styled(ch.to_string(), cell_style));
    }
    spans.push(Span::styled("]".to_string(), base_style));

    // QUEUED marker: shown after the activity strip when a launch is pending.
    // Distinct amber style so it reads clearly as "not yet active".
    if let Some(name) = queued_name {
        let queued_style = Style::default()
            .fg(Color::Rgb(0xF5, 0xB0, 0x41))
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled(
            format!("  QUEUED\u{27f6}{}", name),
            queued_style,
        ));
    }

    // FILL indicator: shown when a temporary fill is active on this lane.
    // Magenta/bold so it reads clearly as a live, non-committed transformation.
    let has_fill = app
        .temp_transform
        .as_ref()
        .map(|tt| tt.lane == idx)
        .unwrap_or(false);
    if has_fill {
        let fill_style = Style::default()
            .fg(Color::Rgb(0xFF, 0x44, 0xCC))
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled("  FILL", fill_style));
    }

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

    // --- M3 Task 2: QUEUED marker ----------------------------------------

    #[test]
    fn queued_lane_shows_queued_marker_with_name() {
        let mut app = make_app();
        // Simulate a pending queued launch on lane 0 (DRUM).
        app.queued[0] = Some("queued-pat".to_string());

        let (term, area) = render(&app, 100, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains("QUEUED"),
            "lane with queued should show QUEUED marker: {drum_row:?}"
        );
        assert!(
            drum_row.contains("queued-pat"),
            "QUEUED marker should include the pattern name: {drum_row:?}"
        );
    }

    #[test]
    fn non_queued_lane_does_not_show_queued_marker() {
        let app = make_app();
        // No queued entries (all None by default).
        let (term, area) = render(&app, 100, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let whole: String = rows.join("");
        assert!(
            !whole.contains("QUEUED"),
            "no lane should show QUEUED when nothing is queued: {whole:?}"
        );
    }

    // --- >16-step overview strip test ------------------------------------

    #[test]
    fn overview_strip_shows_activity_beyond_step_16() {
        use crate::pattern::model::{DrumHit, PatternData};

        let mut app = make_app();
        // Set lane 0 (drums) to 32 steps, hit only at step 20 (beyond the old fixed 16-cell view).
        if let PatternData::Drums(ref mut steps) = app.set.lanes[0].pattern.data {
            *steps = vec![Vec::new(); 32];
            steps[20] = vec![DrumHit {
                note: 36,
                vel: 100,
                prob: 1.0,
                ratchet: 1,
                micro: 0,
                cond: TrigCond::Always,
            }];
        }
        app.set.lanes[0].pattern.length = 32;

        // activity_strip maps cell c → step c*32/16 = c*2.
        // Step 20 lands in cell 20/2 = 10 (cell 10 → step 10*2=20). Verify ● appears.
        let strip = activity_strip(&app.set.lanes[0]);
        assert_eq!(strip.chars().count(), 16, "strip always 16 cells");
        let cells: Vec<char> = strip.chars().collect();
        assert_eq!(
            cells[10], '●',
            "cell 10 should show hit at step 20: {strip:?}"
        );
        // Cells not corresponding to step 20 must be empty.
        for (c, &ch) in cells.iter().enumerate() {
            if c != 10 {
                assert_eq!(ch, '·', "cell {c} should be empty: {strip:?}");
            }
        }

        // Also verify the rendered overview row contains '●'.
        let (term, area) = render(&app, 80, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains('●'),
            "overview strip must show ● for 32-step hit: {drum_row:?}"
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

    // --- M4b Task 3: FILL indicator -----------------------------------------

    #[test]
    fn fill_indicator_shown_when_temp_transform_active_on_lane() {
        use crate::app::TempTransform;
        use crate::pattern::model::{Pattern, PatternData};

        let mut app = make_app();
        // Inject an active temp_transform on lane 0.
        app.temp_transform = Some(TempTransform {
            lane: 0,
            original: Pattern {
                name: "orig".into(),
                desc: String::new(),
                length: 16,
                data: PatternData::Drums(vec![Vec::new(); 16]),
                id: crate::persist::Id::nil(),
                cc: Default::default(),
            },
        });

        let (term, area) = render(&app, 100, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let drum_row = rows.iter().find(|r| r.contains("DRUM")).expect("DRUM row");
        assert!(
            drum_row.contains("FILL"),
            "lane with active temp_transform must show FILL indicator: {drum_row:?}"
        );
    }

    #[test]
    fn fill_indicator_absent_when_no_temp_transform() {
        let app = make_app();
        assert!(app.temp_transform.is_none());
        let (term, area) = render(&app, 100, 5);
        let buf = term.backend().buffer();
        let rows = all_rows(buf, area);
        let whole: String = rows.join("");
        assert!(
            !whole.contains("FILL"),
            "no FILL indicator should appear when temp_transform is None: {whole:?}"
        );
    }
}
