//! Keybinding help overlay.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

fn header(title: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

fn row(text: &'static str) -> Line<'static> {
    Line::from(format!("  {text}"))
}

fn blank() -> Line<'static> {
    Line::from("")
}

/// Render the help overlay into `area` (caller clears/positions it).
pub fn render_help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = vec![
        // ── Transport ────────────────────────────────────────────────
        header("Transport"),
        row("[space]        play / stop"),
        row("[esc]          panic / all-notes-off (transport keeps running)"),
        row("[!]            full MIDI panic"),
        row("[t]            type BPM (Enter confirm, Esc cancel)   [; / ']  BPM −/+"),
        row("[T]            tap tempo   [k]  toggle Ableton Link"),
        row("[< / >]        swing   [{ / }]  pattern length"),
        blank(),
        // ── Edit (common) ────────────────────────────────────────────
        header("Edit"),
        row("[tab / shift+tab]  cycle lane focus (next / prev)"),
        row("[enter]        toggle step (Drums) / place note (Melodic)"),
        row("[0-9]          velocity bucket   [+ / -]  fine velocity"),
        row("[p / P]        step probability   [y / Y]  ratchet"),
        row("[x c v]        cut / copy / paste   [r / R]  rotate   [del]  clear"),
        blank(),
        // ── Drums ─────────────────────────────────────────────────────
        header("Drums"),
        row("[← →]          move cursor   [↑ ↓]  move cursor vertically"),
        row("[e / E]        euclid pulses (add/remove)   [[ / ]]  euclid rotation"),
        blank(),
        // ── Melodic ───────────────────────────────────────────────────
        header("Melodic"),
        row("[← →]          step cursor   [↑ ↓]  pitch up / down"),
        row("[g]            toggle slide   [, / .]  note length   [[ / ]]  octave"),
        blank(),
        // ── Per-step ──────────────────────────────────────────────────
        header("Per-step"),
        row("[p / P]        probability up / down"),
        row("[y / Y]        ratchet up / down"),
        blank(),
        // ── Global ────────────────────────────────────────────────────
        header("Global"),
        row("[ctrl+z] / [u]  undo   [ctrl+y]  redo"),
        row("[m]            mute lane   [S]  solo lane   [M]  mirror output"),
        row("[l]            library   [o]  open set   [s]  save"),
        row("[w]            route editor (port / channel / clock-out per lane)"),
        row("[b]            toggle launch quant: next bar / next beat"),
        row("[C]            cancel pending queued launch on focused lane"),
        row("[?]            help   [q]  quit (twice while playing)"),
        blank(),
        // ── Library ───────────────────────────────────────────────────
        header("Library  [l] to open"),
        row("[enter]        commit pattern (queues at next bar/beat when playing)"),
        row("[a]            audition (preview without committing)"),
        row("[b]            toggle launch quant: next bar / next beat"),
        row("[C]            cancel queued launch"),
        row("[esc / l]      close library"),
        blank(),
        // ── Set manager ───────────────────────────────────────────────
        header("Set Manager  [o] to open"),
        row("[enter]        load set"),
        row("[r]            rename set   [a / S]  save as new   [D]  duplicate"),
        row("[d]            delete set (confirm)   [n]  new set (confirm if unsaved)"),
        row("[esc / o]      close"),
        blank(),
        // ── Pattern management ────────────────────────────────────────
        header("Pattern  (Edit mode)"),
        row("[A]            save focused lane as user pattern (name dialog)"),
        row("[Z]            clear focused lane pattern (confirm if material)"),
        blank(),
        // ── Route editor ──────────────────────────────────────────────
        header("Route Editor  [w] to open"),
        row("[↑ ↓]          select lane"),
        row("[← →]          move between fields (Port / Channel / Clock-out)"),
        row("[c / C]        cycle port forward / backward  (+ / − through available ports)"),
        row("[[ / ]]        channel −1 / +1  (display: 1-based, range 1-16)"),
        row("[z]            toggle MIDI clock output on/off for the lane"),
        row("[esc]          close route editor"),
    ];
    // Clear behind the overlay so it sits on top of the editor.
    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " CONTROLS ",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_help_to_string(w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_help(f, f.area())).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn help_shows_play_hint() {
        // Use a tall enough terminal so all content sections are rendered.
        let whole = render_help_to_string(110, 40);
        assert!(whole.contains("play"), "expected play hint, got: {whole:?}");
        // New bindings are listed in the help overlay.
        assert!(
            whole.contains("probability"),
            "expected prob hint, got: {whole:?}"
        );
        assert!(
            whole.contains("ratchet"),
            "expected ratchet hint, got: {whole:?}"
        );
        assert!(
            whole.contains("euclid"),
            "expected euclid hint, got: {whole:?}"
        );
        assert!(
            whole.contains("ctrl+z"),
            "expected ctrl+z undo hint, got: {whole:?}"
        );
        assert!(
            whole.contains("panic"),
            "expected esc panic hint, got: {whole:?}"
        );
    }

    #[test]
    fn help_lists_all_groups() {
        // Must be wide/tall enough to show all grouped content (including set manager / pattern sections).
        let whole = render_help_to_string(110, 75);
        // Transport group
        assert!(
            whole.contains("Transport"),
            "expected Transport group header"
        );
        assert!(whole.contains("panic"), "expected panic in Transport group");
        // Edit/Drums group
        assert!(whole.contains("Drums"), "expected Drums group header");
        assert!(whole.contains("euclid"), "expected euclid in Drums group");
        // Edit/Melodic group
        assert!(whole.contains("Melodic"), "expected Melodic group header");
        assert!(whole.contains("slide"), "expected slide in Melodic group");
        // Per-step group
        assert!(whole.contains("Per-step"), "expected Per-step group header");
        assert!(
            whole.contains("probability"),
            "expected probability in Per-step"
        );
        assert!(whole.contains("ratchet"), "expected ratchet in Per-step");
        // Global/misc group
        assert!(whole.contains("Global"), "expected Global group header");
        assert!(whole.contains("undo"), "expected undo in Global group");
        assert!(whole.contains("open"), "expected open in Global group");
        // Route editor group
        assert!(
            whole.contains("Route Editor"),
            "expected Route Editor group header"
        );
        assert!(
            whole.contains("clock"),
            "expected clock hint in Route Editor group"
        );
    }

    #[test]
    fn help_shows_route_editor_key_and_controls() {
        let whole = render_help_to_string(110, 75);
        assert!(
            whole.contains("[w]"),
            "expected [w] route editor key; got: {whole:?}"
        );
        assert!(
            whole.contains("route editor"),
            "expected 'route editor' description; got: {whole:?}"
        );
    }
}
