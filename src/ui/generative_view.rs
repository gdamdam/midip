//! Generative tool overlay: mode selector, parameter knobs, candidate summary, live preview.

use crate::ui::theme::EMBER;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::pattern::generate::GenMode;
use crate::pattern::model::PatternData;

/// Render the generative panel overlay into `area`.
pub fn render_generative_panel(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(Clear, area);

    let p = &app.gen_params;
    let mode_label = match p.mode {
        GenMode::Generate => "Generate",
        GenMode::Vary => "Vary",
    };
    let title = format!(" GENERATE  [{mode_label}] ");
    let outer = Block::default().borders(Borders::ALL).title(Span::styled(
        title,
        Style::default()
            .fg(EMBER.synth)
            .add_modifier(Modifier::BOLD),
    ));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Count active steps in the current (previewed) focused lane pattern.
    let lane = app.focused_lane();
    let active_steps = match &lane.pattern.data {
        PatternData::Drums(steps) => steps.iter().filter(|s| !s.is_empty()).count(),
        PatternData::Melodic(steps) => steps.iter().filter(|s| !s.is_empty()).count(),
    };
    let total_steps = match &lane.pattern.data {
        PatternData::Drums(steps) => steps.len(),
        PatternData::Melodic(steps) => steps.len(),
    };

    let header_style = Style::default().fg(EMBER.warn).add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(EMBER.synth);
    let val_style = Style::default().fg(EMBER.fg).add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(EMBER.dim);

    let mut lines: Vec<Line> = Vec::new();

    // ── Key hints ──────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled("[tab]mode  ", key_style),
        Span::styled("[d/D]density  [r/R]range  [m/M]mutate  ", key_style),
        Span::styled("[z]reroll  [enter]commit  [esc]cancel", key_style),
    ]));
    lines.push(Line::from(""));

    // ── Mode ───────────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled("Mode        ", header_style),
        Span::styled(mode_label, val_style),
        Span::raw("  "),
        Span::styled(
            match p.mode {
                GenMode::Generate => "(generate from scratch)",
                GenMode::Vary => "(vary existing pattern)",
            },
            dim_style,
        ),
    ]));

    // ── Parameters ─────────────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled("Density     ", header_style),
        Span::styled(format!("{:3}", p.density), val_style),
        Span::raw("/100  "),
        Span::styled("[d]-  [D]+", key_style),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Range       ", header_style),
        Span::styled(format!("{:3}", p.range), val_style),
        Span::raw(" st   "),
        Span::styled("[r]-  [R]+", key_style),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Mutate      ", header_style),
        Span::styled(format!("{:3}", p.mutate), val_style),
        Span::raw("/100  "),
        Span::styled("[m]-  [M]+", key_style),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Seed        ", header_style),
        Span::styled(format!("{:016x}", p.seed), val_style),
        Span::raw("  "),
        Span::styled("[z]reroll", key_style),
    ]));

    lines.push(Line::from(""));

    // ── Candidate summary ──────────────────────────────────────────────────────
    let density_pct = (active_steps * 100).checked_div(total_steps).unwrap_or(0);
    lines.push(Line::from(vec![
        Span::styled("Preview     ", header_style),
        Span::styled(
            format!("{active_steps}/{total_steps} active steps"),
            val_style,
        ),
        Span::raw("  "),
        Span::styled(format!("({density_pct}% fill)"), dim_style),
    ]));

    // ── Visual step bar ────────────────────────────────────────────────────────
    if total_steps > 0 {
        let bar: String = match &lane.pattern.data {
            PatternData::Drums(steps) => steps
                .iter()
                .map(|s| if s.is_empty() { '·' } else { '█' })
                .collect(),
            PatternData::Melodic(steps) => steps
                .iter()
                .map(|s| if s.is_empty() { '·' } else { '█' })
                .collect(),
        };
        lines.push(Line::from(vec![
            Span::styled("Pattern     ", header_style),
            Span::styled(bar, val_style),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Mode};
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::Library;
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library::empty()
    }

    fn new_app() -> App {
        let set = Set::default_set(default_profiles());
        App::new(set, empty_library())
    }

    #[test]
    fn generative_panel_renders_without_panic() {
        let mut app = new_app();
        app.mode = Mode::Generative;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                render_generative_panel(f, f.area(), &app);
            })
            .unwrap();
    }
}
