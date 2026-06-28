//! midip — terminal MIDI pattern sequencer. Entry point + terminal lifecycle.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use midip::app::App;
use midip::devices::profiles::{default_profiles, DeviceProfile};
use midip::engine::spawn_engine;
use midip::link::AbletonLink;
use midip::midi::ports::{connect, list_output_ports, match_port, MidiSink, NullSink};
use midip::pattern::library::{GenreMap, Library};
use midip::pattern::model::Set;

/// Map each profile to a detected output-port index by its `port_match` substring.
/// `None` means no device matched that profile (caller substitutes a fallback sink).
/// Pure — unit-tested without hardware.
fn select_sinks(available: &[String], profiles: &[DeviceProfile]) -> Vec<Option<usize>> {
    profiles
        .iter()
        .map(|p| match_port(available, p.port_match))
        .collect()
}

/// RAII guard that restores the terminal on drop (covers the error path too).
struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let _guard = TermGuard; // restores even if run() errors / panics-unwinds
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    run(terminal)
}

fn run(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let profiles = default_profiles();

    // Library (fall back to empty with a status note).
    let (library, lib_status) = match Library::load(&midip::config::patterns_dir()) {
        Ok(lib) => (lib, String::from("library loaded")),
        Err(e) => (
            Library { drums: GenreMap::new(), bass: GenreMap::new(), synth: GenreMap::new() },
            format!("library load failed: {e} (running with empty library)"),
        ),
    };

    // Auto-detect ports and build one sink per lane.
    let available = list_output_ports();
    let picks = select_sinks(&available, &profiles);
    let mut sinks: Vec<Box<dyn MidiSink>> = Vec::with_capacity(picks.len());
    for (i, pick) in picks.iter().enumerate() {
        let sink: Box<dyn MidiSink> = match pick {
            Some(_) => match connect(profiles[i].port_match) {
                Ok(s) => Box::new(s),
                Err(_) => Box::new(NullSink),
            },
            None => Box::new(NullSink),
        };
        sinks.push(sink);
    }

    let set = Set::default_set(profiles);
    let link = Box::new(AbletonLink::new(set.bpm));
    let engine = spawn_engine(set.clone(), link, sinks);

    let mut app = App::new(set, library);
    app.status = lib_status;

    loop {
        terminal.draw(|f| midip::ui::render(f, &app))?;

        // Input: poll with ~16ms timeout for ~60fps responsiveness.
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                let action = midip::input::key_to_action(key, app.mode, app.focused_kind());
                let cmds = app.apply(action);
                for cmd in cmds {
                    let _ = engine.tx.send(cmd);
                }
            }
        }

        // Drain engine events into display state.
        while let Ok(ev) = engine.rx.try_recv() {
            app.on_engine_event(ev);
        }

        if app.should_quit {
            let _ = engine.tx.send(midip::engine::UiCommand::Quit);
            break;
        }
    }

    // Best-effort join (guard restores the terminal regardless).
    let _ = engine.join.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::select_sinks;
    use midip::devices::profiles::default_profiles;

    #[test]
    fn select_sinks_maps_profiles_to_detected_ports() {
        // Two ports present: a T-8 and an S-1; profiles are [T8_DRUMS, T8_BASS, S1].
        let available = vec![
            "Roland T-8 Bus 1".to_string(),
            "Roland S-1 Bus 1".to_string(),
        ];
        let profiles = default_profiles();
        let picks = select_sinks(&available, &profiles);
        assert_eq!(picks.len(), 3);
        // T8_DRUMS and T8_BASS both match "T-8" -> index 0.
        assert_eq!(picks[0], Some(0));
        assert_eq!(picks[1], Some(0));
        // S1 matches "S-1" -> index 1.
        assert_eq!(picks[2], Some(1));
    }

    #[test]
    fn select_sinks_returns_none_when_absent() {
        let available: Vec<String> = vec![];
        let profiles = default_profiles();
        let picks = select_sinks(&available, &profiles);
        assert_eq!(picks, vec![None, None, None]);
    }
}
