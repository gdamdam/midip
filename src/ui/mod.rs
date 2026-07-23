pub mod chain_view;
pub mod clock_in_selector;
pub mod command_palette;
pub mod crate_view;
pub mod device_picker;
pub mod editor_drums;
pub mod editor_melodic;
pub mod generative_view;
pub mod help;
pub mod lanes;
pub mod library;
pub mod mgmt;
pub mod onboarding;
pub mod recovery;
pub mod route_editor;
pub mod scene_view;
pub mod tab_strip;
pub mod theme;
pub mod transport;

use crate::ui::theme::EMBER;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{Action, App, Overlay, Workspace};
use crate::commands;
use crate::pattern::model::LaneKind;

/// Build the footer hint text for the current `(workspace, overlay)` state —
/// the authoritative pair (see `App::sync_mode`), not the derived `Mode`
/// shadow. Overlays fully own the keymap while raised (their legends stay
/// hand-authored below: registry entries don't carry overlay membership, see
/// `commands.rs`'s module doc). The bare-workspace branch instead sources its
/// discrete accelerator-bound commands from the registry via
/// [`workspace_more_hint`], so those labels can never drift from `input.rs`.
fn footer_hint(app: &App) -> String {
    if let Some(overlay) = &app.overlay {
        return match overlay {
            Overlay::Help => "[tab]detail [?/esc]close".to_string(),
            Overlay::Onboarding => "[enter/→]next [esc]skip".to_string(),
            Overlay::TempoEntry => "[0-9]type BPM [enter]set [esc]cancel".to_string(),
            Overlay::NameEntry(_) => {
                "[a-z 0-9 - #]type name [enter]confirm [esc]cancel".to_string()
            }
            Overlay::Confirm(_) => "[y/enter]yes [n/esc]no".to_string(),
            Overlay::Recovery => "[r/enter]recover [d/esc]discard [o]open saved".to_string(),
            Overlay::SetBrowser => {
                "[↑↓]select [enter]open  [r]rename [a/S]save-as [D]dup [d]del [n]new  [o/esc]close"
                    .to_string()
            }
            Overlay::Chains => "[↑↓]chain [enter]play [c]create [r]rename [d]dup [x]del [m]loop [a]add [X]rm [[/]]bars [{/}]rpts [K/esc]close".to_string(),
            Overlay::ClockInSelector => "[↑↓]select port  [enter]confirm  [esc]cancel".to_string(),
            Overlay::DevicePicker => "[↑↓]device  [enter]select  [esc]cancel".to_string(),
            Overlay::NoteInput => {
                "[a-k]white [w/e/t/y/u]black [z]oct- [x]oct+ [bksp]del [esc]exit".to_string()
            }
            Overlay::Generative => {
                "[tab]mode  [d/D]density  [r/R]range  [m/M]mutate  [z]reroll  [enter]commit  [esc]cancel".to_string()
            }
            Overlay::CrateView => {
                "[↑↓]entry [←→]crate [enter]launch [a]audition [f]fav [V/esc]close".to_string()
            }
            Overlay::CommandPalette => {
                "[type]filter [↑↓]select [enter]run [esc]cancel".to_string()
            }
        };
    }

    match app.workspace {
        Workspace::Perform | Workspace::Pattern => {
            let editing = match app.focused_kind() {
                LaneKind::Drums => {
                    "[space]play [tab]lane [arrows]move [enter]toggle [0-9]vel [e/E][/]]euclid"
                }
                LaneKind::Melodic => {
                    "[space]play [tab]lane [←→]step [↑↓]pitch [enter]note [g]slide [[/]]oct"
                }
            };
            format!("{editing} {}", workspace_more_hint())
        }
        Workspace::Library => "[←→]column [↑↓]select [enter]load [esc]close".to_string(),
        Workspace::Song => {
            "[↑↓]select [enter]recall [c]capture [r]rename [d]dup [x]del [z]validate [G/esc]close"
                .to_string()
        }
        Workspace::Setup => "[↑↓]lane [←→]field [c]port [[ /]]ch [z]clk-out [esc]close".to_string(),
    }
}

