//! Always-visible transport header.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::ui::theme::lane_color;

/// Render the transport header into `area`.
pub fn render_transport(f: &mut Frame, area: Rect, app: &App) {
    let play = if app.playing { "▶ PLAY" } else { "■ STOP" };

    // Additive accent: the play indicator wears the focused lane's static color (spec §7).
    // Text content is unchanged; degrades to monochrome automatically without color support.
    let accent = lane_color(app.focused_lane().profile.id);

    let bpm_display = if app.mode == Mode::TempoEntry {
        format!("BPM: {}_", app.tempo_input)
    } else {
        let bpm = if app.link_enabled { app.link_tempo } else { app.set.bpm };
        format!("♩={bpm:.1}")
    };

    let link = if app.link_enabled {
        format!("⟲ LINK ●{}", app.link_peers)
    } else {
        "⟲ link  ○".to_string()
    };

    // beat within bar from the playhead (16 steps -> 4 beats), 1-indexed.
    let beat = (app.playhead / 4) % 4 + 1;
    let bar = app.bar;

    let swing_pct = (app.set.swing * 100.0).round() as i32;

    let line = Line::from(vec![
        Span::styled(play, Style::default().fg(accent).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::raw(bpm_display),
        Span::raw("   "),
        Span::raw(link),
        Span::raw("   "),
        Span::raw(format!("bar {bar:03}·{beat}")),
        Span::raw("   "),
        Span::raw("4/4"),
        Span::raw("   "),
        Span::raw(format!("swing {swing_pct}%")),
    ]);

    let block = Block::default().borders(Borders::ALL).title(" midip ");
    let para = Paragraph::new(line).block(block);
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
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() }
    }

    fn buffer_text(t: &Terminal<TestBackend>) -> String {
        let buf = t.backend().buffer();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn transport_shows_play_bpm_and_link() {
        let mut set = Set::default_set(default_profiles());
        set.bpm = 124.0;
        let mut app = App::new(set, empty_library());
        app.playing = true;
        app.link_enabled = true;
        app.link_tempo = 124.0;
        app.link_peers = 2;

        let backend = TestBackend::new(92, 3);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_transport(f, f.area(), &app)).unwrap();

        let text = buffer_text(&term);
        assert!(text.contains("PLAY"), "expected PLAY, got: {text:?}");
        assert!(text.contains("124"), "expected bpm 124, got: {text:?}");
        assert!(text.contains("LINK"), "expected LINK, got: {text:?}");
    }

    #[test]
    fn transport_shows_bpm_entry_prompt_in_tempo_entry_mode() {
        use crate::app::{Action, Mode};
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.apply(Action::OpenTempo);
        app.apply(Action::TempoDigit('1'));
        app.apply(Action::TempoDigit('2'));
        assert_eq!(app.mode, Mode::TempoEntry);
        assert_eq!(app.tempo_input, "12");

        let backend = TestBackend::new(92, 3);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_transport(f, f.area(), &app)).unwrap();
        let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("12"), "expected typed digits in transport, got: {text:?}");
        assert!(text.contains("BPM"), "expected BPM prompt in transport, got: {text:?}");
    }

    #[test]
    fn transport_shows_stop_when_not_playing() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        let backend = TestBackend::new(92, 3);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_transport(f, f.area(), &app)).unwrap();
        assert!(buffer_text(&term).contains("STOP"));
    }
}
