//! Keybinding help overlay.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
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

/// Build the left-column content (Transport / Edit / Drums / Melodic / Per-step).
fn left_column_lines() -> Vec<Line<'static>> {
    vec![
        // ── Transport ────────────────────────────────────────────────
        header("Transport"),
        row("[space]        play / stop"),
        row("[esc]          panic / all-notes-off"),
        row("[!]            full MIDI panic"),
        row("[t]  type BPM   [T]  tap tempo   [k]  Link"),
        row("[; / ']  BPM −/+   [< / >]  swing"),
        row("[{ / }]  pattern len   [L]  double length"),
        blank(),
        // ── Edit ─────────────────────────────────────────────────────
        header("Edit"),
        row("[tab / shift+tab]  next / prev lane"),
        row("[enter]  toggle step / place note"),
        row("[0-9]  velocity bucket   [+ / -]  fine vel"),
        row("[p / P]  probability   [y / Y]  ratchet"),
        row("[x c v]  cut/copy/paste   [r / R]  rotate"),
        row("[del]  clear step"),
        blank(),
        // ── Drums ─────────────────────────────────────────────────────
        header("Drums"),
        row("[← →]  step   [↑ ↓]  voice row"),
        row("[e / E]  euclid pulses +/−"),
        row("[[ / ]]  euclid rotation"),
        blank(),
        // ── Melodic ───────────────────────────────────────────────────
        header("Melodic"),
        row("[← →]  step cursor   [↑ ↓]  pitch"),
        row("[g]  slide   [, / .]  note len"),
        row("[[ / ]]  octave"),
        blank(),
        // ── Chords  (poly lanes, e.g. S-1) ────────────────────────────
        header("Chords  (poly lanes)"),
        row("[j]  build scale-aware triad (root + 3rd + 5th)"),
        row("[J]  remove last note from the chord step"),
        row("note-input on a poly lane STACKS keys into a chord"),
        row("  (repeat a key to toggle that pitch off)"),
        blank(),
        // ── Scales ────────────────────────────────────────────────────
        header("Scales  (Melodic mode)"),
        row("[n / N]  cycle scale fwd / bwd"),
        row("[h / H]  root down / up"),
        row("[X]  conform notes to scale"),
        row("[I]  note-input (piano) sub-mode"),
        blank(),
        // ── Note input (I) ────────────────────────────────────────────
        header("Note input  [I] to open"),
        row("[a s d f g h j k]  white keys"),
        row("[w e t y u]  black keys"),
        row("[z / x]  octave down / up"),
        row("[Bksp]  clear   [Esc]  exit"),
        row("mono lane: replace + advance · poly lane: stack chord"),
        blank(),
        // ── Per-step ──────────────────────────────────────────────────
        header("Per-step"),
        row("[p / P]  probability up / down"),
        row("[y / Y]  ratchet up / down"),
    ]
}

/// Build the right-column content (Global / Routing / Library / Set Manager / Pattern).
fn right_column_lines() -> Vec<Line<'static>> {
    vec![
        // ── Global ────────────────────────────────────────────────────
        header("Global"),
        row("[ctrl+z] / [u]  undo   [ctrl+y]  redo"),
        row("[m]  mute   [S]  solo   [M]  mirror output"),
        row("[l]  library   [o]  open set   [s]  save"),
        row("[?]  help   [q]  quit (twice while playing)"),
        blank(),
        // ── Routing / Performance ─────────────────────────────────────
        header("Routing / Performance"),
        row("[w]  route editor (port / channel / clock-out)"),
        row("[b]  launch quant: next bar / next beat"),
        row("[C]  cancel queued launch on focused lane"),
        row("[i]  restart lane phase at next bar/beat (re-sync)"),
        row("[f]  toggle fill on focused lane (non-destructive, latched)"),
        row("[F]  commit fill — make it permanent (undoable)"),
        blank(),
        // ── Library  [l] ──────────────────────────────────────────────
        header("Library  [l] to open"),
        row("[enter]  load pattern (queues if playing)"),
        row("[a]  audition   [b]  launch quant   [C]  cancel"),
        row("[esc / l]  close"),
        blank(),
        // ── Set Manager  [o] ──────────────────────────────────────────
        header("Set Manager  [o] to open"),
        row("[enter]  load set"),
        row("[r]  rename   [a / S]  save-as   [D]  duplicate"),
        row("[d]  delete   [n]  new set"),
        row("[esc / o]  close"),
        blank(),
        // ── Pattern management ────────────────────────────────────────
        header("Pattern  (Edit mode)"),
        row("[A]  save lane as user pattern (name dialog)"),
        row("[Z]  clear focused lane pattern"),
        blank(),
        // ── Route Editor  [w] ─────────────────────────────────────────
        header("Route Editor  [w] to open"),
        row("[↑ ↓]  select lane"),
        row("[← →]  move between fields"),
        row("[c / C]  cycle port fwd / bwd"),
        row("[[ / ]]  channel −1 / +1  (1-based, 1-16)"),
        row("[z]  toggle MIDI clock output"),
        row("[esc]  close"),
        blank(),
        // ── Crate / Live  [V] ─────────────────────────────────────────
        header("Crate / Live  [V] to open"),
        row("[↑ ↓]  select entry   [← →]  change crate"),
        row("[enter]  launch (quantized, role-matched)"),
        row("[a]  audition entry   [C]  cancel queue"),
        row("[z]  validate crate   [V / esc]  close"),
        blank(),
        // ── Favorites ─────────────────────────────────────────────────
        header("Favorites  (Library mode)"),
        row("[f]  toggle favorite on selected pattern"),
        row("[F]  toggle favorites-only filter"),
    ]
}

