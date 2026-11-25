//! Shared responsive sizing for chart overlays.
//!
//! The chart canvas already fills its module; these metrics keep the legends in
//! proportion with the available plot area instead of using fixed pixel sizes.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartMetrics {
    pub pane_width: f32,
    pub pane_height: f32,
    pub plot_width: f32,
    pub plot_height: f32,
    pub scale: f32,
    pub text_size: f32,
    pub legend_text_size: f32,
    pub compact: bool,
}

impl Default for ChartMetrics {
    fn default() -> Self {
        Self::new(800.0, 500.0, 740.0, 460.0)
    }
}

impl ChartMetrics {
    pub fn new(pane_width: f32, pane_height: f32, plot_width: f32, plot_height: f32) -> Self {
        let pane_width = if pane_width.is_finite() && pane_width > 0.0 {
            pane_width
        } else {
            800.0
        };
        let pane_height = if pane_height.is_finite() && pane_height > 0.0 {
            pane_height
        } else {
            500.0
        };
        let plot_width = if plot_width.is_finite() && plot_width > 0.0 {
            plot_width
        } else {
            (pane_width - 64.0).max(80.0)
        };
        let plot_height = if plot_height.is_finite() && plot_height > 0.0 {
            plot_height
        } else {
            (pane_height - 34.0).max(80.0)
        };

        let scale = ((pane_width / 1000.0).min(pane_height / 700.0)).clamp(0.75, 1.35);
        let text_size = (10.0 * scale).clamp(9.0, 13.0);
        let legend_text_size = (11.0 * scale).clamp(9.5, 14.0);

        Self {
            pane_width,
            pane_height,
            plot_width,
            plot_height,
            scale,
            text_size,
            legend_text_size,
            compact: pane_width < 520.0 || pane_height < 220.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_panes_reduce_overlay_density() {
        let small = ChartMetrics::new(320.0, 180.0, 260.0, 130.0);
        let large = ChartMetrics::new(1600.0, 900.0, 1500.0, 760.0);
        assert!(small.compact);
        assert!(small.text_size <= large.text_size);
        assert!(small.legend_text_size <= large.legend_text_size);
    }

    #[test]
    fn invalid_plot_size_falls_back_to_pane_size() {
        let metrics = ChartMetrics::new(900.0, 500.0, 0.0, 0.0);
        assert!(metrics.plot_width > 0.0);
        assert!(metrics.plot_height > 0.0);
        assert!(!metrics.compact);
    }
}
