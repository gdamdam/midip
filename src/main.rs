//! midip — terminal MIDI pattern sequencer. Entry point + terminal lifecycle.

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use midip::app::App;
use midip::devices::profiles::default_profiles;
use midip::engine::spawn_engine;
use midip::link::AbletonLink;
use midip::pattern::library::Library;
use midip::pattern::model::Set;

// select_sinks / unique_ports are pure helpers used only in tests below.
// They live here (not in a library module) because they were originally part of
// the port-connection logic that has since moved into the engine thread.
#[cfg(test)]
use midip::devices::profiles::DeviceProfile;
#[cfg(test)]
use midip::midi::ports::match_port;

/// Map each profile to a detected output-port index by its `port_match` substring.
/// `None` means no device matched that profile.
/// Pure — unit-tested without hardware.
#[cfg(test)]
fn select_sinks(available: &[String], profiles: &[DeviceProfile]) -> Vec<Option<usize>> {
    profiles
        .iter()
        .map(|p| match_port(available, p.port_match))
        .collect()
}

/// Distinct output-port indices in first-seen order (deduplication of shared ports).
#[cfg(test)]
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

/// RAII guard: on scope exit — including a `?` error return or a panic unwind —
/// tell the engine to Quit (its handler runs `seq.panic`, releasing every sounding
/// note) and join it so the MIDI flush completes before the process exits.
struct EngineQuitGuard {
    tx: crossbeam_channel::Sender<midip::engine::UiCommand>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for EngineQuitGuard {
    fn drop(&mut self) {
        let _ = self.tx.send(midip::engine::UiCommand::Quit);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn main() -> Result<()> {
    // L1: a panic mid-run would otherwise print to the alternate screen, which the
    // TermGuard then wipes — the message is lost and the terminal state confusing.
    // Restore the terminal FIRST, then let the default hook print to a usable screen.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));
    enable_raw_mode()?;
    let _guard = TermGuard; // restores even if run() errors / panics-unwinds
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    run(terminal)
}

/// Send a command to the engine; on channel failure set a visible status toast.
fn send_or_toast(
    tx: &crossbeam_channel::Sender<midip::engine::UiCommand>,
    cmd: midip::engine::UiCommand,
    app: &mut App,
) {
    if tx.send(cmd).is_err() {
        app.set_status("engine unavailable");
    }
}

fn run(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    // Layer the user's optional devices.json over the shipped catalog before any
    // profile lookup (the device picker and saved-set loading read the catalog).
    midip::devices::profiles::init_user_catalog(&midip::config::data_dir());
    let profiles = default_profiles();

    // Library (fall back to empty with a status note).
    let (library, lib_status) = match Library::load(&midip::config::patterns_dir()) {
        Ok(lib) => (lib, String::from("library loaded")),
        Err(e) => (
            Library::empty(),
            format!("library load failed: {e} (running with empty library)"),
        ),
    };

    // The engine now owns the full sink lifecycle: it connects ports at startup,
    // emits one DeviceStatus per lane (populating app.device_status via on_engine_event),
    // and rescans every ~1 s for hot-plug / send-failure. main.rs no longer builds sinks
    // or sets app.device_status directly.
    let set = Set::default_set(profiles);
    let link = Box::new(AbletonLink::new(set.bpm));
    let engine = spawn_engine(set.clone(), link);
    // L1: release sounding notes on EVERY exit path — the guard sends Quit (engine
    // runs its all-notes-off panic flush) and joins, even when run() returns an
    // error via `?` or unwinds from a panic. The clean-quit path below still sends
    // its own Quit first; the duplicate is harmless.
    let _engine_quit = EngineQuitGuard {
        tx: engine.tx.clone(),
        join: Some(engine.join),
    };

    let mut app = App::new(set, library);

    // Load persisted mirror preference and sync engine if on.
    let (prefs, prefs_note) = midip::pattern::store::load_prefs(&midip::config::data_dir());
    app.mirror_on = prefs.mirror_on;
    if prefs.mirror_on {
        send_or_toast(
            &engine.tx,
            midip::engine::UiCommand::SetMirror(true),
            &mut app,
        );
    }

    // Load persisted favorites.
    let (favorites, fav_note) = midip::pattern::store::load_favorites(&midip::config::data_dir());
    app.favorites = favorites;

    // Load persisted crates.
    let (crates, crates_note) = midip::pattern::store::load_crates(&midip::config::data_dir());
    app.crates = crates;

    app.set_status(lib_status);
    // M7: a corrupt sidecar file was quarantined as .bak instead of being silently
    // reset — surface that so the performer knows their data survived.
    for note in [prefs_note, fav_note, crates_note].into_iter().flatten() {
        app.set_status(note);
    }

    // Crash detection: if a recovery file exists with no clean-shutdown marker,
    // the previous run was killed or crashed — prompt the performer to recover.
    let dir = midip::config::data_dir();
    if midip::pattern::store::unclean_shutdown_detected(&dir) {
        app.mode = midip::app::Mode::RecoveryPrompt;
    }
    // Always remove the clean marker at startup so that if THIS run crashes,
    // the absence of the marker will be detected on next launch.
    midip::pattern::store::clear_clean_marker(&dir);

    loop {
        terminal.draw(|f| midip::ui::render(f, &app))?;

        // Input: poll with ~16ms timeout for ~60fps responsiveness.
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                // On Windows, crossterm emits both Press and Release; only act on Press
                // so each keystroke triggers its action once. (Unix reports only Press.)
                if key.kind == KeyEventKind::Press {
                    let action =
                        midip::input::key_to_action(key, app.mode.clone(), app.focused_kind());
                    let cmds = app.apply(action);
                    for cmd in cmds {
                        send_or_toast(&engine.tx, cmd, &mut app);
                    }
                }
            }
        }

        // Drain engine events into display state. Some events (Playhead bar boundaries)
        // drive chain auto-advance, which emits commands the engine must receive.
        while let Ok(ev) = engine.rx.try_recv() {
            let cmds = app.on_engine_event(ev);
            for cmd in cmds {
                send_or_toast(&engine.tx, cmd, &mut app);
            }
        }

        // Expire status toasts after ~3 s (STATUS_TTL_FRAMES × 16 ms poll timeout).
        app.tick_status();

        // Debounced autosave to a separate recovery file (never overwrites deliberate saves).
        // Best-effort: a failed autosave is silently dropped so it never disrupts the UI thread.
        if app.tick_autosave() {
            if let Err(e) = midip::pattern::store::save_recovery(
                &midip::config::data_dir(),
                &app.committed_set(),
            ) {
                app.set_status(format!("autosave failed: {e}"));
            }
        }

        if app.should_quit {
            send_or_toast(&engine.tx, midip::engine::UiCommand::Quit, &mut app);
            // Clean exit: write marker so next launch doesn't show recovery prompt.
            let _ = midip::pattern::store::mark_clean_shutdown(&dir);
            midip::pattern::store::clear_recovery(&dir);
            break;
        }
    }

    // EngineQuitGuard sends Quit + joins on drop (guard restores the terminal regardless).
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
