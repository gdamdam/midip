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
use midip::pattern::library::Library;
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

/// Distinct output-port indices to open a connection to, in first-seen order.
/// Lanes that target the same physical port (e.g. the T-8's drum + bass lanes both
/// match "T-8") collapse to a SINGLE entry: the engine fans every message — including
/// the 24 PPQN MIDI clock — out to every open connection, so two connections to one
/// device would deliver the clock twice and the device would read double tempo.
fn unique_ports(picks: &[Option<usize>]) -> Vec<usize> {
    let mut out = Vec::new();
    for &idx in picks.iter().flatten() {
        if !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
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
    let _guard = TermGuard; // restores even if run() errors / panics-unwinds
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
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
            Library::empty(),
            format!("library load failed: {e} (running with empty library)"),
        ),
    };

    // Auto-detect ports and open ONE connection per unique physical port. Lanes that
    // share a port (T-8 drums + bass) collapse to a single connection — otherwise the
    // fanned-out MIDI clock would reach the device twice and double its tempo.
    let available = list_output_ports();
    let picks = select_sinks(&available, &profiles);
    let mut sinks: Vec<Box<dyn MidiSink>> = Vec::new();
    let mut connected_ports: Vec<usize> = Vec::new();
    for port_idx in unique_ports(&picks) {
        // Any profile that resolved to this port shares the same `port_match`/physical
        // device; use the first one to open the single connection.
        if let Some(i) = picks.iter().position(|p| *p == Some(port_idx)) {
            if let Ok(s) = connect(profiles[i].port_match) {
                sinks.push(Box::new(s));
                connected_ports.push(port_idx);
            }
        }
    }
    // No hardware detected (or all connections failed): keep one no-op sink so the
    // engine still runs (silent) without special-casing an empty fan-out.
    if sinks.is_empty() {
        sinks.push(Box::new(NullSink));
    }

    // Per-lane connection status for the lane-overview `●/○`: a lane is connected iff its
    // matched physical port actually opened. (Hot-plug after startup is future work, via
    // the EngineEvent::DeviceStatus path.)
    let device_status: Vec<(bool, String)> = picks
        .iter()
        .map(|pick| match pick {
            Some(idx) if connected_ports.contains(idx) => (true, available[*idx].clone()),
            _ => (false, String::new()),
        })
        .collect();

    let set = Set::default_set(profiles);
    let link = Box::new(AbletonLink::new(set.bpm));
    let engine = spawn_engine(set.clone(), link, sinks);

    let mut app = App::new(set, library);
    app.status = lib_status;
    app.device_status = device_status;

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

    #[test]
    fn unique_ports_dedupes_shared_physical_port() {
        use super::unique_ports;
        // T-8 drums + T-8 bass both resolve to port 0; S-1 to port 1.
        // Each PHYSICAL port must be connected exactly once — two connections to the
        // T-8 would deliver MIDI clock twice and double the device's tempo.
        assert_eq!(unique_ports(&[Some(0), Some(0), Some(1)]), vec![0, 1]);
        // No devices -> nothing to connect.
        assert_eq!(unique_ports(&[None, None, None]), Vec::<usize>::new());
        // Distinct ports preserved in first-seen order.
        assert_eq!(unique_ports(&[Some(2), Some(0), Some(2)]), vec![2, 0]);
    }
}