/// Render the help overlay into `area` (caller clears/positions it).
///
/// `scroll` is the current scroll offset in lines; this function clamps it
/// to a valid range internally and renders the appropriate slice.
pub fn render_help(f: &mut Frame, area: Rect, scroll: u16) {
    f.render_widget(Clear, area);

    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        " CONTROLS ",
        Style::default().add_modifier(Modifier::BOLD),
    ));

    // Compute inner rect manually (border = 1 on each side).
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    // Render block border first.
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // Split inner: content above, hint line at bottom.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let content_area = split[0];
    let hint_area = split[1];

    // Two equal columns for content.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(content_area);

    let left_lines = left_column_lines();
    let right_lines = right_column_lines();

    let left_total = left_lines.len() as u16;
    let right_total = right_lines.len() as u16;
    let col_height = cols[0].height;

    let left_max = left_total.saturating_sub(col_height);
    let right_max = right_total.saturating_sub(col_height);
    let max_scroll = left_max.max(right_max);

    let effective_scroll = scroll.min(max_scroll);

    let show_up = effective_scroll > 0;
    let show_down = effective_scroll < max_scroll;

    f.render_widget(
        Paragraph::new(left_lines).scroll((effective_scroll, 0)),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(right_lines).scroll((effective_scroll, 0)),
        cols[1],
    );

    let indicators = match (show_up, show_down) {
        (true, true) => " ▲▼ ",
        (true, false) => " ▲  ",
        (false, true) => "  ▼ ",
        (false, false) => "    ",
    };
    let hint = if max_scroll > 0 {
        format!("{indicators}↑↓ PgUp/PgDn scroll · ? close")
    } else {
        "? close".to_string()
    };
    f.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_help_to_string(w: u16, h: u16, scroll: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_help(f, f.area(), scroll)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn help_shows_play_hint() {
        let whole = render_help_to_string(110, 40, 0);
        assert!(whole.contains("play"), "expected play hint, got: {whole:?}");
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
        let whole = render_help_to_string(110, 75, 0);
        assert!(
            whole.contains("Transport"),
            "expected Transport group header"
        );
        assert!(whole.contains("panic"), "expected panic in Transport group");
        assert!(whole.contains("Drums"), "expected Drums group header");
        assert!(whole.contains("euclid"), "expected euclid in Drums group");
        assert!(whole.contains("Melodic"), "expected Melodic group header");
        assert!(whole.contains("slide"), "expected slide in Melodic group");
        assert!(whole.contains("Per-step"), "expected Per-step group header");
        assert!(
            whole.contains("probability"),
            "expected probability in Per-step"
        );
        assert!(whole.contains("ratchet"), "expected ratchet in Per-step");
        assert!(whole.contains("Global"), "expected Global group header");
        assert!(whole.contains("undo"), "expected undo in Global group");
        assert!(whole.contains("open"), "expected open in Global group");
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
        let whole = render_help_to_string(110, 75, 0);
        assert!(
            whole.contains("[w]"),
            "expected [w] route editor key; got: {whole:?}"
        );
        assert!(
            whole.contains("route editor"),
            "expected 'route editor' description; got: {whole:?}"
        );
    }

    #[test]
    fn help_scroll_reveals_bottom_content() {
        let short_scroll0 = render_help_to_string(110, 12, 0);
        let short_scrolled = render_help_to_string(110, 12, 999);
        // At max scroll, the bottom-most groups (Crate/Live and Favorites) appear.
        assert!(
            short_scrolled.contains("Favorites") || short_scrolled.contains("Crate"),
            "scrolled view should show bottom sections (Crate/Favorites); got: {short_scrolled:?}"
        );
        assert_ne!(
            short_scroll0, short_scrolled,
            "scroll=0 and scroll=999 should produce different output"
        );
    }

    #[test]
    fn help_shows_mirror_and_double_length_keys() {
        let whole = render_help_to_string(110, 75, 0);
        assert!(
            whole.contains("[M]"),
            "expected [M] mirror key in help; got: {whole:?}"
        );
        assert!(
            whole.contains("[L]"),
            "expected [L] double-length key in help; got: {whole:?}"
        );
    }

    /// Fix 3: crate-view and favorites keys must appear in the help overlay.
    #[test]
    fn help_shows_crate_view_keys() {
        // Tall enough to render all groups without scrolling.
        let whole = render_help_to_string(110, 100, 0);
        assert!(
            whole.contains("Crate"),
            "expected 'Crate' group header in help; got: {whole:?}"
        );
        assert!(
            whole.contains("[V]"),
            "expected [V] open-crate key in help; got: {whole:?}"
        );
        assert!(
            whole.contains("audition"),
            "expected audition hint in crate-view group; got: {whole:?}"
        );
        assert!(
            whole.contains("validate"),
            "expected validate hint in crate-view group; got: {whole:?}"
        );
        assert!(
            whole.contains("Favorites"),
            "expected 'Favorites' group header in help; got: {whole:?}"
        );
        assert!(
            whole.contains("[f]"),
            "expected [f] favorite key in help; got: {whole:?}"
        );
        assert!(
            whole.contains("[F]"),
            "expected [F] favorites-filter key in help; got: {whole:?}"
        );
    }

    /// M5a: scale keys and note-input sub-mode must appear in the help overlay.
    #[test]
    fn help_shows_scale_and_note_input_keys() {
        // Tall enough to render all groups without scrolling.
        let whole = render_help_to_string(110, 120, 0);
        assert!(
            whole.contains("cycle scale"),
            "expected 'cycle scale' hint; got: {whole:?}"
        );
        assert!(
            whole.contains("[n / N]"),
            "expected [n / N] scale keys; got: {whole:?}"
        );
        assert!(
            whole.contains("[h / H]"),
            "expected [h / H] root keys; got: {whole:?}"
        );
        assert!(
            whole.contains("conform notes"),
            "expected 'conform notes' hint; got: {whole:?}"
        );
        assert!(
            whole.contains("[X]"),
            "expected [X] conform key; got: {whole:?}"
        );
        assert!(
            whole.contains("note-input"),
            "expected 'note-input' sub-mode hint; got: {whole:?}"
        );
        assert!(
            whole.contains("[I]"),
            "expected [I] note-input key; got: {whole:?}"
        );
        assert!(
            whole.contains("white keys"),
            "expected 'white keys' piano hint; got: {whole:?}"
        );
        assert!(
            whole.contains("black keys"),
            "expected 'black keys' piano hint; got: {whole:?}"
        );
        assert!(
            whole.contains("Bksp"),
            "expected Bksp clear hint; got: {whole:?}"
        );
    }

    /// M5b Task 4: chord-entry keys and poly-stacking note must appear in the help overlay.
    #[test]
    fn help_shows_chord_keys_and_poly_stacking() {
        // Tall enough to render all groups without scrolling.
        let whole = render_help_to_string(110, 130, 0);
        assert!(
            whole.contains("Chords"),
            "expected 'Chords' group header; got: {whole:?}"
        );
        assert!(
            whole.contains("triad"),
            "expected 'triad' hint for [j]; got: {whole:?}"
        );
        assert!(
            whole.contains("[j]"),
            "expected [j] build-triad key; got: {whole:?}"
        );
        assert!(
            whole.contains("[J]"),
            "expected [J] remove-note key; got: {whole:?}"
        );
        assert!(
            whole.contains("stack"),
            "expected note-input poly-stacking hint; got: {whole:?}"
        );
    }
}
