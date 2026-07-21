//! Command registry: the single source of truth for the accelerator hint bar
//! (`ui::context_footer`) and the (future) command palette.
//!
//! Every entry pairs a display name with the real [`Action`] it dispatches and
//! the actual key binding from `src/input.rs`. Consumers look up accelerators
//! via [`accel_for`] instead of hard-coding key literals, so hints can never
//! silently drift from what `key_to_action` actually does (this is exactly
//! what happened across the workspace/overlay reroute: `l`, `G`, `w` and
//! Ctrl+1..5 all changed meaning, and nothing forced hint strings to follow).
//!
//! Scope (Task 6 / Phase-1 foundation): the ~30 most-used commands — global
//! shortcuts plus the Perform/Pattern (Edit) keymap, which is where the
//! reroute concretely added new workspace switches. Overlays (Help,
//! TempoEntry, SetBrowser, …) fully own the keymap while raised and are not
//! modeled as registry entries here; their hint text remains hand-authored in
//! `ui::mod::context_footer`. Continuous/parameterized editing primitives
//! (arrow-key movement, `0-9` velocity buckets, `e`/`E` euclid nudge, …) are
//! likewise out of scope — they aren't single discrete accelerator-bound
//! commands in the palette sense.

use crate::app::{Action, Workspace};

/// A single user-facing command: its display name, the [`Action`] it
/// dispatches, the workspace it is reachable from, and its real keyboard
/// accelerator.
pub struct Command {
    /// Display name for the command palette, e.g. `"Open library"`.
    pub name: &'static str,
    /// The `Action` this command dispatches.
    pub action: Action,
    /// `None` = reachable regardless of the active workspace (a truly global
    /// shortcut, e.g. Space/Panic/Undo/F1..F5, or one bound in more than
    /// one workspace, e.g. `?` for Help). `Some(ws)` = only bound in `ws`'s
    /// base keymap. Perform and Pattern share the Edit keymap (see
    /// `input.rs`), so `Some(Workspace::Perform)` entries are also reachable
    /// from `Workspace::Pattern` — callers filtering by workspace should
    /// treat the two as equivalent (see `ui::mod::footer_workspace_hint`).
    pub workspace: Option<Workspace>,
    /// Real accelerator label as bound in `src/input.rs`, e.g. `"l"` or
    /// `"F3"`. Empty only if a command is intentionally unbound.
    pub accel: &'static str,
}

/// The command registry, in a stable declaration order (used as display
/// order for the palette). See the module doc for scope.
static REGISTRY: [Command; 35] = [
    // ── Truly global (checked before any workspace/overlay branch, or bound
    // ── in more than one workspace) ──────────────────────────────────────
    Command {
        name: "Play / stop",
        action: Action::TogglePlay,
        workspace: None,
        accel: "Space",
    },
    Command {
        name: "Panic (all notes off)",
        action: Action::Panic,
        workspace: None,
        accel: "!",
    },
    Command {
        name: "Undo",
        action: Action::Undo,
        workspace: None,
        accel: "Ctrl+Z",
    },
    Command {
        name: "Redo",
        action: Action::Redo,
        workspace: None,
        accel: "Ctrl+Y",
    },
    Command {
        name: "Switch to Perform",
        action: Action::SwitchWorkspace(Workspace::Perform),
        workspace: None,
        accel: "F1",
    },
    Command {
        name: "Switch to Pattern",
        action: Action::SwitchWorkspace(Workspace::Pattern),
        workspace: None,
        accel: "F2",
    },
    Command {
        name: "Switch to Library",
        action: Action::SwitchWorkspace(Workspace::Library),
        workspace: None,
        accel: "F3",
    },
    Command {
        name: "Switch to Song",
        action: Action::SwitchWorkspace(Workspace::Song),
        workspace: None,
        accel: "F4",
    },
    Command {
        name: "Switch to Setup",
        action: Action::SwitchWorkspace(Workspace::Setup),
        workspace: None,
        accel: "F5",
    },
    // '?' opens Help from Perform/Pattern/Library (not Song/Setup, which
    // return early from their own keymaps); not tied to a single workspace.
    Command {
        name: "Help",
        action: Action::Help,
        workspace: None,
        accel: "?",
    },
    // Ctrl+P works from anywhere (':' additionally opens it from any bare
    // workspace). Listed so the palette itself is discoverable in the hints.
    Command {
        name: "Open command palette",
        action: Action::OpenPalette,
        workspace: None,
        accel: "Ctrl+P",
    },
    // ── Perform/Pattern (Edit keymap) ────────────────────────────────────
    Command {
        name: "Save",
        action: Action::Save,
        workspace: Some(Workspace::Perform),
        accel: "s",
    },
    Command {
        name: "Quit",
        action: Action::Quit,
        workspace: Some(Workspace::Perform),
        accel: "q",
    },
    Command {
        name: "Toggle Ableton Link",
        action: Action::ToggleLink,
        workspace: Some(Workspace::Perform),
        accel: "k",
    },
    Command {
        name: "Open tempo entry",
        action: Action::OpenTempo,
        workspace: Some(Workspace::Perform),
        accel: "t",
    },
    Command {
        name: "Tap tempo",
        action: Action::Tap,
        workspace: Some(Workspace::Perform),
        accel: "T",
    },
    Command {
        name: "Open library",
        action: Action::OpenLibrary,
        workspace: Some(Workspace::Perform),
        accel: "l",
    },
    Command {
        name: "Open set browser",
        action: Action::OpenSetBrowser,
        workspace: Some(Workspace::Perform),
        accel: "o",
    },
    Command {
        name: "Open route editor",
        action: Action::OpenRouteEditor,
        workspace: Some(Workspace::Perform),
        accel: "w",
    },
    Command {
        name: "Open device picker",
        action: Action::OpenDevicePicker,
        workspace: Some(Workspace::Perform),
        accel: "d",
    },
    Command {
        name: "Open scene manager",
        action: Action::OpenScenes,
        workspace: Some(Workspace::Perform),
        accel: "G",
    },
    Command {
        name: "Open chain manager",
        action: Action::OpenChains,
        workspace: Some(Workspace::Perform),
        accel: "K",
    },
    Command {
        name: "Open generative tool",
        action: Action::OpenGenerative,
        workspace: Some(Workspace::Perform),
        accel: "D",
    },
    Command {
        name: "Open crate browser",
        action: Action::OpenCrateView,
        workspace: Some(Workspace::Perform),
        accel: "V",
    },
    Command {
        name: "Open clock-in selector",
        action: Action::OpenClockInSelector,
        workspace: Some(Workspace::Perform),
        accel: "W",
    },
    Command {
        name: "Toggle launch quantize",
        action: Action::ToggleLaunchQuant,
        workspace: Some(Workspace::Perform),
        accel: "b",
    },
    Command {
        name: "Cancel queued launch",
        action: Action::CancelQueue,
        workspace: Some(Workspace::Perform),
        accel: "C",
    },
    Command {
        name: "Restart lane sync",
        action: Action::RestartLane,
        workspace: Some(Workspace::Perform),
        accel: "i",
    },
    Command {
        name: "Toggle fill",
        action: Action::ToggleFill,
        workspace: Some(Workspace::Perform),
        accel: "f",
    },
    Command {
        name: "Commit fill",
        action: Action::CommitTransform,
        workspace: Some(Workspace::Perform),
        accel: "F",
    },
    Command {
        name: "Save lane as user pattern",
        action: Action::OpenSaveUserPattern,
        workspace: Some(Workspace::Perform),
        accel: "A",
    },
    Command {
        name: "Clear pattern",
        action: Action::OpenClearPattern,
        workspace: Some(Workspace::Perform),
        accel: "Z",
    },
    Command {
        name: "Double pattern length",
        action: Action::DoubleLength,
        workspace: Some(Workspace::Perform),
        accel: "L",
    },
    Command {
        name: "Toggle mirror",
        action: Action::ToggleMirror,
        workspace: Some(Workspace::Perform),
        accel: "M",
    },
    Command {
        name: "Cycle clock divisor",
        action: Action::CycleClockDiv,
        workspace: Some(Workspace::Perform),
        accel: "Q",
    },
];

