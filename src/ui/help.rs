//! Keybinding help overlay.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Render the help overlay into `area` (caller clears/positions it).
pub fn render_help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = vec![
        Line::from("[space]  play / stop"),
        Line::from("[tab] / [shift+tab]  cycle lane focus"),
        Line::from("[1 2 3]  jump to lane"),
        Line::from("[← →]  move cursor   [↑ ↓]  cursor (drums) / pitch (melodic)"),
        Line::from("[enter]  toggle step / place note"),
        Line::from("[0-9]  velocity bucket   [+ / -]  fine velocity"),
        Line::from("[g]  toggle slide (melodic)   [[ / ]]  octave"),
        Line::from("[, / .]  note length   [{ / }]  pattern length"),
        Line::from("[< / >]  swing"),
        Line::from("[p / P]  step probability   [y / Y]  ratchet"),
        Line::from("[e / E]  euclid pulses (drums)   [[ / ]]  euclid rotation (drums)"),
        Line::from("[x c v]  cut / copy / paste   [r / R]  rotate   [del]  clear"),
        Line::from("[ctrl+z] / [u]  undo   [ctrl+y]  redo"),
        Line::from("[m]  mute   [S]  solo"),
        Line::from("[esc]  panic / all-notes-off (does not stop transport)"),
        Line::from("[t]  set tempo   [T]  tap   [k]  toggle Link"),
        Line::from("[l]  library   [s]  save   [?]  help   [q]  quit"),
    ];
    // Clear behind the overlay so it sits on top of the editor.
    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).title(" HELP ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn help_shows_play_hint() {
        let backend = TestBackend::new(92, 22);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render_help(f, f.area())).unwrap();
        let whole: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(whole.contains("play"), "expected play hint, got: {whole:?}");
        // New bindings are listed in the help overlay.
        assert!(whole.contains("probability"), "expected prob hint, got: {whole:?}");
        assert!(whole.contains("ratchet"), "expected ratchet hint, got: {whole:?}");
        assert!(whole.contains("euclid"), "expected euclid hint, got: {whole:?}");
        assert!(whole.contains("ctrl+z"), "expected ctrl+z undo hint, got: {whole:?}");
        assert!(whole.contains("panic"), "expected esc panic hint, got: {whole:?}");
    }
}
