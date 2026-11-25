//! Price scales and tick generation.
//!
//! The scale owns the mapping between data values and plot pixels. Scale modes
//! change the mapping (log, inverted) or the labels (percent); percent mode keeps
//! a linear geometric mapping but emits ticks at nice percentage intervals.

use super::geometry::Rect;

/// How prices are mapped onto the vertical axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceScaleMode {
    Linear,
    Log,
    /// Linear geometry, percentage labels relative to `anchor`.
    Percent {
        anchor_bits: u64,
    },
    InvertedLinear,
}

impl PriceScaleMode {
    pub fn percent(anchor: f64) -> Self {
        Self::Percent {
            anchor_bits: anchor.to_bits(),
        }
    }

    pub fn anchor(self) -> Option<f64> {
        match self {
            Self::Percent { anchor_bits } => Some(f64::from_bits(anchor_bits)),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "LIN",
            Self::Log => "LOG",
            Self::Percent { .. } => "%",
            Self::InvertedLinear => "INV",
        }
    }
}

/// A tick on the price axis, in data units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub value: f64,
}

/// A price scale over a fixed plot rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriceScale {
    pub mode: PriceScaleMode,
    pub min: f64,
    pub max: f64,
    pub plot: Rect,
}

impl PriceScale {
    pub fn new(mode: PriceScaleMode, min: f64, max: f64, plot: Rect) -> Self {
        let (min, max) = sanitize_range(min, max);
        Self {
            mode,
            min,
            max,
            plot,
        }
    }

    pub fn range(&self) -> (f64, f64) {
        (self.min, self.max)
    }

    /// True when a log mode is requested but the range cannot support it.
    pub fn log_is_degenerate(&self) -> bool {
        matches!(self.mode, PriceScaleMode::Log) && (self.min <= 0.0 || self.max <= 0.0)
    }

    fn effective_mode(&self) -> PriceScaleMode {
        if self.log_is_degenerate() {
            PriceScaleMode::Linear
        } else {
            self.mode
        }
    }

    /// Normalized position of a price: 0 at the bottom, 1 at the top.
    pub fn fraction(&self, price: f64) -> f64 {
        if !price.is_finite() {
            return 0.0;
        }
        let f = match self.effective_mode() {
            PriceScaleMode::Log => {
                let lo = self.min.ln();
                let hi = self.max.ln();
                (price.max(f64::MIN_POSITIVE).ln() - lo) / (hi - lo)
            }
            _ => (price - self.min) / (self.max - self.min),
        };
        match self.mode {
            PriceScaleMode::InvertedLinear => 1.0 - f,
            _ => f,
        }
    }

    pub fn to_y(&self, price: f64) -> f32 {
        let f = self.fraction(price).clamp(-1.0, 2.0);
        self.plot.bottom() - f as f32 * self.plot.h
    }

    pub fn to_price(&self, y: f32) -> f64 {
        if self.plot.h <= 0.0 {
            return self.min;
        }
        let mut f = ((self.plot.bottom() - y) / self.plot.h) as f64;
        f = f.clamp(0.0, 1.0);
        if matches!(self.mode, PriceScaleMode::InvertedLinear) {
            f = 1.0 - f;
        }
        match self.effective_mode() {
            PriceScaleMode::Log => {
                let lo = self.min.ln();
                let hi = self.max.ln();
                (lo + f * (hi - lo)).exp()
            }
            _ => self.min + f * (self.max - self.min),
        }
    }

    /// Nice ticks for the axis, in data units.
    pub fn ticks(&self, target: usize) -> Vec<Tick> {
        let target = target.max(2);
        let values = match self.effective_mode() {
            PriceScaleMode::Log => {
                let lo = self.min.ln();
                let hi = self.max.ln();
                nice_ticks(lo, hi, target)
                    .into_iter()
                    .map(f64::exp)
                    .filter(|v| v.is_finite() && *v >= self.min && *v <= self.max)
                    .collect()
            }
            PriceScaleMode::Percent { anchor_bits } => {
                let anchor = f64::from_bits(anchor_bits);
                if !anchor.is_finite() || anchor == 0.0 {
                    nice_ticks(self.min, self.max, target)
                } else {
                    let pmin = (self.min / anchor - 1.0) * 100.0;
                    let pmax = (self.max / anchor - 1.0) * 100.0;
                    nice_ticks(pmin.min(pmax), pmin.max(pmax), target)
                        .into_iter()
                        .map(|pct| anchor * (1.0 + pct / 100.0))
                        .filter(|v| v.is_finite() && *v >= self.min && *v <= self.max)
                        .collect()
                }
            }
            _ => nice_ticks(self.min, self.max, target),
        };
        values.into_iter().map(|value| Tick { value }).collect()
    }

    /// Percentage change of `price` from the scale's percent anchor.
    pub fn percent_change(&self, price: f64) -> Option<f64> {
        self.mode
            .anchor()
            .filter(|anchor| anchor.is_finite() && *anchor != 0.0)
            .map(|anchor| (price / anchor - 1.0) * 100.0)
    }
}