/// The full command registry.
pub fn registry() -> &'static [Command] {
    &REGISTRY
}

/// Look up the real accelerator label for `action`, if the registry has an
/// entry for it. Linear scan — the registry is tiny and this is only called
/// per-render (footer/palette), never in a hot per-frame loop over steps.
///
/// Matches by full equality (`Action` derives `PartialEq`), so data-bearing
/// variants (e.g. `SwitchWorkspace(Workspace)`) only match the specific
/// variant + payload a registry entry declares — `accel_for` does not do
/// discriminant-only matching.
pub fn accel_for(action: &Action) -> Option<&'static str> {
    REGISTRY
        .iter()
        .find(|c| &c.action == action)
        .map(|c| c.accel)
        .filter(|a| !a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_actions_are_unique_and_have_names() {
        let r = registry();
        assert!(!r.is_empty());
        for c in r {
            assert!(!c.name.is_empty(), "command {:?} needs a name", c.action);
        }
        // every command's accel label, if non-empty, is what input.rs actually
        // binds (spot-check a few).
        assert_eq!(accel_for(&Action::OpenLibrary), Some("l"));
    }

    #[test]
    fn registry_names_are_unique() {
        let mut seen = HashSet::new();
        for c in registry() {
            assert!(seen.insert(c.name), "duplicate command name: {}", c.name);
        }
    }

    /// Spot-check accelerators against the real `key_to_action` mapping in
    /// `input.rs`, focusing on the keys the Task-4/5 workspace reroute
    /// touched or added (`l`, `G`, `w`, F1..F5) plus a few long-standing
    /// globals, so drift is caught here rather than only in the UI.
    #[test]
    fn accel_for_matches_input_rs_spot_checks() {
        assert_eq!(accel_for(&Action::OpenLibrary), Some("l"));
        assert_eq!(accel_for(&Action::OpenScenes), Some("G"));
        assert_eq!(accel_for(&Action::OpenRouteEditor), Some("w"));
        assert_eq!(accel_for(&Action::OpenSetBrowser), Some("o"));
        assert_eq!(accel_for(&Action::Save), Some("s"));
        assert_eq!(accel_for(&Action::Panic), Some("!"));
        assert_eq!(accel_for(&Action::ToggleLink), Some("k"));
        assert_eq!(accel_for(&Action::TogglePlay), Some("Space"));
        assert_eq!(
            accel_for(&Action::SwitchWorkspace(Workspace::Library)),
            Some("F3")
        );
        assert_eq!(
            accel_for(&Action::SwitchWorkspace(Workspace::Perform)),
            Some("F1")
        );
        // An action with no registry entry returns None.
        assert_eq!(accel_for(&Action::FocusNext), None);
    }
}