/// Compact "more" suffix for the Perform/Pattern footer: a hand-picked subset
/// of the command registry (discrete, single-key, accelerator-bound
/// commands — not the continuous editing grammar covered by `editing` above).
/// Each label's accelerator is looked up live via [`commands::accel_for`], so
/// if `input.rs` ever rebinds one of these keys again (as the Task-4/5
/// workspace reroute did to `l`, `w`, `G`, …), this hint updates with it
/// instead of silently going stale.
fn workspace_more_hint() -> String {
    [
        (&Action::OpenLibrary, "lib"),
        (&Action::OpenSetBrowser, "sets"),
        (&Action::OpenRouteEditor, "routes"),
        (&Action::Save, "save"),
        (&Action::Help, "more"),
    ]
    .into_iter()
    .filter_map(|(action, label)| {
        commands::accel_for(action).map(|accel| format!("[{accel}]{label}"))
    })
    .collect::<Vec<_>>()
    .join(" ")
}

fn context_footer(app: &App) -> Line<'static> {
    let label = app.context_label();
    let hint = footer_hint(app);
    let label_style = Style::default()
        .fg(EMBER.bg)
        .bg(EMBER.synth)
        .add_modifier(Modifier::BOLD);
    Line::from(vec![
        Span::styled(format!(" {label} "), label_style),
        Span::raw(" "),
        Span::raw(hint),
    ])
}

/// Centered rect helper for overlays.
fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1]);
    h[1]
}

/// Minimum usable terminal dimensions.
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 16;