/// Expand degenerate or non-finite ranges into something drawable.
fn sanitize_range(min: f64, max: f64) -> (f64, f64) {
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    if max > min {
        return (min, max);
    }
    // A flat market still needs a visible band. Use a relative pad when the
    // value is large enough, otherwise a small absolute pad.
    let pad = if min.abs() > 1.0 {
        min.abs() * 0.001
    } else {
        0.5
    };
    (min - pad, min + pad)
}

/// Generate evenly spaced "nice" ticks within `[min, max]`.
pub fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    let target = target.max(2);
    let (min, max) = sanitize_range(min, max);
    let span = max - min;
    if !span.is_finite() || span <= 0.0 {
        return vec![min, max];
    }

    let rough = span / target as f64;
    let exponent = rough.log10().floor();
    let base = 10_f64.powf(exponent);
    let fraction = rough / base;
    let step = if fraction <= 1.0 {
        base
    } else if fraction <= 2.0 {
        2.0 * base
    } else if fraction <= 5.0 {
        5.0 * base
    } else {
        10.0 * base
    };
    if !step.is_finite() || step <= 0.0 {
        return vec![min, max];
    }

    let first = (min / step).ceil() * step;
    let mut ticks = Vec::with_capacity(target + 2);
    let mut value = first;
    let epsilon = step * 1e-9;
    while value <= max + epsilon && ticks.len() < target * 4 + 8 {
        // Avoid emitting -0.0.
        let normalized = if value.abs() < step * 1e-9 {
            0.0
        } else {
            value
        };
        ticks.push(normalized);
        value += step;
    }
    if ticks.is_empty() {
        ticks.push(min);
        ticks.push(max);
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 400.0)
    }

    #[test]
    fn linear_maps_bottom_and_top() {
        let s = PriceScale::new(PriceScaleMode::Linear, 100.0, 200.0, plot());
        assert_eq!(s.to_y(100.0), 400.0);
        assert_eq!(s.to_y(200.0), 0.0);
        assert!((s.to_y(150.0) - 200.0).abs() < 1e-4);
    }

    #[test]
    fn linear_inverts() {
        let s = PriceScale::new(PriceScaleMode::Linear, 100.0, 200.0, plot());
        for price in [100.0, 125.0, 150.0, 199.0, 200.0] {
            let y = s.to_y(price);
            assert!((s.to_price(y) - price).abs() < 1e-6);
        }
    }

    #[test]
    fn log_maps_geometric_midpoint_to_middle() {
        let s = PriceScale::new(PriceScaleMode::Log, 10.0, 1000.0, plot());
        assert!((s.to_y(100.0) - 200.0).abs() < 1e-4);
        let y = s.to_y(250.0);
        assert!((s.to_price(y) - 250.0).abs() < 1e-2);
    }

    #[test]
    fn log_falls_back_when_non_positive() {
        let s = PriceScale::new(PriceScaleMode::Log, -10.0, 10.0, plot());
        assert!(s.log_is_degenerate());
        // Falls back to a linear mapping rather than producing NaNs.
        assert!(s.to_y(0.0).is_finite());
    }

    #[test]
    fn inverted_scale_flips() {
        let normal = PriceScale::new(PriceScaleMode::Linear, 100.0, 200.0, plot());
        let inverted = PriceScale::new(PriceScaleMode::InvertedLinear, 100.0, 200.0, plot());
        assert!((normal.to_y(100.0) - inverted.to_y(200.0)).abs() < 1e-4);
        assert!((inverted.to_price(inverted.to_y(123.0)) - 123.0).abs() < 1e-2);
    }

    #[test]
    fn percent_ticks_are_relative_to_anchor() {
        let s = PriceScale::new(PriceScaleMode::percent(100.0), 90.0, 110.0, plot());
        let ticks = s.ticks(5);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().any(|t| (t.value - 100.0).abs() < 1e-9));
        assert!(ticks.iter().all(|t| t.value >= 90.0 && t.value <= 110.0));
        assert!((s.percent_change(110.0).unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_range_is_expanded() {
        let s = PriceScale::new(PriceScaleMode::Linear, 42.0, 42.0, plot());
        assert!(s.max > s.min);
        assert!(s.to_y(42.0).is_finite());
        assert!(!s.ticks(4).is_empty());
    }

    #[test]
    fn pathologically_tiny_range_is_expanded() {
        let s = PriceScale::new(PriceScaleMode::Linear, 1e-12, 1e-12, plot());
        assert!(s.max > s.min);
        assert!(s.to_price(s.to_y(1e-12)).is_finite());
    }

    #[test]
    fn nice_ticks_are_ordered_and_bounded() {
        let ticks = nice_ticks(0.0, 100.0, 5);
        assert!(ticks.len() >= 3);
        assert!(ticks.windows(2).all(|w| w[1] > w[0]));
        assert!(ticks.iter().all(|v| *v >= 0.0 && *v <= 100.0));
    }

    #[test]
    fn nice_ticks_handle_negative_ranges() {
        let ticks = nice_ticks(-100.0, -10.0, 5);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|v| *v >= -100.0 && *v <= -10.0));
    }

    #[test]
    fn non_finite_inputs_are_safe() {
        let s = PriceScale::new(PriceScaleMode::Linear, f64::NAN, f64::INFINITY, plot());
        assert!(s.to_y(0.0).is_finite());
        assert!(s.ticks(4).iter().all(|t| t.value.is_finite()));
    }
}
