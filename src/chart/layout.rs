//! Shared responsive sizing for chart overlays.
//!
//! The chart canvas already fills its module; these metrics keep the overlays
//! (footprint rows, profile width, liquidity resolution, legends) in proportion
//! with the available plot area instead of using fixed pixel sizes.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartMetrics {
    pub pane_width: f32,
    pub pane_height: f32,
    pub plot_width: f32,
    pub plot_height: f32,
    pub scale: f32,
    pub text_size: f32,
    pub legend_text_size: f32,
    pub row_height: f32,
    pub footprint_rows: usize,
    pub profile_rows: usize,
    pub liquidity_rows: usize,
    pub profile_width_fraction: f32,
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
        let row_height = (text_size + 3.0).max(10.0);

        let footprint_rows = (plot_height / row_height).floor().clamp(10.0, 80.0) as usize;
        let profile_rows = (plot_height / (text_size + 6.0)).floor().clamp(14.0, 60.0) as usize;
        let liquidity_rows = (plot_height / 10.0).floor().clamp(20.0, 80.0) as usize;

        let profile_px = (plot_width * 0.18).clamp(70.0, 260.0);
        let profile_width_fraction = (profile_px / plot_width.max(1.0)).clamp(0.10, 0.28);

        Self {
            pane_width,
            pane_height,
            plot_width,
            plot_height,
            scale,
            text_size,
            legend_text_size,
            row_height,
            footprint_rows,
            profile_rows,
            liquidity_rows,
            profile_width_fraction,
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
        assert!(small.footprint_rows < large.footprint_rows);
        assert!(small.profile_rows < large.profile_rows);
        assert!(small.liquidity_rows < large.liquidity_rows);
    }

    #[test]
    fn profile_width_is_bounded() {
        let narrow = ChartMetrics::new(320.0, 240.0, 260.0, 190.0);
        let wide = ChartMetrics::new(1800.0, 900.0, 1700.0, 780.0);
        assert!((0.10..=0.28).contains(&narrow.profile_width_fraction));
        assert!((0.10..=0.28).contains(&wide.profile_width_fraction));
        // A narrow plot keeps a readable minimum pixel width, so its fraction
        // can be larger even though the absolute width is smaller.
        assert!(narrow.profile_width_fraction >= wide.profile_width_fraction);
    }

    #[test]
    fn invalid_plot_size_falls_back_to_pane_size() {
        let metrics = ChartMetrics::new(900.0, 500.0, 0.0, 0.0);
        assert!(metrics.plot_width > 0.0);
        assert!(metrics.plot_height > 0.0);
        assert!(metrics.footprint_rows >= 10);
    }
}