/// Top-level render: transport / lanes / editor / footer with overlay support.
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    // Full-frame background paint FIRST: fill every cell with the theme bg so
    // ratatui emits a known value for the whole frame each render. Without this,
    // cells no widget writes keep `Color::Reset` and match ratatui's own prior
    // buffer, so ratatui never emits them and the real terminal's scrollback
    // shows through (we never call `terminal.clear()`). This is the floor; panes
    // and overlays paint on top.
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(EMBER.bg)),
        area,
    );

    // Rebuild the mouse hit-map from scratch each frame: every interactive
    // widget below appends its regions in render order, so `hit_test`'s
    // last-pushed-first search resolves overlaps by z-order. Clearing here
    // (not per-pane) also drops stale grid cells when no editor renders.
    app.hits.borrow_mut().clear();

    // Guard: if the terminal is too small, show a resize prompt and bail out.
    // Attempting the normal multi-pane layout on a tiny frame produces garbage.
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let w = area.width;
        let h = area.height;
        let msg = format!(
            "midip needs at least {MIN_WIDTH}x{MIN_HEIGHT} — resize the terminal (now {w}x{h})"
        );
        let para =
            ratatui::widgets::Paragraph::new(msg).alignment(ratatui::layout::Alignment::Center);
        // Render into the full available area (however small it is).
        f.render_widget(para, area);
        return;
    }

    // Reserve the top row for the persistent workspace tab strip; everything
    // else (base panes + centered overlays) renders into the remaining area, so
    // nothing below shifts except by one row.
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    tab_strip::render(f, rows[0], app);
    let area = rows[1];

    // Base composition is workspace-specialized:
    //   • PERFORM  → transport + all-lane OVERVIEW (launch/perform focus), no editor.
    //   • PATTERN  → transport + the FOCUSED lane's step editor, no overview.
    //   • LIBRARY/SONG/SETUP → the combined transport+lanes+editor base as a backdrop
    //     for their centered panel (unchanged).
    // Keymaps for Perform/Pattern remain shared (Edit); only the render differs.
    let footer = |f: &mut Frame, area: Rect| {
        f.render_widget(Paragraph::new(context_footer(app)), area);
    };
    match app.workspace {
        Workspace::Perform => {
            let chunks = Layout::vertical([
                Constraint::Length(3), // transport
                Constraint::Min(3),    // lane overview
                Constraint::Length(1), // footer
            ])
            .split(area);
            transport::render_transport(f, chunks[0], app);
            lanes::render_lanes(f, chunks[1], app);
            footer(f, chunks[2]);
        }
        Workspace::Pattern => {
            let chunks = Layout::vertical([
                Constraint::Length(3), // transport
                Constraint::Min(3),    // step editor
                Constraint::Length(1), // footer
            ])
            .split(area);
            transport::render_transport(f, chunks[0], app);
            match app.focused_kind() {
                LaneKind::Drums => editor_drums::render_drum_editor(f, chunks[1], app),
                LaneKind::Melodic => editor_melodic::render_melodic_editor(f, chunks[1], app),
            }
            footer(f, chunks[2]);
        }
        // Library/Song/Setup each render their OWN view as the full body
        // (transport on top + the workspace view filling the rest + footer).
        // Previously these drew the combined transport+lanes+editor base and
        // then floated a small centered panel on top with no Clear, so the drum
        // editor showed through behind the panel. Now the base IS the view.
        Workspace::Library => {
            let chunks = Layout::vertical([
                Constraint::Length(3), // transport
                Constraint::Min(3),    // library browser
                Constraint::Length(1), // footer
            ])
            .split(area);
            transport::render_transport(f, chunks[0], app);
            library::render_library(f, chunks[1], app);
            footer(f, chunks[2]);
        }
        Workspace::Song => {
            let chunks = Layout::vertical([
                Constraint::Length(3), // transport
                Constraint::Min(3),    // scene manager
                Constraint::Length(1), // footer
            ])
            .split(area);
            transport::render_transport(f, chunks[0], app);
            scene_view::render_scene_view(f, chunks[1], app);
            footer(f, chunks[2]);
        }
        Workspace::Setup => {
            let chunks = Layout::vertical([
                Constraint::Length(3), // transport
                Constraint::Min(3),    // route editor
                Constraint::Length(1), // footer
            ])
            .split(area);
            transport::render_transport(f, chunks[0], app);
            route_editor::render_route_editor(f, chunks[1], app);
            footer(f, chunks[2]);
        }
    }

    // Overlays float centered on top of the active workspace base. Each first
    // paints an opaque theme backdrop over its rect (Clear + bg block) so the
    // workspace base never bleeds through the panel's internal gaps.
    if let Some(overlay) = &app.overlay {
        match overlay {
            Overlay::Help => {
                let r = centered(area, 60, 70);
                clear_overlay_bg(f, r);
                help::render_help(f, r, app.help_scroll, app.help_basic)
            }
            Overlay::Onboarding => {
                let r = centered(area, 60, 50);
                clear_overlay_bg(f, r);
                onboarding::render_onboarding(f, r, app)
            }
            // Tempo entry is shown inline in the transport bar; no centered panel.
            Overlay::TempoEntry => {}
            Overlay::NameEntry(_) => {
                let r = centered(area, 50, 30);
                clear_overlay_bg(f, r);
                mgmt::render_name_entry(f, r, app)
            }
            Overlay::Confirm(_) => {
                let r = centered(area, 50, 25);
                clear_overlay_bg(f, r);
                mgmt::render_confirm(f, r, app)
            }
            Overlay::Recovery => {
                let r = centered(area, 70, 60);
                clear_overlay_bg(f, r);
                recovery::render_recovery_prompt(f, r)
            }
            Overlay::SetBrowser => {
                let r = centered(area, 60, 70);
                clear_overlay_bg(f, r);
                library::render_set_browser(f, r, app)
            }
            Overlay::Chains => {
                let r = centered(area, 70, 80);
                clear_overlay_bg(f, r);
                chain_view::render_chain_view(f, r, app)
            }
            Overlay::ClockInSelector => {
                let r = centered(area, 60, 60);
                clear_overlay_bg(f, r);
                clock_in_selector::render_clock_in_selector(f, r, app)
            }
            Overlay::DevicePicker => {
                let r = centered(area, 70, 70);
                clear_overlay_bg(f, r);
                device_picker::render_device_picker(f, r, app)
            }
            Overlay::NoteInput => {
                let r = centered(area, 60, 20);
                clear_overlay_bg(f, r);
                mgmt::render_note_input(f, r, app)
            }
            Overlay::Generative => {
                let r = centered(area, 70, 70);
                clear_overlay_bg(f, r);
                generative_view::render_generative_panel(f, r, app)
            }
            Overlay::CrateView => {
                let r = centered(area, 70, 70);
                clear_overlay_bg(f, r);
                crate_view::render_crate_view(f, r, app)
            }
            Overlay::CommandPalette => {
                let r = centered(area, 60, 70);
                clear_overlay_bg(f, r);
                command_palette::render_command_palette(f, r, app)
            }
        }
    }
}

