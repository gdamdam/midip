//! Device picker overlay: assign a catalog device to the focused lane.
//!
//! Lists the device catalog filtered to the focused lane's kind (drums or
//! melodic) — a drum lane only offers drum machines, a melodic lane only offers
//! synths. Restricting to same-kind keeps the lane's pattern data valid across
//! the swap. Selecting a device swaps the lane's `DeviceProfile` and re-routes
//! it to that device's port + default channel (the engine reconnects).

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::devices::profiles::DeviceProfile;
use crate::pattern::model::LaneKind;

/// The catalog entries selectable for a lane of `kind`. Drum lanes get drum
/// devices; melodic lanes get synths. Catalog order (built-ins first, then
/// shipped devices, then any user additions).
pub fn choices(kind: LaneKind) -> Vec<DeviceProfile> {
    crate::devices::profiles::catalog()
        .iter()
        .copied()
        .filter(|p| p.kind == kind)
        .collect()
}

/// Render the device picker overlay into `area`.
pub fn render_device_picker(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " DEVICE PICKER ",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lane = app.device_picker_lane;
    let kind = app
        .set
        .lanes
        .get(lane)
        .map(|l| l.profile.kind)
        .unwrap_or(LaneKind::Melodic);
    let kind_label = match kind {
        LaneKind::Drums => "drum machines",
        LaneKind::Melodic => "synths",
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Lane {lane} — choose a device ({kind_label}):"),
        Style::default()
            .fg(EMBER.synth)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let choices = choices(kind);
    let cur_id = app.set.lanes.get(lane).map(|l| l.profile.id);

    for (i, p) in choices.iter().enumerate() {
        let selected = i == app.device_picker_index;
        let is_current = Some(p.id) == cur_id;
        let marker = if selected { "▸ " } else { "  " };
        let current_tag = if is_current { "(current)" } else { "" };
        // Right column: MIDI channel (1-based) + a kind-specific hint.
        let detail = match p.kind {
            LaneKind::Drums => format!("ch {}  {} voices", p.channel + 1, p.drum_voices.len()),
            LaneKind::Melodic => format!(
                "ch {}  {}",
                p.channel + 1,
                if p.poly { "poly" } else { "mono" }
            ),
        };
        let style = if selected {
            Style::default()
                .fg(EMBER.bg)
                .bg(EMBER.synth)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}{:<16}{:<10}", marker, p.label, current_tag),
                style,
            ),
            Span::styled(detail, Style::default().fg(EMBER.dim)),
        ]));
    }

    if choices.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no devices for this lane kind)",
            Style::default().fg(EMBER.dim),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[↑↓]device  [enter]select  [esc]cancel   — fine-tune port/channel in the route editor [w]",
        Style::default().fg(EMBER.dim),
    )));

    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            chords: crate::pattern::library::GenreMap::new(),
            records: Vec::new(),
            v2_index: Default::default(),
            families: Vec::new(),
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn make_app() -> App {
        App::new(Set::default_set(default_profiles()), empty_library())
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_device_picker(f, f.area(), app))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn choices_filter_by_lane_kind() {
        let drums = choices(LaneKind::Drums);
        assert!(drums.iter().all(|p| p.kind == LaneKind::Drums));
        assert!(drums.iter().any(|p| p.id == "t8-drums"));
        assert!(drums.iter().any(|p| p.id == "rd-8"));
        assert!(drums.iter().all(|p| p.id != "td-3"));

        let mel = choices(LaneKind::Melodic);
        assert!(mel.iter().all(|p| p.kind == LaneKind::Melodic));
        assert!(mel.iter().any(|p| p.id == "s1"));
        assert!(mel.iter().any(|p| p.id == "td-3"));
        assert!(mel.iter().all(|p| p.id != "rd-8"));
    }

    #[test]
    fn picker_lists_devices_for_focused_lane_kind() {
        let mut app = make_app();
        app.mode = Mode::DevicePicker;
        app.device_picker_lane = 0; // default lane 0 is t8-drums (Drums)
        app.device_picker_index = 0;
        let whole = render_to_string(&app);
        assert!(whole.contains("DEVICE PICKER"), "got: {whole:?}");
        assert!(
            whole.contains("RD-8 DRUM"),
            "expected a drum device label; got: {whole:?}"
        );
    }

    #[test]
    fn picker_marks_the_current_device() {
        let mut app = make_app();
        app.mode = Mode::DevicePicker;
        app.device_picker_lane = 0;
        let whole = render_to_string(&app);
        assert!(
            whole.contains("(current)"),
            "expected a (current) tag; got: {whole:?}"
        );
    }
}
