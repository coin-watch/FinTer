//! Level-of-detail aggregation for dense candle views.
//!
//! Below a few pixels per slot, drawing individual candles produces visual
//! noise. The envelope preserves the exact high/low extremes while dropping the
//! interior detail that cannot be resolved.

use std::ops::Range;

use super::geometry::Rect;
use super::viewport::Viewport;

/// A neutral OHLCV bar used by the core. The product layer converts its candle
/// type into this shape at the scene boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ohlcv {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Ohlcv {
    pub const fn new(open: f64, high: f64, low: f64, close: f64, volume: f64) -> Self {
        Self {
            open,
            high,
            low,
            close,
            volume,
        }
    }
}

/// How candles should be drawn at the current slot width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodLevel {
    /// Full candles with bodies and wicks.
    Candles,
    /// Candles with reduced detail.
    ThinCandles,
    /// One vertical min/max envelope per pixel column.
    Envelope,
    /// Density band with no individual bars.
    Density,
}

impl LodLevel {
    pub fn for_slot_width(slot_width: f32) -> Self {
        if !slot_width.is_finite() || slot_width <= 0.0 {
            return Self::Density;
        }
        if slot_width >= 4.0 {
            Self::Candles
        } else if slot_width >= 1.5 {
            Self::ThinCandles
        } else if slot_width >= 0.6 {
            Self::Envelope
        } else {
            Self::Density
        }
    }
}

/// One aggregated column of candles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeBar {
    pub x: f32,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub first: usize,
    pub last: usize,
}

/// Aggregate visible candles into one envelope bar per pixel column.
///
/// The returned bars are sorted left-to-right. Columns with no visible candle
/// are omitted so data gaps remain gaps.
pub fn build_envelope(
    bars: &[Ohlcv],
    visible: Range<usize>,
    viewport: &Viewport,
    plot: Rect,
) -> Vec<EnvelopeBar> {
    if bars.is_empty() || plot.w <= 0.0 || plot.h <= 0.0 || viewport.span <= 0.0 {
        return Vec::new();
    }
    let start = visible.start.min(bars.len());
    let end = visible.end.min(bars.len());
    if start >= end {
        return Vec::new();
    }

    let columns = plot.w.round().max(1.0) as usize;
    let slot = plot.w as f64 / viewport.span;
    if !slot.is_finite() || slot <= 0.0 {
        return Vec::new();
    }

    let mut out: Vec<Option<EnvelopeBar>> = vec![None; columns];
    for index in start..end {
        let bar = bars[index];
        if !bar.high.is_finite()
            || !bar.low.is_finite()
            || !bar.open.is_finite()
            || !bar.close.is_finite()
        {
            continue;
        }
        let x = plot.x + ((index as f64 - viewport.start + 0.5) * slot) as f32;
        let relative = (x - plot.x) / plot.w;
        if !relative.is_finite() {
            continue;
        }
        let column = (relative * columns as f32)
            .floor()
            .clamp(0.0, columns as f32 - 1.0) as usize;
        match &mut out[column] {
            Some(agg) => {
                agg.high = agg.high.max(bar.high);
                agg.low = agg.low.min(bar.low);
                agg.close = bar.close;
                agg.volume += bar.volume.max(0.0);
                agg.last = index;
            }
            None => {
                out[column] = Some(EnvelopeBar {
                    x: plot.x + (column as f32 + 0.5) * (plot.w / columns as f32),
                    open: bar.open,
                    high: bar.high,
                    low: bar.low,
                    close: bar.close,
                    volume: bar.volume.max(0.0),
                    first: index,
                    last: index,
                });
            }
        }
    }

    out.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(open: f64, high: f64, low: f64, close: f64) -> Ohlcv {
        Ohlcv::new(open, high, low, close, 1.0)
    }

    fn plot() -> Rect {
        Rect::new(0.0, 0.0, 100.0, 200.0)
    }

    #[test]
    fn lod_thresholds() {
        assert_eq!(LodLevel::for_slot_width(10.0), LodLevel::Candles);
        assert_eq!(LodLevel::for_slot_width(4.0), LodLevel::Candles);
        assert_eq!(LodLevel::for_slot_width(2.0), LodLevel::ThinCandles);
        assert_eq!(LodLevel::for_slot_width(1.0), LodLevel::Envelope);
        assert_eq!(LodLevel::for_slot_width(0.2), LodLevel::Density);
        assert_eq!(LodLevel::for_slot_width(0.0), LodLevel::Density);
        assert_eq!(LodLevel::for_slot_width(f32::NAN), LodLevel::Density);
    }

    #[test]
    fn envelope_preserves_global_extremes() {
        let bars: Vec<_> = (0..100)
            .map(|i| {
                let base = i as f64;
                bar(base, base + 5.0, base - 5.0, base + 1.0)
            })
            .collect();
        let viewport = Viewport::new(0.0, 100.0);
        let envelope = build_envelope(&bars, 0..100, &viewport, plot());
        assert!(!envelope.is_empty());
        let high = envelope
            .iter()
            .map(|b| b.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let low = envelope.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
        assert_eq!(high, 104.0);
        assert_eq!(low, -5.0);
    }

    #[test]
    fn envelope_columns_do_not_exceed_plot_width() {
        let bars: Vec<_> = (0..10_000)
            .map(|i| bar(i as f64, i as f64, i as f64, i as f64))
            .collect();
        let viewport = Viewport::new(0.0, 10_000.0);
        let envelope = build_envelope(&bars, 0..10_000, &viewport, plot());
        assert!(envelope.len() <= 100);
        assert!(envelope.windows(2).all(|w| w[1].x > w[0].x));
    }

    #[test]
    fn envelope_first_and_last_match_visible_edges() {
        let bars: Vec<_> = (0..100)
            .map(|i| bar(i as f64, i as f64, i as f64, i as f64))
            .collect();
        let viewport = Viewport::new(0.0, 100.0);
        let envelope = build_envelope(&bars, 0..100, &viewport, plot());
        assert_eq!(envelope.first().unwrap().first, 0);
        assert_eq!(envelope.last().unwrap().last, 99);
    }

    #[test]
    fn envelope_skips_non_finite_bars() {
        let mut bars: Vec<_> = (0..10)
            .map(|i| bar(i as f64, i as f64, i as f64, i as f64))
            .collect();
        bars[3].high = f64::NAN;
        let viewport = Viewport::new(0.0, 10.0);
        let envelope = build_envelope(&bars, 0..10, &viewport, plot());
        assert!(envelope.iter().all(|b| b.high.is_finite()));
    }

    #[test]
    fn empty_inputs_return_empty() {
        let viewport = Viewport::new(0.0, 10.0);
        assert!(build_envelope(&[], 0..0, &viewport, plot()).is_empty());
        assert!(build_envelope(&[bar(1.0, 2.0, 0.5, 1.5)], 0..0, &viewport, plot()).is_empty());
    }
}