/// Paint an opaque theme-colored backdrop over `rect` for a modal overlay:
/// `Clear` drops whatever the workspace base drew there, then a theme-bg block
/// writes an opaque background so the panel's internal gaps don't reveal the
/// base beneath it.
fn clear_overlay_bg(f: &mut Frame, rect: Rect) {
    f.render_widget(ratatui::widgets::Clear, rect);
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(EMBER.bg)),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::profiles::default_profiles;
    use crate::pattern::library::{GenreMap, Library};
    use crate::pattern::model::Set;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn empty_library() -> Library {
        Library {
            v2_index: Default::default(),
            families: Vec::new(),
            drums: GenreMap::new(),
            bass: GenreMap::new(),
            synth: GenreMap::new(),
        }
    }

    fn render_to_string(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    // --- Bug A: full-frame paint + proper per-workspace bases ---

    #[test]
    fn song_base_is_scenes_not_step_editor() {
        // The Song workspace base must render the scenes view filling the body,
        // NOT the transport+lanes+step-editor backdrop with a floating panel.
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Song);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("SCENES"),
            "Song base must show the scenes view; got: {whole:?}"
        );
        // "steps" only appears in the drum/melodic step-editor title/indicator.
        assert!(
            !whole.contains("steps"),
            "Song base must NOT render the step editor; got: {whole:?}"
        );
    }

    #[test]
    fn every_cell_is_painted_with_a_background() {
        // Regression for scrollback bleed-through: render must write a background
        // to EVERY cell each frame (ratatui only diffs against its own buffer, so
        // untouched cells would leave the real terminal's scrollback visible).
        for ws in [
            Workspace::Perform,
            Workspace::Pattern,
            Workspace::Library,
            Workspace::Song,
            Workspace::Setup,
        ] {
            let set = Set::default_set(default_profiles());
            let mut app = App::new(set, empty_library());
            app.set_workspace(ws);
            let backend = TestBackend::new(120, 30);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| render(f, &app)).unwrap();
            let buf = term.backend().buffer();
            // An "unpainted" cell keeps the terminal-default background
            // (`Color::Reset`); ratatui then never emits it, so the real
            // terminal's scrollback shows through. Every cell must instead carry
            // an explicit background from the full-frame theme paint.
            for cell in buf.content() {
                assert_ne!(
                    cell.style().bg,
                    Some(ratatui::style::Color::Reset),
                    "found an unpainted cell in {ws:?} (Color::Reset bg): {cell:?}"
                );
            }
        }
    }

    #[test]
    fn render_draws_without_panic_and_shows_footer() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        let whole = render_to_string(&app);
        // Footer hint contains "play".
        assert!(
            whole.contains("play"),
            "expected footer hint, got: {whole:?}"
        );
    }

    // --- context-sensitive footer tests ---

    #[test]
    fn footer_edit_drums_shows_euclid_and_toggle() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Perform);
        // Default focused lane should be Drums; verify via focused_kind.
        assert_eq!(app.focused_kind(), LaneKind::Drums);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("euclid"),
            "Edit+Drums footer should contain 'euclid'"
        );
        assert!(
            whole.contains("toggle"),
            "Edit+Drums footer should contain 'toggle'"
        );
        // Should NOT contain melodic-only terms
        assert!(
            !whole.contains("pitch"),
            "Edit+Drums footer should NOT contain 'pitch'"
        );
    }

    #[test]
    fn footer_edit_melodic_shows_pitch_and_slide() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Perform);
        // Switch to a melodic lane (if one exists in the default set; otherwise skip gracefully)
        if app.focused_kind() == LaneKind::Melodic {
            let whole = render_to_string(&app);
            assert!(
                whole.contains("pitch"),
                "Edit+Melodic footer should contain 'pitch'"
            );
            assert!(
                whole.contains("slide"),
                "Edit+Melodic footer should contain 'slide'"
            );
            assert!(
                !whole.contains("euclid"),
                "Edit+Melodic footer should NOT contain 'euclid'"
            );
        }
    }

    #[test]
    fn footer_library_shows_column_and_load_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Library);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("column"),
            "Library footer should contain 'column'"
        );
        assert!(
            whole.contains("load"),
            "Library footer should contain 'load'"
        );
        assert!(
            !whole.contains("euclid"),
            "Library footer should NOT contain 'euclid'"
        );
    }

    #[test]
    fn footer_setbrowser_shows_open_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.open_overlay(Overlay::SetBrowser);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("open"),
            "SetBrowser footer should contain 'open'"
        );
        assert!(
            !whole.contains("euclid"),
            "SetBrowser footer should NOT contain 'euclid'"
        );
    }

    #[test]
    fn footer_tempo_entry_shows_bpm_not_euclid() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.open_overlay(Overlay::TempoEntry);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("BPM"),
            "TempoEntry footer should contain 'BPM'"
        );
        assert!(
            !whole.contains("euclid"),
            "TempoEntry footer should NOT contain 'euclid'"
        );
    }

    #[test]
    fn footer_shows_context_label() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());

        // Perform/Edit is the App::new() default — no sync-helper call needed.
        let whole = render_to_string(&app);
        assert!(
            whole.contains("EDIT DRUM"),
            "should show context label EDIT DRUM"
        );

        app.set_workspace(Workspace::Library);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("LIBRARY"),
            "should show context label LIBRARY"
        );
    }

    #[test]
    fn footer_help_mode_shows_close_hint() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.open_overlay(Overlay::Help);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("close"),
            "Help footer should contain 'close'"
        );
    }

    #[test]
    fn footer_route_editor_shows_lane_and_close_hints() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Setup);
        let whole = render_to_string(&app);
        assert!(
            whole.contains("lane"),
            "RouteEditor footer should contain 'lane'; got: {whole:?}"
        );
        assert!(
            whole.contains("close"),
            "RouteEditor footer should contain 'close'; got: {whole:?}"
        );
        assert!(
            whole.contains("ROUTES"),
            "RouteEditor context label should be 'ROUTES'; got: {whole:?}"
        );
    }

    // --- Task 5: PERFORM/PATTERN render specialization ---

    #[test]
    fn perform_base_shows_lane_overview_not_step_editor() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Perform);
        let whole = render_to_string(&app);
        // Overview: the LANES panel is present…
        assert!(
            whole.contains("LANES"),
            "Perform base should render the lane overview panel, got: {whole:?}"
        );
        // …and the focused-lane step editor (its " EDIT · … steps " title) is NOT.
        assert!(
            !whole.contains("steps"),
            "Perform base must not render the step editor, got: {whole:?}"
        );
    }

    #[test]
    fn pattern_base_shows_step_editor_not_lane_overview() {
        let set = Set::default_set(default_profiles());
        let mut app = App::new(set, empty_library());
        app.set_workspace(Workspace::Pattern);
        let whole = render_to_string(&app);
        // Step editor: the focused-lane grid title includes "steps"…
        assert!(
            whole.contains("steps"),
            "Pattern base should render the focused-lane step editor, got: {whole:?}"
        );
        // …and the multi-lane overview panel is NOT shown.
        assert!(
            !whole.contains("LANES"),
            "Pattern base must not render the lane overview, got: {whole:?}"
        );
    }

    // --- minimum terminal size tests ---

    fn render_to_string_sized(app: &App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn tiny_terminal_shows_resize_warning() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        // 30x6 is well below the 60x16 minimum.
        let whole = render_to_string_sized(&app, 30, 6);
        // The full message is longer than 30 chars so it gets clipped, but the
        // minimum-size spec "60x16" fits within the first 30 chars and must appear.
        assert!(
            whole.contains("60x16"),
            "expected minimum dimensions in resize warning, got: {whole:?}"
        );
    }

    #[test]
    fn normal_terminal_does_not_show_resize_warning() {
        let set = Set::default_set(default_profiles());
        let app = App::new(set, empty_library());
        // 120x30 is well above the minimum — normal render path.
        let whole = render_to_string_sized(&app, 120, 30);
        assert!(
            whole.contains("play"),
            "expected normal footer hint on large terminal, got: {whole:?}"
        );
        assert!(
            !whole.contains("resize the terminal"),
            "should NOT show resize warning on large terminal"
        );
    }
}
