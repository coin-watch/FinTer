//! Chart-specific color palette.
//!
//! Derived from the active GPUI theme so every existing theme keeps working.
//! The palette adds the tokens the chart needs (grid, cursor, measurement,
//! flags) without requiring every theme definition to list them.

use gpui::{Hsla, hsla};

use crate::theme::ThemeColor;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartPalette {
    pub canvas: Hsla,
    pub rail: Hsla,
    pub grid_minor: Hsla,
    pub grid_major: Hsla,
    pub axis_text: Hsla,
    pub axis_text_active: Hsla,
    pub crosshair: Hsla,
    pub measure_fill: Hsla,
    pub measure_stroke: Hsla,
    pub live_edge: Hsla,
    pub up: Hsla,
    pub down: Hsla,
    pub gap_hatch: Hsla,
    pub flag_bg: Hsla,
    pub flag_fg: Hsla,
    pub volume_up: Hsla,
    pub volume_down: Hsla,
}

impl ChartPalette {
    pub fn from_theme(theme: &ThemeColor) -> Self {
        let accent = hsla(0.204, 1.0, 0.655, 1.0);
        Self {
            canvas: theme.background,
            rail: theme.background,
            grid_minor: theme.foreground.opacity(0.06),
            grid_major: theme.foreground.opacity(0.10),
            axis_text: theme.muted_foreground,
            axis_text_active: theme.foreground,
            crosshair: accent.opacity(0.55),
            measure_fill: accent.opacity(0.10),
            measure_stroke: accent,
            live_edge: accent,
            up: theme.bullish,
            down: theme.bearish,
            gap_hatch: theme.muted_foreground.opacity(0.12),
            flag_bg: theme.background,
            flag_fg: theme.foreground,
            volume_up: theme.bullish.opacity(0.45),
            volume_down: theme.bearish.opacity(0.45),
        }
    }

    /// Default "Obsidian" palette, used by tests and before a theme is bound.
    pub fn obsidian() -> Self {
        let accent = hsla(0.204, 1.0, 0.655, 1.0);
        Self {
            canvas: hsla(0.52, 0.13, 0.045, 1.0),
            rail: hsla(0.50, 0.07, 0.060, 1.0),
            grid_minor: hsla(0.43, 0.05, 0.92, 0.06),
            grid_major: hsla(0.43, 0.05, 0.92, 0.10),
            axis_text: hsla(0.42, 0.05, 0.50, 1.0),
            axis_text_active: hsla(0.42, 0.05, 0.92, 1.0),
            crosshair: accent.opacity(0.55),
            measure_fill: accent.opacity(0.10),
            measure_stroke: accent,
            live_edge: accent,
            up: hsla(0.425, 0.71, 0.62, 1.0),
            down: hsla(0.983, 1.0, 0.71, 1.0),
            gap_hatch: hsla(0.42, 0.05, 0.50, 0.12),
            flag_bg: hsla(0.50, 0.09, 0.09, 1.0),
            flag_fg: hsla(0.42, 0.05, 0.92, 1.0),
            volume_up: hsla(0.425, 0.71, 0.62, 0.45),
            volume_down: hsla(0.983, 1.0, 0.71, 0.45),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_palette_has_contrasting_up_and_down() {
        let palette = ChartPalette::obsidian();
        assert_ne!(palette.up, palette.down);
        assert!(palette.crosshair.a > 0.0);
        assert!(palette.canvas.a > 0.0);
    }
}
