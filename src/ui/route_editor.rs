//! Route editor overlay: per-lane port / channel / clock-out assignment + connection status.

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, RouteField};

/// Connected glyph (matches the lanes overview convention).
const CONNECTED: &str = "●";
/// Disconnected / missing glyph.
const MISSING: &str = "○";

/// Render the route editor overlay into `area`.
pub fn render_route_editor(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    // Opaque theme bg: `Clear` above resets the rect to the terminal default, so
    // repaint the theme background here (the panel now fills the Setup body base,
    // not a small centered float over an already-painted backdrop).
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(EMBER.bg))
        .title(Span::styled(
            " ROUTE EDITOR ",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Header row.
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<12}", "LANE"),
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<26}", "PORT"),
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<5}", "CH"),
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<6}", "CLK"),
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "CON",
            Style::default()
                .fg(EMBER.synth)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(""));

    for (i, lane) in app.set.lanes.iter().enumerate() {
        let selected = i == app.route_editor_lane;
        let (connected, _port_name) = app
            .device_status
            .get(i)
            .cloned()
            .unwrap_or((false, String::new()));
        let conn_glyph = if connected { CONNECTED } else { MISSING };
        let conn_color = if connected { EMBER.ok } else { EMBER.err };

        let effective = lane.effective_route();
        // Port display: show explicit port name, or "(default: <profile port>)" when None.
        let port_display = match &lane.route {
            Some(r) => r.port.name.clone(),
            None => format!("(default: {})", effective.port.name),
        };
        // Channel is 0-indexed internally; display as 1-based.
        let channel_display = format!("{}", effective.channel + 1);
        let clock_display = if effective.clock_out { "on" } else { "off" };

        // Lane name label: profile id.
        let lane_label = lane.profile.id;

        // Highlight selected lane's focused field.
        let base_style = if selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let sel_style = Style::default()
            .fg(EMBER.bg)
            .bg(EMBER.synth)
            .add_modifier(Modifier::BOLD);

        let port_style = if selected && app.route_editor_field == RouteField::Port {
            sel_style
        } else {
            base_style
        };
        let ch_style = if selected && app.route_editor_field == RouteField::Channel {
            sel_style
        } else {
            base_style
        };
        let clk_style = if selected && app.route_editor_field == RouteField::ClockOut {
            sel_style
        } else {
            base_style
        };

        let marker = if selected { "▸ " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{}{:<10}", marker, truncate(lane_label, 10)),
                base_style,
            ),
            Span::styled(format!("{:<26}", truncate(&port_display, 25)), port_style),
            Span::styled(format!("{:<5}", channel_display), ch_style),
            Span::styled(format!("{:<6}", clock_display), clk_style),
            Span::styled(conn_glyph, Style::default().fg(conn_color)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[↑↓]lane  [←→]field  [c]cycle port  [[ /]]channel  [z]clock-out  [esc]close",
        Style::default().fg(EMBER.dim),
    )));

    f.render_widget(Paragraph::new(lines), inner);
}

/// Truncate a &str to at most `max_chars` characters.
fn truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let mut end = 0;
    for (i, _) in s.char_indices().take(max_chars) {
        end = i;
    }
    &s[..s[end..]
        .char_indices()
        .nth(1)
        .map(|(j, _)| end + j)
        .unwrap_or(s.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Mode, RouteField};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::{LaneRoute, PortRef, Set};
    use ratatui::backend::TestBackend;
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

    fn render_route_editor_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_route_editor(f, f.area(), app))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    fn make_app() -> App {
        let set = Set::default_set(default_profiles());
        App::new(set, empty_library())
    }

    #[test]
    fn route_editor_shows_lane_names() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        app.route_editor_lane = 0;
        app.route_editor_field = RouteField::Port;
        let whole = render_route_editor_to_string(&app);
        // Default profiles: t8-drums, t8-bass, s1
        assert!(
            whole.contains("t8-drums"),
            "expected t8-drums lane label; got: {whole:?}"
        );
        assert!(
            whole.contains("t8-bass"),
            "expected t8-bass lane label; got: {whole:?}"
        );
        assert!(
            whole.contains("s1"),
            "expected s1 lane label; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_shows_default_port_label_when_no_explicit_route() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        app.route_editor_lane = 0;
        // No explicit route on any lane → should show "(default: ..."
        let whole = render_route_editor_to_string(&app);
        assert!(
            whole.contains("default"),
            "expected '(default:' label when no explicit route; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_shows_explicit_port_name_when_route_set() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        app.route_editor_lane = 0;
        app.set.lanes[0].route = Some(LaneRoute {
            port: PortRef {
                stable_key: "Roland T-8".into(),
                name: "Roland T-8".into(),
            },
            channel: 9,
            clock_out: true,
        });
        let whole = render_route_editor_to_string(&app);
        assert!(
            whole.contains("Roland T-8"),
            "expected explicit port name; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_shows_connection_glyphs() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        // Simulate lane 0 connected, lanes 1 and 2 not connected.
        app.device_status[0] = (true, "Roland T-8".to_string());
        app.device_status[1] = (false, String::new());
        app.device_status[2] = (false, String::new());
        let whole = render_route_editor_to_string(&app);
        // At least one connected glyph and at least one missing glyph.
        assert!(
            whole.contains(CONNECTED),
            "expected connected glyph ●; got: {whole:?}"
        );
        assert!(
            whole.contains(MISSING),
            "expected missing glyph ○; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_shows_channel_one_based() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        // Default lane 0 (t8-drums) has profile channel 9 → display as "10".
        let profiles = default_profiles();
        let profile_ch = profiles[0].channel; // should be 9
        let whole = render_route_editor_to_string(&app);
        let display_ch = (profile_ch + 1).to_string();
        assert!(
            whole.contains(&display_ch),
            "expected 1-based channel display '{display_ch}'; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_shows_selection_marker_on_focused_lane() {
        let mut app = make_app();
        app.mode = Mode::RouteEditor;
        app.route_editor_lane = 1;
        let whole = render_route_editor_to_string(&app);
        assert!(
            whole.contains('▸'),
            "expected selection marker ▸; got: {whole:?}"
        );
    }

    #[test]
    fn route_editor_differs_by_mode() {
        // When mode is Edit the route editor overlay is NOT shown (rendered via mod.rs dispatch).
        // This test just confirms the route editor renders something when mode is RouteEditor.
        let mut app_re = make_app();
        app_re.mode = Mode::RouteEditor;
        let with_re = render_route_editor_to_string(&app_re);
        assert!(
            with_re.contains("ROUTE EDITOR"),
            "expected ROUTE EDITOR title; got: {with_re:?}"
        );
    }
}
