//! Always-visible transport header (two-line layout).
//!
//! Line 1: ▶ PLAY | 124 BPM | LINK 2 LOCKED | 001.2.3 | SW 56% | SAVED
//! Line 2: status/toast (app.status), blank when empty.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::ui::theme::lane_color;

// --- static styles -----------------------------------------------------------

fn sep_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn muted_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn bright_style() -> Style {
    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
}

fn ok_style() -> Style {
    // Subtle green for SAVED
    Style::default().fg(Color::Rgb(0x7A, 0xC9, 0x80))
}

fn warn_style() -> Style {
    // Amber for EDITED — draws the eye without alarming
    Style::default().fg(Color::Rgb(0xF5, 0xB0, 0x41))
}

fn toast_style() -> Style {
    Style::default().fg(Color::Rgb(0xCC, 0xCC, 0xCC))
}

/// Render the transport header into `area`.
///
/// Uses 2 rows inside the block border:
///   row 0 — state bar (play/BPM/LINK/position/swing/save)
///   row 1 — status/toast line
pub fn render_transport(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" midip ");

    // Accent on the play indicator = focused lane's static color (spec §7).
    let accent = lane_color(app.focused_lane().profile.id);

    // --- play/stop ----------------------------------------------------------
    let (play_glyph, play_label) = if app.playing {
        ("▶ ", "PLAY")
    } else {
        ("■ ", "STOP")
    };

    // --- BPM ----------------------------------------------------------------
    let bpm_field: String = if app.mode == Mode::TempoEntry {
        format!("BPM: {}_", app.tempo_input)
    } else {
        let bpm = if app.link_enabled { app.link_tempo } else { app.set.bpm };
        format!("{} BPM", bpm.round() as i64)
    };

    // --- LINK ---------------------------------------------------------------
    let link_field: String = if app.link_enabled {
        let suffix = if app.link_peers > 0 { " LOCKED" } else { "" };
        format!("LINK {}{}", app.link_peers, suffix)
    } else {
        "LINK off".to_string()
    };

    // --- position: bar.beat.sixteenth (all 1-based) -------------------------
    let sixteenth = app.playhead % 4 + 1;
    let beat = (app.playhead / 4) % 4 + 1;
    let bar_num = app.bar + 1;
    let pos_field = format!("{:03}.{}.{}", bar_num, beat, sixteenth);

    // --- swing --------------------------------------------------------------
    let swing_pct = (app.set.swing * 100.0).round() as i64;
    let swing_field = format!("SW {}%", swing_pct);

    // --- save state ---------------------------------------------------------
    let saved = !app.dirty();

    // --- assemble top line --------------------------------------------------
    let sep = Span::styled(" | ", sep_style());
    let mut top_spans = vec![
        Span::styled(play_glyph, Style::default().fg(accent).add_modifier(Modifier::BOLD)),
        Span::styled(play_label, Style::default().fg(accent).add_modifier(Modifier::BOLD)),
        sep.clone(),
        Span::styled(bpm_field, bright_style()),
        sep.clone(),
        Span::styled(link_field, muted_style()),
        sep.clone(),
        Span::styled(pos_field, muted_style()),
        sep.clone(),
        Span::styled(swing_field, muted_style()),
        sep.clone(),
    ];
    if saved {
        top_spans.push(Span::styled("SAVED", ok_style()));
    } else {
        top_spans.push(Span::styled("EDITED", warn_style()));
    }

    // --- status/toast line --------------------------------------------------
    let status_line = if app.status.is_empty() {
        Line::from(Span::raw(""))
    } else {
        Line::from(Span::styled(app.status.clone(), toast_style()))
    };

    // --- render -------------------------------------------------------------
    let inner = block.inner(area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    f.render_widget(block, area);
    f.render_widget(Paragraph::new(Line::from(top_spans)), rows[0]);
    f.render_widget(Paragraph::new(status_line), rows[1]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, App, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
    }

    fn make_app() -> App {
        let set = Set::default_set(default_profiles());
        App::new(set, empty_library())
    }

    fn buf_text(t: &Terminal<TestBackend>) -> String {
        t.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    /// Render into a 100×5 TestBackend and return the full buffer string.
    fn render(app: &App) -> String {
        let backend = TestBackend::new(100, 5);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_transport(f, f.area(), app)).unwrap();
        buf_text(&term)
    }

    #[test]
    fn shows_play_when_playing() {
        let mut app = make_app();
        app.playing = true;
        let text = render(&app);
        assert!(text.contains("PLAY"), "expected PLAY, got: {text:?}");
        assert!(!text.contains("STOP"), "should not show STOP when playing, got: {text:?}");
    }

    #[test]
    fn shows_stop_when_not_playing() {
        let app = make_app();
        let text = render(&app);
        assert!(text.contains("STOP"), "expected STOP, got: {text:?}");
        assert!(!text.contains("PLAY"), "should not show PLAY when stopped, got: {text:?}");
    }

    #[test]
    fn shows_set_bpm_when_link_disabled() {
        let mut app = make_app();
        app.set.bpm = 124.0;
        app.link_enabled = false;
        let text = render(&app);
        assert!(text.contains("124"), "expected 124, got: {text:?}");
        assert!(text.contains("BPM"), "expected BPM label, got: {text:?}");
    }

    #[test]
    fn shows_link_tempo_when_link_enabled() {
        let mut app = make_app();
        app.set.bpm = 100.0;
        app.link_enabled = true;
        app.link_tempo = 128.5; // rounds to 129
        let text = render(&app);
        assert!(text.contains("129"), "expected rounded link_tempo 129, got: {text:?}");
        assert!(text.contains("BPM"), "expected BPM label, got: {text:?}");
    }

    #[test]
    fn shows_link_with_peer_count_and_locked() {
        let mut app = make_app();
        app.link_enabled = true;
        app.link_peers = 2;
        let text = render(&app);
        assert!(text.contains("LINK"), "expected LINK, got: {text:?}");
        assert!(text.contains('2'), "expected peer count 2, got: {text:?}");
        assert!(text.contains("LOCKED"), "expected LOCKED with peers>0, got: {text:?}");
    }

    #[test]
    fn shows_link_off_when_disabled() {
        let app = make_app();
        let text = render(&app);
        assert!(text.contains("LINK"), "expected LINK, got: {text:?}");
        assert!(text.contains("off"), "expected 'off' when disabled, got: {text:?}");
        assert!(!text.contains("LOCKED"), "should not show LOCKED, got: {text:?}");
    }

    #[test]
    fn shows_bar_beat_sixteenth_position() {
        let mut app = make_app();
        app.bar = 0;
        app.playhead = 0;
        let text = render(&app);
        assert!(text.contains("001.1.1"), "expected 001.1.1, got: {text:?}");

        // bar=2, playhead=9 -> beat=(9/4)%4+1=3, sixteenth=9%4+1=2 -> 003.3.2
        app.bar = 2;
        app.playhead = 9;
        let text = render(&app);
        assert!(text.contains("003.3.2"), "expected 003.3.2, got: {text:?}");
    }

    #[test]
    fn shows_swing_percentage() {
        let mut app = make_app();
        app.set.swing = 0.56;
        let text = render(&app);
        assert!(text.contains("SW"), "expected SW label, got: {text:?}");
        assert!(text.contains("56%"), "expected 56%, got: {text:?}");
    }

    #[test]
    fn shows_saved_when_clean() {
        let app = make_app();
        assert!(!app.dirty());
        let text = render(&app);
        assert!(text.contains("SAVED"), "expected SAVED, got: {text:?}");
        assert!(!text.contains("EDITED"), "should not show EDITED, got: {text:?}");
    }

    #[test]
    fn shows_edited_when_dirty() {
        let mut app = make_app();
        app.apply(Action::ToggleStep); // snapshot -> dirty = true
        assert!(app.dirty());
        let text = render(&app);
        assert!(text.contains("EDITED"), "expected EDITED, got: {text:?}");
        assert!(!text.contains("SAVED"), "should not show SAVED, got: {text:?}");
    }

    #[test]
    fn shows_status_toast_when_set() {
        let mut app = make_app();
        app.status = "Saved".to_string();
        let text = render(&app);
        assert!(text.contains("Saved"), "expected toast 'Saved', got: {text:?}");
    }

    #[test]
    fn shows_various_status_messages() {
        let mut app = make_app();
        for msg in &["Loaded dub #07", "Velocity 96", "Press q again to quit"] {
            app.status = msg.to_string();
            let text = render(&app);
            assert!(text.contains(msg), "expected toast {msg:?}, got: {text:?}");
        }
    }

    #[test]
    fn tempo_entry_mode_shows_typed_buffer() {
        let mut app = make_app();
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('1'));
        app.apply(Action::TempoDigit('2'));
        assert_eq!(app.mode, Mode::TempoEntry);
        assert_eq!(app.tempo_input, "12");
        let text = render(&app);
        assert!(text.contains("BPM"), "expected BPM prompt, got: {text:?}");
        assert!(text.contains("12"), "expected typed digits, got: {text:?}");
    }
}
