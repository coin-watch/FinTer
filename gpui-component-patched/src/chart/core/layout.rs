//! Pane and axis-rail layout.
//!
//! The layout is computed from bounds and pane ratios once per size change. It
//! replaces the legacy pattern of saving plot origins and separator positions
//! into `Rc<Cell>` fields during paint.

use super::geometry::Rect;

/// Height of the time-axis rail.
pub const AXIS_GAP: f32 = 18.0;
/// Gap between stacked panes.
pub const PANE_GAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKind {
    Price,
    Volume,
    Subpane(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLayout {
    pub kind: PaneKind,
    /// Plot rectangle for this pane.
    pub rect: Rect,
    /// Fraction of the plot height reserved for this pane.
    pub ratio: f32,
    /// Price-axis rail slice beside this pane.
    pub axis: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartLayout {
    pub bounds: Rect,
    /// Main candle plot.
    pub plot: Rect,
    /// Right-hand price rail spanning all panes.
    pub price_rail: Rect,
    /// Bottom time rail.
    pub time_rail: Rect,
    pub panes: Vec<PaneLayout>,
}

impl Default for ChartLayout {
    fn default() -> Self {
        Self::empty(Rect::ZERO)
    }
}

impl ChartLayout {
    pub fn empty(bounds: Rect) -> Self {
        Self {
            bounds,
            plot: bounds,
            price_rail: Rect::ZERO,
            time_rail: Rect::ZERO,
            panes: Vec::new(),
        }
    }

    pub fn main(&self) -> Option<&PaneLayout> {
        self.panes
            .iter()
            .find(|pane| matches!(pane.kind, PaneKind::Price))
    }

    pub fn pane_at(&self, y: f32) -> Option<&PaneLayout> {
        self.panes
            .iter()
            .find(|pane| y >= pane.rect.y && y < pane.rect.bottom() && pane.rect.h > 0.0)
    }

    /// Y positions of the draggable separators between panes.
    pub fn separator_ys(&self) -> Vec<(usize, f32)> {
        self.panes
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                let bottom = pair[0].rect.bottom();
                let top = pair[1].rect.y;
                if top > bottom {
                    Some((index, (bottom + top) * 0.5))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutOptions {
    pub time_rail_h: f32,
    pub pane_gap: f32,
    pub min_main_h: f32,
    pub min_pane_h: f32,
    pub default_pane_ratio: f32,
    pub max_total_pane_ratio: f32,
    pub min_plot_w: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            time_rail_h: AXIS_GAP,
            pane_gap: PANE_GAP,
            min_main_h: 10.0,
            min_pane_h: 10.0,
            default_pane_ratio: 0.18,
            max_total_pane_ratio: 0.80,
            min_plot_w: 40.0,
        }
    }
}

impl LayoutOptions {
    fn sanitized(self) -> Self {
        let clamp_positive = |v: f32, fallback: f32| {
            if v.is_finite() && v >= 0.0 {
                v
            } else {
                fallback
            }
        };
        let default_pane_ratio = if self.default_pane_ratio.is_finite() {
            self.default_pane_ratio.clamp(0.02, 0.60)
        } else {
            0.18
        };
        let max_total = if self.max_total_pane_ratio.is_finite() {
            self.max_total_pane_ratio.clamp(0.10, 0.90)
        } else {
            0.80
        };
        Self {
            time_rail_h: clamp_positive(self.time_rail_h, AXIS_GAP),
            pane_gap: clamp_positive(self.pane_gap, PANE_GAP),
            min_main_h: clamp_positive(self.min_main_h, 10.0),
            min_pane_h: clamp_positive(self.min_pane_h, 10.0),
            default_pane_ratio,
            max_total_pane_ratio: max_total,
            min_plot_w: clamp_positive(self.min_plot_w, 40.0),
        }
    }
}

/// Width for the price rail based on the widest label.
pub fn price_rail_width(
    labels: &[&str],
    mut measure: impl FnMut(&str) -> f32,
    padding: f32,
    min: f32,
    max: f32,
) -> f32 {
    let padding = if padding.is_finite() && padding >= 0.0 {
        padding
    } else {
        0.0
    };
    let min = if min.is_finite() && min > 0.0 {
        min
    } else {
        40.0
    };
    let max = if max.is_finite() && max >= min {
        max
    } else {
        min
    };
    let widest = labels
        .iter()
        .map(|label| measure(label))
        .filter(|width| width.is_finite())
        .fold(0.0_f32, f32::max);
    (widest + padding).clamp(min, max)
}

/// Compute the pane layout for a chart bounds.
///
/// `pane_ratios` includes every stacked pane below the main price pane. When
/// `has_volume` is true, the first ratio belongs to the volume pane.
pub fn compute(
    bounds: Rect,
    price_rail_w: f32,
    pane_ratios: &[f32],
    has_volume: bool,
    options: LayoutOptions,
) -> ChartLayout {
    let options = options.sanitized();
    if !bounds.is_finite() || bounds.w <= 1.0 || bounds.h <= 1.0 {
        return ChartLayout::empty(if bounds.is_finite() {
            bounds
        } else {
            Rect::ZERO
        });
    }

    let rail_w = if price_rail_w.is_finite() {
        price_rail_w
            .max(0.0)
            .min((bounds.w - options.min_plot_w).max(0.0))
    } else {
        0.0
    };
    let plot_w = (bounds.w - rail_w).max(0.0);
    let time_h = options.time_rail_h.min(bounds.h * 0.5);
    let plot_h = (bounds.h - time_h).max(0.0);

    let mut ratios: Vec<f32> = pane_ratios
        .iter()
        .copied()
        .map(|ratio| {
            if ratio.is_finite() {
                ratio.clamp(0.02, 0.60)
            } else {
                options.default_pane_ratio
            }
        })
        .collect();
    if has_volume && ratios.is_empty() {
        ratios.push(options.default_pane_ratio);
    }

    let mut total: f32 = ratios.iter().sum();
    if total > options.max_total_pane_ratio && total > 0.0 {
        let scale = options.max_total_pane_ratio / total;
        for ratio in &mut ratios {
            *ratio *= scale;
        }
        total = options.max_total_pane_ratio;
    }
    if ratios.is_empty() {
        total = 0.0;
    }

    let gap_count = ratios.len() as f32;
    let gaps = options.pane_gap * gap_count;
    let desired_main = plot_h * (1.0 - total);
    let main_h = desired_main.max(options.min_main_h).min(plot_h).max(0.0);
    let sub_available = (plot_h - main_h - gaps).max(0.0);
    let ratio_sum: f32 = ratios.iter().sum();

    let main_rect = Rect::new(bounds.x, bounds.y, plot_w, main_h);
    let price_rail = Rect::new(bounds.x + plot_w, bounds.y, rail_w, plot_h);
    let time_rail = Rect::new(bounds.x, bounds.y + plot_h, plot_w, time_h);

    let mut panes = Vec::with_capacity(1 + ratios.len());
    panes.push(PaneLayout {
        kind: PaneKind::Price,
        rect: main_rect,
        ratio: 1.0 - total,
        axis: Rect::new(bounds.x + plot_w, bounds.y, rail_w, main_h),
    });

    let mut y = main_rect.bottom() + options.pane_gap;
    let bottom_limit = bounds.y + plot_h;
    for (index, ratio) in ratios.iter().copied().enumerate() {
        let share = if ratio_sum > 0.0 {
            ratio / ratio_sum
        } else {
            1.0 / ratios.len().max(1) as f32
        };
        let mut h = (sub_available * share).max(options.min_pane_h);
        if y + h > bottom_limit {
            h = (bottom_limit - y).max(0.0);
        }
        if h <= 0.0 {
            break;
        }
        let kind = if has_volume && index == 0 {
            PaneKind::Volume
        } else {
            PaneKind::Subpane(if has_volume { index - 1 } else { index })
        };
        panes.push(PaneLayout {
            kind,
            rect: Rect::new(bounds.x, y, plot_w, h),
            ratio,
            axis: Rect::new(bounds.x + plot_w, y, rail_w, h),
        });
        y += h + options.pane_gap;
    }

    ChartLayout {
        bounds,
        plot: main_rect,
        price_rail,
        time_rail,
        panes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 500.0)
    }

    #[test]
    fn no_subpanes_plot_fills_available_area() {
        let layout = compute(bounds(), 60.0, &[], false, LayoutOptions::default());
        assert_eq!(layout.plot, Rect::new(0.0, 0.0, 740.0, 482.0));
        assert_eq!(layout.price_rail, Rect::new(740.0, 0.0, 60.0, 482.0));
        assert_eq!(layout.time_rail, Rect::new(0.0, 482.0, 740.0, 18.0));
        assert_eq!(layout.panes.len(), 1);
        assert!(matches!(layout.panes[0].kind, PaneKind::Price));
    }

    #[test]
    fn volume_and_subpanes_are_stacked() {
        let layout = compute(
            bounds(),
            60.0,
            &[0.18, 0.18, 0.18],
            true,
            LayoutOptions::default(),
        );
        assert_eq!(layout.panes.len(), 4);
        assert!(matches!(layout.panes[0].kind, PaneKind::Price));
        assert!(matches!(layout.panes[1].kind, PaneKind::Volume));
        assert!(matches!(layout.panes[2].kind, PaneKind::Subpane(0)));
        assert!(matches!(layout.panes[3].kind, PaneKind::Subpane(1)));
        assert!(
            layout
                .panes
                .windows(2)
                .all(|p| p[0].rect.bottom() <= p[1].rect.y)
        );
        assert_eq!(layout.separator_ys().len(), 3);
    }

    #[test]
    fn volume_without_ratios_gets_default_slot() {
        let layout = compute(bounds(), 60.0, &[], true, LayoutOptions::default());
        assert_eq!(layout.panes.len(), 2);
        assert!(matches!(layout.panes[1].kind, PaneKind::Volume));
        assert!(layout.panes[1].rect.h > 0.0);
    }

    #[test]
    fn ratios_are_capped_at_max_total() {
        let layout = compute(
            bounds(),
            60.0,
            &[0.6, 0.6, 0.6],
            false,
            LayoutOptions::default(),
        );
        let total: f32 = layout.panes.iter().skip(1).map(|pane| pane.ratio).sum();
        assert!(total <= 0.8001, "{total}");
        assert!(layout.plot.h >= 10.0);
    }

    #[test]
    fn tiny_bounds_do_not_panic_or_overflow() {
        let layout = compute(
            Rect::new(0.0, 0.0, 20.0, 12.0),
            60.0,
            &[0.2, 0.2],
            true,
            LayoutOptions::default(),
        );
        assert!(layout.plot.w >= 0.0);
        assert!(layout.plot.h >= 0.0);
        assert!(layout.panes.iter().all(|pane| pane.rect.w >= 0.0));
    }

    #[test]
    fn invalid_bounds_return_empty() {
        let layout = compute(
            Rect::new(f32::NAN, 0.0, 800.0, 500.0),
            60.0,
            &[],
            false,
            LayoutOptions::default(),
        );
        assert!(layout.panes.is_empty());
    }

    #[test]
    fn pane_at_finds_stacked_panes() {
        let layout = compute(bounds(), 60.0, &[0.2, 0.2], true, LayoutOptions::default());
        let volume = &layout.panes[1];
        assert_eq!(
            layout.pane_at(volume.rect.center().y).map(|p| p.kind),
            Some(PaneKind::Volume)
        );
    }

    #[test]
    fn price_rail_width_measures_and_clamps() {
        let width = price_rail_width(
            &["64,120.50", "1.2k"],
            |label| label.len() as f32 * 7.0,
            12.0,
            50.0,
            120.0,
        );
        assert!((width - 75.0).abs() < 1e-4);
        let width = price_rail_width(&[], |_| 0.0, 12.0, 50.0, 120.0);
        assert_eq!(width, 50.0);
        let long_label = "x".repeat(200);
        let width = price_rail_width(
            &[long_label.as_str()],
            |label| label.len() as f32,
            0.0,
            50.0,
            120.0,
        );
        assert_eq!(width, 120.0);
    }

    #[test]
    fn separator_positions_are_between_panes() {
        let layout = compute(bounds(), 60.0, &[0.2], true, LayoutOptions::default());
        let separators = layout.separator_ys();
        assert_eq!(separators.len(), 1);
        let (index, y) = separators[0];
        assert_eq!(index, 0);
        assert!(y > layout.panes[0].rect.bottom());
        assert!(y < layout.panes[1].rect.y);
    }
}
