//! Visual ramps and note naming. Pure, hardware-free, unit-tested.

/// Velocity -> shading glyph. `vel` is MIDI velocity 0..=127.
/// 0 renders as blank; 1..=127 is split into 5 ascending bands.
pub fn vel_glyph(vel: u8) -> char {
    if vel == 0 {
        return ' ';
    }
    // 5 bands over 1..=127. Band width ~= 127/5 ~= 25.4; use thresholds.
    match vel {
        1..=25 => '·',
        26..=51 => '░',
        52..=89 => '▒',
        90..=115 => '▓',
        _ => '█', // 116..=127
    }
}

/// Velocity -> 8-level vertical bar glyph `▁▂▃▄▅▆▇█`.
pub fn vel_bar(vel: u8) -> char {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    // Map 0..=127 onto index 0..=7.
    let idx = (vel as usize * 7) / 127;
    BARS[idx.min(7)]
}

/// MIDI note number -> name, using the MIDI 60 = "C4" convention.
pub fn note_name(midi: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let name = NAMES[(midi % 12) as usize];
    // octave: C4 == 60 -> 60/12 - 1 = 4; C-1 == 0 -> -1.
    let octave = (midi as i32) / 12 - 1;
    format!("{name}{octave}")
}

// --- static color theme (spec §7) -----------------------------------------
// Never hue-from-audio, never flashing. Each lane has a fixed accent (mpump's device
// hues); the cursor and playhead are reverse/bright. The terminal degrades to monochrome
// automatically when it reports no color support — no extra code is needed for that.

use ratatui::style::{Color, Modifier, Style};

/// Distinct static accent color per lane, keyed by `DeviceProfile::id`. Unknown ids fall
/// back to `Color::Gray` so an added profile still renders (just without a custom hue).
pub fn lane_color(profile_id: &str) -> Color {
    match profile_id {
        "s1" => Color::Rgb(0x6E, 0xC6, 0xFF),       // S-1 synth: cool cyan-blue
        "t8-drums" => Color::Rgb(0xFF, 0x8A, 0x3D), // T-8 drums: warm orange
        "t8-bass" => Color::Rgb(0xB6, 0x8C, 0xFF),  // T-8 bass: violet
        _ => Color::Gray,
    }
}

/// Velocity -> intensity color (dim -> bright), banded to match `vel_glyph`. Reinforces
/// the `░▒▓█` shading with a parallel brightness ramp.
pub fn vel_color(vel: u8) -> Color {
    match vel {
        0 => Color::DarkGray,
        1..=25 => Color::Rgb(0x55, 0x55, 0x55),
        26..=51 => Color::Rgb(0x80, 0x80, 0x80),
        52..=89 => Color::Rgb(0xAA, 0xAA, 0xAA),
        90..=115 => Color::Rgb(0xD0, 0xD0, 0xD0),
        _ => Color::Rgb(0xFF, 0xFF, 0xFF), // 116..=127, brightest
    }
}

/// The editor cursor cell: reverse-video + bold so it reads at a glance.
pub fn cursor_style() -> Style {
    Style::default()
        .add_modifier(Modifier::REVERSED)
        .add_modifier(Modifier::BOLD)
}

/// The live playhead column: a bright background sweep.
pub fn playhead_style() -> Style {
    Style::default().bg(Color::Rgb(0x33, 0x44, 0x55))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vel_glyph_ramps_from_space_to_block() {
        assert_eq!(vel_glyph(0), ' ');
        // mid velocity is a mid-ramp glyph, never space and never full block
        let mid = vel_glyph(64);
        assert_ne!(mid, ' ');
        assert_ne!(mid, '█');
        assert_eq!(vel_glyph(127), '█');
    }

    #[test]
    fn vel_bar_bands_cover_low_mid_high() {
        assert_eq!(vel_bar(0), '▁');
        assert_eq!(vel_bar(127), '█');
        let mid = vel_bar(64);
        assert_ne!(mid, '▁');
        assert_ne!(mid, '█');
    }

    #[test]
    fn note_name_uses_c4_is_60_convention() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(45), "A2");
        assert_eq!(note_name(46), "A#2");
        assert_eq!(note_name(0), "C-1");
    }

    #[test]
    fn lane_color_is_distinct_per_known_id_and_gray_for_unknown() {
        use ratatui::style::Color;
        let drums = lane_color("t8-drums");
        let bass = lane_color("t8-bass");
        let synth = lane_color("s1");
        // Three known ids -> three distinct colors.
        assert_ne!(drums, bass);
        assert_ne!(bass, synth);
        assert_ne!(drums, synth);
        // Unknown id -> Gray fallback.
        assert_eq!(lane_color("j6"), Color::Gray);
    }

    #[test]
    fn vel_color_brightens_with_velocity() {
        // A low and a high velocity must not map to the same color.
        assert_ne!(vel_color(20), vel_color(120));
    }

    #[test]
    fn cursor_and_playhead_styles_are_non_default() {
        use ratatui::style::Style;
        assert_ne!(cursor_style(), Style::default());
        assert_ne!(playhead_style(), Style::default());
    }
}
