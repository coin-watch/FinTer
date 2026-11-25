//! Pure order-flow analytics layered on top of the footprint types.
//!
//! The chart panel owns pixels; this module owns the derived, viewport-bound
//! view of the order-flow data. Everything here is pure and unit-testable.
//!
//! The visible-range volume profile (`VPVR`) is intentionally built from the
//! bars currently on screen. It never falls back to "all retained bars", which
//! would make the profile stick to the session instead of following the
//! viewport. The only exception is the backend session profile, used by the
//! caller when the visible bars have no order-flow rows at all.

use std::collections::BTreeMap;
use std::ops::Range;

use crate::chart::footprint::{
    DEFAULT_VALUE_AREA_PCT, FootprintBarView, bucket_index, infer_source_row_size,
};
use crate::data::liquidity_feed::LiquiditySample;
use crate::data::orderflow_feed::FlowBar;

/// Smallest price span treated as a usable denominator.
const MIN_RANGE_EPSILON: f64 = 1e-12;

// ---------------------------------------------------------------------------
// Visible-range volume profile (VPVR)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VolumeProfileRow {
    /// Midpoint of the price bucket this row aggregates.
    pub price: f64,
    pub price_low: f64,
    pub price_high: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub total: f64,
    /// True for the single row holding the point of control.
    pub is_poc: bool,
    pub is_vah: bool,
    pub is_val: bool,
}

impl VolumeProfileRow {
    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }
}

/// Aggregated buy/sell volume by price for one window of bars.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VolumeProfile {
    /// Rows in ascending price order, holes skipped.
    pub rows: Vec<VolumeProfileRow>,
    pub total_volume: f64,
    pub total_buy: f64,
    pub total_sell: f64,
    /// Price of the highest-volume row, `None` when nothing traded.
    pub poc: Option<f64>,
    /// Price at which the value area ends; grows upward from the POC.
    pub value_area_high: Option<f64>,
    /// Price at which the value area begins; grows downward from the POC.
    pub value_area_low: Option<f64>,
    /// Share of total volume actually enclosed by the value area.
    pub value_area_coverage: f64,
}

impl VolumeProfile {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn delta(&self) -> f64 {
        self.total_buy - self.total_sell
    }
}

/// Builds a visible-range volume profile from `bars` only.
///
/// `bars` should already be the visible slice chosen by the chart. `price_step`
/// is the bucket height; a non-positive or non-finite step falls back to the
/// smallest positive gap between observed source rows. Buckets are anchored to
/// absolute prices (`floor(price / step)`), so the profile does not shift when
/// the viewport moves. `value_area_pct` outside `0.0..=1.0` is clamped.
pub fn build_visible_profile(
    bars: &[FlowBar],
    price_step: f64,
    value_area_pct: f64,
) -> VolumeProfile {
    let pct = clamp_unit(value_area_pct);
    if pct <= 0.0 {
        return VolumeProfile::default();
    }
    let step = if price_step.is_finite() && price_step > 0.0 {
        price_step
    } else {
        infer_source_row_size(bars).unwrap_or(1.0)
    };
    if !step.is_finite() || step <= 0.0 {
        return VolumeProfile::default();
    }

    let mut buckets: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
    for bar in bars {
        for row in &bar.price_rows {
            let buy = sanitize_volume(row.buy_volume);
            let sell = sanitize_volume(row.sell_volume);
            if buy + sell <= 0.0 {
                continue;
            }
            let price = profile_price(row);
            if !price.is_finite() {
                continue;
            }
            let bucket = buckets
                .entry(bucket_index(price, step))
                .or_insert((0.0, 0.0));
            bucket.0 += buy;
            bucket.1 += sell;
        }
    }
    if buckets.is_empty() {
        return VolumeProfile::default();
    }

    let mut rows: Vec<VolumeProfileRow> = Vec::with_capacity(buckets.len());
    for (index, (buy, sell)) in buckets {
        let total = buy + sell;
        if total <= 0.0 || !total.is_finite() {
            continue;
        }
        let low = index as f64 * step;
        let high = index.saturating_add(1) as f64 * step;
        rows.push(VolumeProfileRow {
            price: (low + high) / 2.0,
            price_low: low,
            price_high: high,
            buy_volume: buy,
            sell_volume: sell,
            total,
            is_poc: false,
            is_vah: false,
            is_val: false,
        });
    }
    if rows.is_empty() {
        return VolumeProfile::default();
    }

    let total_volume: f64 = rows.iter().map(|row| row.total).sum();
    if total_volume <= 0.0 || !total_volume.is_finite() {
        return VolumeProfile::default();
    }
    let total_buy: f64 = rows.iter().map(|row| row.buy_volume).sum();
    let total_sell: f64 = rows.iter().map(|row| row.sell_volume).sum();

    // POC: highest volume, earliest row on ties.
    let mut poc_index = 0usize;
    for (index, row) in rows.iter().enumerate() {
        if row.total > rows[poc_index].total {
            poc_index = index;
        }
    }
    rows[poc_index].is_poc = true;

    let (low_index, high_index, coverage) = expand_value_area(&rows, poc_index, total_volume, pct);
    rows[low_index].is_val = true;
    rows[high_index].is_vah = true;

    VolumeProfile {
        poc: Some(rows[poc_index].price),
        value_area_low: Some(rows[low_index].price_low),
        value_area_high: Some(rows[high_index].price_high),
        value_area_coverage: coverage,
        rows,
        total_volume,
        total_buy,
        total_sell,
    }
}

fn profile_price(row: &crate::data::orderflow_feed::PriceFlow) -> f64 {
    let mid = (row.price_low + row.price_high) / 2.0;
    if mid.is_finite() { mid } else { row.price }
}

/// Grows a value area around the POC until it covers `pct` of `total_volume`.
///
/// At each step compare the two neighbouring rows (one below, one above) and
/// take the larger; upward wins ties. Expansion stops at the target, when both
/// sides are exhausted, or after a bounds-safe number of steps.
fn expand_value_area(
    rows: &[VolumeProfileRow],
    poc_index: usize,
    total_volume: f64,
    pct: f64,
) -> (usize, usize, f64) {
    if rows.is_empty() || poc_index >= rows.len() || total_volume <= 0.0 {
        return (0, 0, 0.0);
    }
    let target = total_volume * pct;
    let mut low = poc_index;
    let mut high = poc_index;
    let mut accumulated = rows[poc_index].total;

    for _ in 0..rows.len().saturating_mul(2).max(2) {
        if accumulated >= target {
            break;
        }
        let up = rows.get(high + 1).map(|row| row.total);
        let down = low
            .checked_sub(1)
            .and_then(|index| rows.get(index))
            .map(|row| row.total);
        match (up, down) {
            (None, None) => break,
            (Some(up), None) => {
                high += 1;
                accumulated += up;
            }
            (None, Some(down)) => {
                low -= 1;
                accumulated += down;
            }
            (Some(up), Some(down)) => {
                if up >= down {
                    high += 1;
                    accumulated += up;
                } else {
                    low -= 1;
                    accumulated += down;
                }
            }
        }
    }

    let coverage = if total_volume > MIN_RANGE_EPSILON {
        (accumulated / total_volume).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (low, high, coverage)
}

fn sanitize_volume(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn clamp_unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        DEFAULT_VALUE_AREA_PCT
    }
}

// ---------------------------------------------------------------------------
// Visible-range selection helpers
// ---------------------------------------------------------------------------

/// Range of `bars` whose timestamps fall inside `[first_ts, last_ts]`.
///
/// Uses `partition_point` because `FlowBar`s are timestamp-sorted; the chart
/// never scans its whole retained history to find the visible slice.
pub fn visible_bar_range(bars: &[FlowBar], first_ts: i64, last_ts: i64) -> Range<usize> {
    if bars.is_empty() || last_ts < first_ts {
        return 0..0;
    }
    let start = bars.partition_point(|bar| bar.timestamp < first_ts);
    let end = bars.partition_point(|bar| bar.timestamp <= last_ts);
    start.min(end)..end
}

/// Range of book-history samples whose timestamps fall inside `[first_ts,
/// last_ts]`. Samples outside the window are skipped rather than clamped to the
/// plot edge.
pub fn visible_sample_range(
    samples: &[LiquiditySample],
    first_ts: i64,
    last_ts: i64,
) -> Range<usize> {
    if samples.is_empty() || last_ts < first_ts {
        return 0..0;
    }
    let start = samples.partition_point(|sample| sample.timestamp_ms < first_ts);
    let end = samples.partition_point(|sample| sample.timestamp_ms <= last_ts);
    start.min(end)..end
}

/// True when the backend session-profile window overlaps the visible range.
///
/// A profile event without a usable window (`0, 0`) is treated as the whole
/// session, so it overlaps any visible range. That keeps old backends usable
/// while still refusing to paint a session profile over an unrelated window.
pub fn profile_window_overlaps(
    window_start: i64,
    window_end: i64,
    visible_start: i64,
    visible_end: i64,
) -> bool {
    if visible_end < visible_start {
        return false;
    }
    if window_start <= 0 && window_end <= 0 {
        return true;
    }
    window_start <= visible_end && window_end >= visible_start
}

// ---------------------------------------------------------------------------
// Absorption
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbsorptionDirection {
    /// Buyers kept lifting but the bar closed lower or flat: sellers absorbed.
    Selling,
    /// Sellers kept hitting but the bar closed higher or flat: buyers absorbed.
    Buying,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AbsorptionMarker {
    pub timestamp: i64,
    /// Bar close, as the pipeline's `range` uses it as the reference price.
    pub price: f64,
    pub direction: AbsorptionDirection,
    /// `0.0..=1.0`; higher means heavier volume relative to its window.
    pub strength: f64,
    pub delta: f64,
    pub volume: f64,
    pub range: f64,
}

/// Detects bars where aggressive volume failed to move price.
///
/// Definition: a bar whose volume is in the top decile of the window (`volume >=
/// decile_90`) while its range is below the window's median range. Direction is
/// the sign of the bar's delta: buyers aggressed (`delta > 0`) but price stalled,
/// so sellers absorbed (`Selling`); sellers aggressed but price held, so buyers
/// absorbed (`Buying`). A zero delta carries no aggressor to absorb, so it is
/// skipped.
///
/// Strength is the normalised position of the bar's volume between the decile
/// threshold and the window maximum, so the heaviest absorbing bar scores `1.0`.
/// Needs at least two bars to build a meaningful distribution; fewer returns empty.
pub fn detect_absorption(bars: &[FlowBar]) -> Vec<AbsorptionMarker> {
    if bars.len() < 2 {
        return Vec::new();
    }

    // Live-session rule: a bar without real price rows has no printed ladder,
    // so it must not contribute a (fabricated) zero-price print to the window.
    let live: Vec<&FlowBar> = bars
        .iter()
        .filter(|bar| !bar.price_rows.is_empty())
        .collect();
    if live.len() < 2 {
        return Vec::new();
    }

    let mut volumes: Vec<f64> = Vec::with_capacity(live.len());
    let mut ranges: Vec<f64> = Vec::with_capacity(live.len());
    for bar in &live {
        volumes.push(bar_volume(bar));
        ranges.push(bar_range(bar));
    }
    if volumes
        .iter()
        .any(|volume| !volume.is_finite() || *volume < 0.0)
    {
        return Vec::new();
    }

    let mut sorted_volumes = volumes.clone();
    sorted_volumes.sort_by(f64::total_cmp);
    let mut sorted_ranges = ranges.clone();
    sorted_ranges.sort_by(f64::total_cmp);

    let decile = percentile(&sorted_volumes, 0.9);
    let median_range = percentile(&sorted_ranges, 0.5);
    let max_volume = *sorted_volumes.last().unwrap_or(&0.0);
    let denominator = max_volume - decile;

    let mut out = Vec::new();
    for (index, bar) in live.iter().enumerate() {
        let volume = volumes[index];
        let range = ranges[index];
        // A zero-volume bar is not a top-decile print; `>=` alone would let an
        // all-zero window match every bar.
        if !volume.is_finite() || volume <= 0.0 || volume < decile || range >= median_range {
            continue;
        }
        let delta = bar_delta(bar);
        let direction = if delta > 0.0 {
            AbsorptionDirection::Selling
        } else if delta < 0.0 {
            AbsorptionDirection::Buying
        } else {
            continue;
        };
        let strength = if denominator > MIN_RANGE_EPSILON {
            ((volume - decile) / denominator).clamp(0.0, 1.0)
        } else {
            1.0
        };
        out.push(AbsorptionMarker {
            timestamp: bar.timestamp,
            price: bar_price(bar),
            direction,
            strength,
            delta,
            volume,
            range,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Divergence
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DivergenceKind {
    /// Price higher high, cumulative delta lower high: buyers are exhausting.
    Bearish,
    /// Price lower low, cumulative delta higher low: sellers are exhausting.
    Bullish,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Divergence {
    pub kind: DivergenceKind,
    /// Earlier swing bar.
    pub first_timestamp: i64,
    pub first_price: f64,
    pub first_delta: f64,
    /// Later swing bar.
    pub second_timestamp: i64,
    pub second_price: f64,
    pub second_delta: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwingKind {
    High,
    Low,
}

#[derive(Clone, Copy, Debug)]
struct Swing {
    timestamp: i64,
    price: f64,
    cvd: f64,
}

#[derive(Clone, Copy, Debug)]
struct BarPoint {
    timestamp: i64,
    price: f64,
    cvd: f64,
}

/// Finds price/CVD divergences across swing points.
///
/// `cvd` is the cumulative delta series aligned with `bars` (`(timestamp, cvd)`),
/// as carried by `OrderFlowSnapshot`. Bars without a matching CVD entry are
/// dropped before swings are located, so misaligned input can never compare the
/// delta of one bar with the price of another.
///
/// Swings are strict local highs/lows over the last `lookback` bars. Consecutive
/// highs are compared (higher high in price with lower high in CVD is `Bearish`);
/// consecutive lows likewise (`Bullish`). Comparisons use an `f64::EPSILON`-scaled
/// tolerance so flat prices and equal deltas do not register as divergences.
pub fn detect_divergences(
    bars: &[FlowBar],
    cvd: &[(i64, f64)],
    lookback: usize,
) -> Vec<Divergence> {
    let points = align_points(bars, cvd);
    if points.len() < 3 {
        return Vec::new();
    }
    let window = if lookback == 0 {
        points.len()
    } else {
        lookback.min(points.len())
    };
    let start = points.len() - window;
    let slice = &points[start..];

    let mut out = Vec::new();
    for (kind, swing_kind) in [
        (DivergenceKind::Bearish, SwingKind::High),
        (DivergenceKind::Bullish, SwingKind::Low),
    ] {
        let swings = collect_swings(slice, swing_kind);
        // A short window can yield one swing; that is simply no comparison.
        for pair in swings.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            let divergence = match kind {
                DivergenceKind::Bearish => {
                    strictly_greater(second.price, first.price)
                        && strictly_less(second.cvd, first.cvd)
                }
                DivergenceKind::Bullish => {
                    strictly_less(second.price, first.price)
                        && strictly_greater(second.cvd, first.cvd)
                }
            };
            if divergence {
                out.push(Divergence {
                    kind,
                    first_timestamp: first.timestamp,
                    first_price: first.price,
                    first_delta: first.cvd,
                    second_timestamp: second.timestamp,
                    second_price: second.price,
                    second_delta: second.cvd,
                });
            }
        }
    }
    out.sort_by_key(|divergence| divergence.second_timestamp);
    out
}

fn strictly_greater(a: f64, b: f64) -> bool {
    a > b + f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

fn strictly_less(a: f64, b: f64) -> bool {
    b > a + f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

/// Joins bars with CVD by timestamp, preserving bar order and keeping only
/// finite points. Bars without a matching CVD timestamp are skipped; repeated
/// timestamps keep the later CVD value.
fn align_points(bars: &[FlowBar], cvd: &[(i64, f64)]) -> Vec<BarPoint> {
    if bars.is_empty() || cvd.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(bars.len());
    for bar in bars {
        // Keep the live-session rule: only bars with a real traded ladder can
        // participate in a price/CVD swing.
        if bar.price_rows.is_empty() {
            continue;
        }
        let Some((_, value)) = cvd.iter().rev().find(|(ts, _)| *ts == bar.timestamp) else {
            continue;
        };
        let price = bar_price(bar);
        if !price.is_finite() || !value.is_finite() {
            continue;
        }
        out.push(BarPoint {
            timestamp: bar.timestamp,
            price,
            cvd: *value,
        });
    }
    out
}

/// Locates strict swing highs or lows over the window. Swings are returned in bar
/// order, which is how consecutive pairs are compared. The module-local scan
/// keeps this file a pure leaf that only depends on the footprint types; it is
/// deliberately strict so a flat stretch produces no swing rather than a
/// plateau of fake ones.
fn collect_swings(points: &[BarPoint], kind: SwingKind) -> Vec<Swing> {
    let mut out = Vec::new();
    if points.len() < 3 {
        return out;
    }
    for index in 1..points.len() - 1 {
        let previous = &points[index - 1];
        let current = &points[index];
        let next = &points[index + 1];
        let is_swing = match kind {
            SwingKind::High => {
                current.price > previous.price + swing_epsilon(current.price, previous.price)
                    && current.price > next.price + swing_epsilon(current.price, next.price)
            }
            SwingKind::Low => {
                current.price < previous.price - swing_epsilon(current.price, previous.price)
                    && current.price < next.price - swing_epsilon(current.price, next.price)
            }
        };
        if is_swing {
            out.push(Swing {
                timestamp: current.timestamp,
                price: current.price,
                cvd: current.cvd,
            });
        }
    }
    out
}

fn swing_epsilon(a: f64, b: f64) -> f64 {
    f64::EPSILON * a.abs().max(b.abs()).max(1.0) * 4.0
}

// ---------------------------------------------------------------------------
// Stacked imbalance
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImbalanceSide {
    /// Bid-side imbalance: buy volume outweighing the ask one row below.
    Buy,
    /// Ask-side imbalance: sell volume outweighing the bid one row above.
    Sell,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImbalanceStack {
    pub timestamp: i64,
    pub side: ImbalanceSide,
    /// Row indices into the source `FootprintBarView::rows`, inclusive.
    pub row_start: usize,
    pub row_end: usize,
    /// Price span of the stacked rows.
    pub price_low: f64,
    pub price_high: f64,
    pub delta: f64,
    pub volume: f64,
}

/// Finds runs of at least `min_stacked` consecutive rows imbalanced on the same
/// diagonal. Reuses the per-row `imbalance` classification from the footprint
/// grid (`1` = bid/buy imbalance, `-1` = ask/sell imbalance, `0` = none).
///
/// A diagonal stack is the standard footprint read: buyers repeatedly lifting
/// the offer row over row, or sellers repeatedly hitting the bid. `min_stacked`
/// is clamped to at least 2 because a single row is not a stack. Bars with no
/// rows and bars whose imbalance is all zero are simply skipped.
pub fn detect_stacked_imbalances(
    grid: &FootprintBarView,
    min_stacked: usize,
) -> Vec<ImbalanceStack> {
    detect_stacks_in(&grid.rows, grid.timestamp, min_stacked)
}

/// Stack detection over any bar whose rows carry the footprint `imbalance`
/// flag. Kept public so callers holding only `FootprintBarView`-like rows (for
/// example a test fixture) can reuse the exact grid behaviour.
pub fn detect_stacks_in(
    rows: &[crate::chart::footprint::FootprintRowView],
    timestamp: i64,
    min_stacked: usize,
) -> Vec<ImbalanceStack> {
    let threshold = min_stacked.max(2);
    let mut out = Vec::new();
    let mut start: Option<(usize, ImbalanceSide)> = None;

    // Closing a run flushes it only when it is long enough; the run boundaries
    // are always row indices into `rows`.
    let close =
        |open: Option<(usize, ImbalanceSide)>, end: usize, out: &mut Vec<ImbalanceStack>| {
            let Some((begin, side)) = open else {
                return;
            };
            if end < begin || end >= rows.len() || end + 1 - begin < threshold {
                return;
            }
            if let Some(stack) = stack_from(rows, timestamp, begin, end, side) {
                out.push(stack);
            }
        };

    for (index, row) in rows.iter().enumerate() {
        let side = match row.imbalance {
            1 => Some(ImbalanceSide::Buy),
            -1 => Some(ImbalanceSide::Sell),
            _ => None,
        };
        match (side, start) {
            (Some(side), Some((_, running))) if running == side => {}
            (Some(side), _) => {
                close(start.take(), index.saturating_sub(1), &mut out);
                start = Some((index, side));
            }
            (None, _) => close(start.take(), index.saturating_sub(1), &mut out),
        }
    }
    if !rows.is_empty() {
        close(start.take(), rows.len() - 1, &mut out);
    }
    out
}

fn stack_from(
    rows: &[crate::chart::footprint::FootprintRowView],
    timestamp: i64,
    row_start: usize,
    row_end: usize,
    side: ImbalanceSide,
) -> Option<ImbalanceStack> {
    if row_end >= rows.len() || row_start > row_end {
        return None;
    }
    let mut price_low = f64::INFINITY;
    let mut price_high = f64::NEG_INFINITY;
    let mut delta = 0.0;
    let mut volume = 0.0;
    for row in &rows[row_start..=row_end] {
        if !row.price_low.is_finite() || !row.price_high.is_finite() {
            return None;
        }
        price_low = price_low.min(row.price_low);
        price_high = price_high.max(row.price_high);
        delta += row.delta();
        volume += row.volume();
    }
    if !price_low.is_finite() || !price_high.is_finite() {
        return None;
    }
    Some(ImbalanceStack {
        timestamp,
        side,
        row_start,
        row_end,
        price_low,
        price_high,
        delta,
        volume,
    })
}

// ---------------------------------------------------------------------------
// CVD regime
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CvdRegime {
    Rising,
    Falling,
    #[default]
    Flat,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CvdSeries {
    /// Aligned `(timestamp, cvd)` pairs after filtering non-finite inputs.
    pub points: Vec<(i64, f64)>,
    /// Ordinary-least-squares slope in CVD units per bar, normalised by the
    /// window's mean absolute delta. `0.0` when it cannot be computed.
    pub slope: f64,
    pub regime: CvdRegime,
    pub lookback: usize,
}

/// Classifies the recent CVD trend from a normalised least-squares slope.
///
/// The slope is divided by the mean absolute per-bar delta in the window so it is
/// dimensionless: `1.0` means the trend consumes about one average delta per bar.
/// Below `flat_threshold` (typically `0.05`) the regime is `Flat`. Taking the CVD
/// *difference* slope rather than raw levels also makes the classification immune
/// to the absolute CVD offset, which is what makes it comparable across sessions.
///
/// Fewer than `2` usable points, a zero denominator, or non-finite intermediates
/// all yield the default series with a `Flat` regime.
pub fn build_cvd_series(cvd: &[(i64, f64)], lookback: usize, flat_threshold: f64) -> CvdSeries {
    let points: Vec<(i64, f64)> = cvd
        .iter()
        .filter(|(ts, value)| *ts > 0 && value.is_finite() && !value.is_nan())
        .copied()
        .collect();
    if points.len() < 2 {
        return CvdSeries::default();
    }
    let window = if lookback == 0 {
        points.len()
    } else {
        lookback.min(points.len())
    };
    let window_points = &points[points.len() - window..];
    if window_points.len() < 2 {
        return CvdSeries::default();
    }

    let n = window_points.len() as f64;
    let mean_x = (window_points.len() - 1) as f64 / 2.0;
    let mean_y = window_points.iter().map(|(_, value)| *value).sum::<f64>() / n;
    let mut covariance = 0.0;
    let mut variance = 0.0;
    for (index, (_, value)) in window_points.iter().enumerate() {
        let dx = index as f64 - mean_x;
        covariance += dx * (*value - mean_y);
        variance += dx * dx;
    }
    if variance <= 0.0 || !variance.is_finite() {
        return CvdSeries {
            points,
            slope: 0.0,
            regime: CvdRegime::Flat,
            lookback: window,
        };
    }
    let raw_slope = covariance / variance;

    // Normaliser: average absolute step of the underlying CVD path.
    let mut step_total = 0.0;
    for pair in window_points.windows(2) {
        step_total += (pair[1].1 - pair[0].1).abs();
    }
    let mean_step = step_total / (window_points.len() - 1) as f64;

    let slope = if mean_step > MIN_RANGE_EPSILON && raw_slope.is_finite() {
        (raw_slope / mean_step).clamp(-1.0e6, 1.0e6)
    } else {
        0.0
    };
    let threshold = if flat_threshold.is_finite() {
        flat_threshold.abs()
    } else {
        0.0
    };
    let regime = if slope > threshold {
        CvdRegime::Rising
    } else if slope < -threshold {
        CvdRegime::Falling
    } else {
        CvdRegime::Flat
    };

    CvdSeries {
        points,
        slope,
        regime,
        lookback: window,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Bar price used by range-based signals: the midpoint of the traded price rows,
/// falling back to the close of the first row (the pipeline stores rows in
/// ascending price order, so its close is the last row).
pub fn bar_price(bar: &FlowBar) -> f64 {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut count = 0usize;
    for row in &bar.price_rows {
        if !row.price.is_finite() {
            continue;
        }
        min = min.min(row.price);
        max = max.max(row.price);
        count += 1;
    }
    if count == 0 { 0.0 } else { (min + max) / 2.0 }
}

/// High minus low of the traded prices. Uses the same last-row-as-close convention
/// as `bar_price`. Returns `0.0` for a bar with no usable rows.
pub fn bar_range(bar: &FlowBar) -> f64 {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut count = 0usize;
    let mut last = f64::NAN;
    for row in &bar.price_rows {
        if !row.price.is_finite() {
            continue;
        }
        min = min.min(row.price);
        max = max.max(row.price);
        last = row.price;
        count += 1;
    }
    if count == 0 || !last.is_finite() {
        0.0
    } else {
        (max - min).max(0.0)
    }
}

fn bar_volume(bar: &FlowBar) -> f64 {
    let sum: f64 = bar
        .price_rows
        .iter()
        .map(|row| sanitize_volume(row.buy_volume) + sanitize_volume(row.sell_volume))
        .sum();
    if sum > 0.0 && sum.is_finite() {
        return sum;
    }
    let total = sanitize_volume(bar.total_volume);
    if total > 0.0 {
        return total;
    }
    sanitize_volume(bar.buy_volume) + sanitize_volume(bar.sell_volume)
}

fn bar_delta(bar: &FlowBar) -> f64 {
    let from_rows: f64 = bar
        .price_rows
        .iter()
        .map(|row| sanitize_volume(row.buy_volume) - sanitize_volume(row.sell_volume))
        .sum();
    if from_rows.abs() > 0.0 {
        from_rows
    } else {
        sanitize_volume(bar.buy_volume) - sanitize_volume(bar.sell_volume)
    }
}

/// Linear-interpolated percentile over an ascending-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = (sorted.len() - 1) as f64 * p.clamp(0.0, 1.0);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let fraction = position - lower as f64;
    sorted[lower] + (sorted[upper] - sorted[lower]) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::footprint::FootprintRowView;
    use crate::data::liquidity_feed::{LiquidityLevel, LiquiditySample};
    use crate::data::orderflow_feed::PriceFlow;

    fn price_row(price: f64, buy: f64, sell: f64) -> PriceFlow {
        PriceFlow {
            price,
            price_low: price,
            price_high: price,
            buy_volume: buy,
            sell_volume: sell,
            buy_count: u32::from(buy > 0.0),
            sell_count: u32::from(sell > 0.0),
        }
    }

    fn flow_bar(timestamp: i64, rows: Vec<PriceFlow>) -> FlowBar {
        let buy_volume = rows.iter().map(|row| row.buy_volume).sum();
        let sell_volume = rows.iter().map(|row| row.sell_volume).sum();
        FlowBar {
            timestamp,
            buy_volume,
            sell_volume,
            trades: rows.len() as u32,
            price_rows: rows,
            total_volume: buy_volume + sell_volume,
            ..FlowBar::default()
        }
    }

    fn flat_bar(timestamp: i64, price: f64, buy: f64, sell: f64) -> FlowBar {
        flow_bar(timestamp, vec![price_row(price, buy, sell)])
    }

    fn sample(timestamp_ms: i64) -> LiquiditySample {
        LiquiditySample {
            timestamp_ms,
            levels: vec![LiquidityLevel {
                price: 100.0,
                quantity: 1.0,
            }],
            ..LiquiditySample::default()
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn visible_profile_poc_and_value_area_match_hand_computation() {
        // Two price levels with step 1: 100 trades vol 25, 101 trades vol 75.
        // Total 100, POC at 101. Target 70% = 70: POC gives 75 >= 70, so the
        // value area is the single row at 101 -> VAH 102, VAL 101, coverage 0.75.
        let bars = vec![flow_bar(
            1,
            vec![price_row(100.0, 10.0, 15.0), price_row(101.0, 50.0, 25.0)],
        )];
        let profile = build_visible_profile(&bars, 1.0, DEFAULT_VALUE_AREA_PCT);

        assert_eq!(profile.rows.len(), 2);
        assert!(approx(profile.poc.unwrap(), 101.5));
        assert!(profile.rows[1].is_poc);
        assert!(profile.rows[1].is_vah && profile.rows[1].is_val);
        assert!(!profile.rows[0].is_poc);
        assert!(approx(profile.value_area_low.unwrap(), 101.0));
        assert!(approx(profile.value_area_high.unwrap(), 102.0));
        assert!(approx(profile.value_area_coverage, 0.75));
        assert!(approx(profile.total_buy, 60.0));
        assert!(approx(profile.total_sell, 40.0));
        assert!(approx(profile.delta(), 20.0));
    }

    #[test]
    fn value_area_expands_downward_to_reach_coverage() {
        // 30 / 20 / 50 at 100 / 101 / 102. POC 102 holds 50 < 70; the bigger
        // neighbour is 101 (20) so it is added (coverage 0.70), stopping there.
        let bars = vec![flow_bar(
            1,
            vec![
                price_row(100.0, 30.0, 0.0),
                price_row(101.0, 20.0, 0.0),
                price_row(102.0, 50.0, 0.0),
            ],
        )];
        let profile = build_visible_profile(&bars, 1.0, DEFAULT_VALUE_AREA_PCT);
        assert!(approx(profile.poc.unwrap(), 102.5));
        assert!(approx(profile.value_area_low.unwrap(), 101.0));
        assert!(approx(profile.value_area_high.unwrap(), 103.0));
        assert!(approx(profile.value_area_coverage, 0.70));
    }

    #[test]
    fn visible_profile_changes_with_the_visible_window() {
        // The same session data yields a different POC once the window slides.
        let bars = [
            flow_bar(1, vec![price_row(100.0, 30.0, 0.0)]),
            flow_bar(2, vec![price_row(101.0, 5.0, 0.0)]),
            flow_bar(3, vec![price_row(102.0, 40.0, 0.0)]),
        ];
        let first_two = build_visible_profile(&bars[..2], 1.0, DEFAULT_VALUE_AREA_PCT);
        let last_two = build_visible_profile(&bars[1..], 1.0, DEFAULT_VALUE_AREA_PCT);

        assert!(approx(first_two.poc.unwrap(), 100.5));
        assert!(approx(last_two.poc.unwrap(), 102.5));
        assert_ne!(first_two.rows, last_two.rows);
        assert_ne!(first_two.total_volume, last_two.total_volume);
    }

    #[test]
    fn profile_does_not_clamp_the_number_of_rows() {
        // The old implementation capped at 240 rows and silently merged the
        // overflow into the last bucket. Absolute sparse buckets must keep all
        // traded levels.
        let rows: Vec<PriceFlow> = (0..400)
            .map(|index| price_row(100.0 + index as f64, 1.0, 0.0))
            .collect();
        let bars = vec![flow_bar(1, rows)];
        let profile = build_visible_profile(&bars, 1.0, DEFAULT_VALUE_AREA_PCT);
        assert_eq!(profile.rows.len(), 400);
        assert_eq!(profile.rows[0].price_low, 100.0);
        assert_eq!(profile.rows[399].price_low, 499.0);
    }

    #[test]
    fn step_is_inferred_from_unsorted_rows_within_one_bar() {
        // min gap 2 between 101 and 103; absolute buckets 100..102 and 102..104.
        let bars = vec![flow_bar(
            1,
            vec![price_row(103.0, 1.0, 0.0), price_row(101.0, 1.0, 1.0)],
        )];
        let profile = build_visible_profile(&bars, 0.0, DEFAULT_VALUE_AREA_PCT);
        assert_eq!(profile.rows.len(), 2);
        assert_eq!(profile.rows[0].total, 2.0);
        assert_eq!(profile.rows[1].total, 1.0);
        assert!(approx(profile.poc.unwrap(), 101.0));
        assert!(approx(profile.value_area_coverage, 1.0));
    }

    #[test]
    fn single_price_profile_is_its_own_value_area() {
        let bars = vec![flat_bar(1, 500.0, 2.0, 3.0)];
        let profile = build_visible_profile(&bars, 0.0, DEFAULT_VALUE_AREA_PCT);
        assert_eq!(profile.rows.len(), 1);
        let row = &profile.rows[0];
        assert!(row.is_poc && row.is_vah && row.is_val);
        assert!(approx(profile.poc.unwrap(), 500.5));
        assert!(approx(profile.value_area_coverage, 1.0));
        assert!(profile.rows.iter().all(|row| row.total.is_finite()));
    }

    #[test]
    fn empty_and_zero_volume_profiles_are_empty() {
        assert_eq!(
            build_visible_profile(&[], 1.0, DEFAULT_VALUE_AREA_PCT),
            VolumeProfile::default()
        );
        let zero = vec![flat_bar(1, 100.0, 0.0, 0.0), flat_bar(2, 101.0, 0.0, 0.0)];
        let profile = build_visible_profile(&zero, 1.0, DEFAULT_VALUE_AREA_PCT);
        assert!(profile.rows.is_empty());
        assert_eq!(profile.poc, None);
        assert_eq!(profile.value_area_coverage, 0.0);
        // Non-finite step and pct still return a usable empty profile.
        assert!(
            build_visible_profile(&zero, f64::NAN, f64::NAN)
                .rows
                .is_empty()
        );
    }

    #[test]
    fn visible_bar_range_is_bounded_and_sorted() {
        let bars: Vec<FlowBar> = (1..=10)
            .map(|ts| flat_bar(ts * 1_000, 100.0, 1.0, 0.0))
            .collect();
        assert_eq!(visible_bar_range(&bars, 3_000, 6_000), 2..6);
        assert_eq!(visible_bar_range(&bars, 0, 0), 0..0);
        assert_eq!(visible_bar_range(&bars, 50_000, 60_000), 10..10);
        assert_eq!(visible_bar_range(&[], 1, 2), 0..0);
    }

    #[test]
    fn visible_sample_range_is_bounded_to_the_window() {
        let samples: Vec<LiquiditySample> = (1..=10).map(|ts| sample(ts * 1_000)).collect();
        assert_eq!(visible_sample_range(&samples, 3_000, 6_000), 2..6);
        assert_eq!(visible_sample_range(&samples, 0, 0), 0..0);
        assert_eq!(visible_sample_range(&samples, 50_000, 60_000), 10..10);
        assert_eq!(visible_sample_range(&[], 1, 2), 0..0);
    }

    #[test]
    fn profile_window_overlap_is_explicit() {
        assert!(profile_window_overlaps(0, 0, 1_000, 2_000));
        assert!(profile_window_overlaps(1_000, 5_000, 2_000, 3_000));
        assert!(profile_window_overlaps(5_000, 6_000, 2_000, 5_000));
        assert!(!profile_window_overlaps(6_000, 7_000, 2_000, 5_000));
        assert!(!profile_window_overlaps(1_000, 2_000, 5_000, 7_000));
    }

    fn footprint_row(index: usize, imbalance: i8) -> FootprintRowView {
        FootprintRowView {
            price_low: 100.0 + index as f64,
            price_high: 101.0 + index as f64,
            buy: 3.0,
            sell: 1.0,
            buy_count: 1,
            sell_count: 1,
            is_poc: false,
            is_vah: false,
            is_val: false,
            imbalance,
        }
    }

    fn footprint_bar(timestamp: i64, rows: Vec<FootprintRowView>) -> FootprintBarView {
        FootprintBarView {
            timestamp,
            rows,
            delta: 0.0,
            total_volume: 0.0,
            poc: None,
            vah: None,
            val: None,
            is_closed: true,
        }
    }

    // --- Absorption -------------------------------------------------------

    fn absorption_window() -> Vec<FlowBar> {
        // Mostly trending bars of range 4 with volume 10; the absorbing bar at
        // price 100 has the window's heaviest volume but a 1-wide range and
        // positive delta (buyers aggressed into sellers).
        vec![
            flow_bar(
                1,
                vec![price_row(98.0, 5.0, 5.0), price_row(102.0, 5.0, 5.0)],
            ),
            flow_bar(
                2,
                vec![price_row(99.0, 5.0, 5.0), price_row(103.0, 5.0, 5.0)],
            ),
            flat_bar(3, 100.0, 50.0, 10.0),
            flow_bar(
                4,
                vec![price_row(101.0, 5.0, 5.0), price_row(105.0, 5.0, 5.0)],
            ),
            flow_bar(
                5,
                vec![price_row(102.0, 6.0, 6.0), price_row(106.0, 6.0, 6.0)],
            ),
            flow_bar(
                6,
                vec![price_row(103.0, 7.0, 7.0), price_row(107.0, 7.0, 7.0)],
            ),
            flow_bar(
                7,
                vec![price_row(104.0, 8.0, 8.0), price_row(108.0, 8.0, 8.0)],
            ),
            flow_bar(
                8,
                vec![price_row(105.0, 9.0, 9.0), price_row(109.0, 9.0, 9.0)],
            ),
            flow_bar(
                9,
                vec![price_row(106.0, 10.0, 10.0), price_row(110.0, 10.0, 10.0)],
            ),
            flow_bar(
                10,
                vec![price_row(107.0, 11.0, 11.0), price_row(111.0, 11.0, 11.0)],
            ),
        ]
    }

    #[test]
    fn heavy_volume_with_no_progress_is_absorption() {
        let markers = detect_absorption(&absorption_window());
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].timestamp, 3);
        assert_eq!(markers[0].direction, AbsorptionDirection::Selling);
        assert!((0.0..=1.0).contains(&markers[0].strength));
        assert!(approx(markers[0].strength, 1.0));
    }

    #[test]
    fn heavy_volume_with_progress_is_not_absorption() {
        // Same volumes and deltas as the absorbing bar, but the bar's prints span
        // the full trending range, so it is a trending bar and must not flag.
        let mut bars = absorption_window();
        bars[2] = flow_bar(
            3,
            vec![price_row(96.0, 50.0, 10.0), price_row(100.0, 50.0, 10.0)],
        );
        let markers = detect_absorption(&bars);
        assert!(markers.is_empty(), "trending bar flagged: {markers:?}");
    }

    #[test]
    fn rowless_bars_never_fabricate_analytics() {
        // Bars carried by the snapshot without a printed ladder are not live
        // order flow and must not enter a signal.
        let rowless: Vec<FlowBar> = (1..=4)
            .map(|timestamp| FlowBar {
                timestamp,
                buy_volume: 50.0,
                sell_volume: 10.0,
                total_volume: 60.0,
                ..FlowBar::default()
            })
            .collect();
        let cvd: Vec<(i64, f64)> = (1..=4)
            .map(|timestamp| (timestamp, timestamp as f64))
            .collect();
        assert!(detect_absorption(&rowless).is_empty());
        assert!(detect_divergences(&rowless, &cvd, 100).is_empty());

        // A single rowless bar inside a real window is skipped rather than
        // becoming a zero-price swing.
        let mut bars = absorption_window();
        let mut rowless_bar = rowless[0].clone();
        rowless_bar.timestamp = 25;
        bars.insert(2, rowless_bar);
        let markers = detect_absorption(&bars);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].timestamp, 3);
    }

    #[test]
    fn absorption_needs_a_direction_and_ignores_zero_volume() {
        // Zero-delta absorbing bar: aggressor unknown, no marker.
        let mut bars = absorption_window();
        bars[2] = flat_bar(3, 100.0, 30.0, 30.0);
        assert!(detect_absorption(&bars).is_empty());

        // All-zero volumes: nothing can be in the top decile of nothing.
        let zeros: Vec<FlowBar> = (0..10).map(|ts| flat_bar(ts, 100.0, 0.0, 0.0)).collect();
        assert!(detect_absorption(&zeros).is_empty());
        // Single bar and empty input.
        assert!(detect_absorption(&[]).is_empty());
        assert!(detect_absorption(&[flat_bar(1, 100.0, 9.0, 1.0)]).is_empty());
    }

    // --- Divergence -------------------------------------------------------

    #[test]
    fn price_higher_high_with_cvd_lower_high_is_bearish_once() {
        // Prices: 10, 12, 11, 15, 13, 20, 18, 16 -> swings highs at 12 and 15.
        // CVD deliberately weaker at the second high.
        let prices = [10.0, 12.0, 11.0, 15.0, 13.0, 20.0, 18.0, 16.0];
        let cvd_values = [0.0, 5.0, 4.0, 3.0, 2.0, 6.0, 5.0, 4.0];
        let bars: Vec<FlowBar> = prices
            .iter()
            .enumerate()
            .map(|(index, price)| flat_bar(index as i64 + 1, *price, 1.0, 0.0))
            .collect();
        let cvd: Vec<(i64, f64)> = cvd_values
            .iter()
            .enumerate()
            .map(|(index, value)| (index as i64 + 1, *value))
            .collect();

        let divergences = detect_divergences(&bars, &cvd, 100);
        let bearish: Vec<_> = divergences
            .iter()
            .filter(|d| d.kind == DivergenceKind::Bearish)
            .collect();
        assert_eq!(bearish.len(), 1, "divergences: {divergences:?}");
        assert_eq!(bearish[0].first_timestamp, 2);
        assert_eq!(bearish[0].second_timestamp, 4);
        assert!(approx(bearish[0].first_price, 12.0));
        assert!(approx(bearish[0].second_price, 15.0));
        assert!(approx(bearish[0].first_delta, 5.0));
        assert!(approx(bearish[0].second_delta, 3.0));
    }

    #[test]
    fn monotonic_price_and_delta_series_has_no_divergence() {
        let bars: Vec<FlowBar> = (1..=10)
            .map(|ts| flat_bar(ts, 100.0 + ts as f64, 2.0, 0.0))
            .collect();
        let cvd: Vec<(i64, f64)> = (1..=10).map(|ts| (ts, ts as f64 * 2.0)).collect();
        assert!(detect_divergences(&bars, &cvd, 100).is_empty());

        // Also check the degenerate inputs.
        assert!(detect_divergences(&[], &[], 100).is_empty());
        assert!(detect_divergences(&bars[..1], &cvd[..1], 100).is_empty());
    }

    #[test]
    fn divergence_respects_the_lookback_window() {
        // Two higher highs; the first pair sits outside a short lookback.
        let prices = [10.0, 12.0, 11.0, 15.0, 13.0, 20.0, 18.0, 16.0];
        let cvd_values = [0.0, 5.0, 4.0, 3.0, 2.0, 6.0, 5.0, 4.0];
        let bars: Vec<FlowBar> = prices
            .iter()
            .enumerate()
            .map(|(index, price)| flat_bar(index as i64 + 1, *price, 1.0, 0.0))
            .collect();
        let cvd: Vec<(i64, f64)> = cvd_values
            .iter()
            .enumerate()
            .map(|(index, value)| (index as i64 + 1, *value))
            .collect();
        assert!(detect_divergences(&bars, &cvd, 3).is_empty());
    }

    // --- Stacked imbalance ------------------------------------------------

    #[test]
    fn three_consecutive_imbalanced_rows_form_one_stack() {
        let bar = footprint_bar(
            7,
            vec![
                footprint_row(0, 0),
                footprint_row(1, 1),
                footprint_row(2, 1),
                footprint_row(3, 1),
                footprint_row(4, 0),
            ],
        );
        let stacks = detect_stacked_imbalances(&bar, 3);
        assert_eq!(stacks.len(), 1);
        let stack = &stacks[0];
        assert_eq!(stack.side, ImbalanceSide::Buy);
        assert_eq!((stack.row_start, stack.row_end), (1, 3));
        assert!(approx(stack.price_low, 101.0));
        assert!(approx(stack.price_high, 104.0));
        assert_eq!(stack.timestamp, 7);
        assert!(stack.volume.is_finite() && stack.delta.is_finite());
    }

    #[test]
    fn two_imbalanced_rows_are_not_a_stack_at_threshold_three() {
        let bar = footprint_bar(
            8,
            vec![
                footprint_row(0, -1),
                footprint_row(1, -1),
                footprint_row(2, 0),
            ],
        );
        assert!(detect_stacked_imbalances(&bar, 3).is_empty());
        // ...but they are at threshold two.
        assert_eq!(detect_stacked_imbalances(&bar, 2).len(), 1);
    }

    #[test]
    fn opposing_runs_split_into_stacks() {
        let bar = footprint_bar(
            9,
            vec![
                footprint_row(0, 1),
                footprint_row(1, 1),
                footprint_row(2, -1),
                footprint_row(3, -1),
            ],
        );
        let stacks = detect_stacked_imbalances(&bar, 2);
        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].side, ImbalanceSide::Buy);
        assert_eq!(stacks[1].side, ImbalanceSide::Sell);
        assert!(detect_stacked_imbalances(&footprint_bar(10, vec![]), 3).is_empty());
    }

    // --- CVD regime -------------------------------------------------------

    #[test]
    fn cvd_slope_classifies_rising_falling_and_flat() {
        let rising: Vec<(i64, f64)> = (1..=20).map(|ts| (ts, ts as f64 * 3.0)).collect();
        assert_eq!(
            build_cvd_series(&rising, 10, 0.05).regime,
            CvdRegime::Rising
        );

        let falling: Vec<(i64, f64)> = (1..=20).map(|ts| (ts, 1000.0 - ts as f64 * 3.0)).collect();
        assert_eq!(
            build_cvd_series(&falling, 10, 0.05).regime,
            CvdRegime::Falling
        );

        let flat: Vec<(i64, f64)> = (1..=20).map(|ts| (ts, 500.0)).collect();
        let series = build_cvd_series(&flat, 10, 0.05);
        assert_eq!(series.regime, CvdRegime::Flat);
        assert!(approx(series.slope, 0.0));

        // Degenerate inputs stay flat and finite.
        assert_eq!(build_cvd_series(&[], 10, 0.05), CvdSeries::default());
        let single = build_cvd_series(&[(1, 5.0)], 10, 0.05);
        assert_eq!(single.regime, CvdRegime::Flat);
        let zeros: Vec<(i64, f64)> = (1..=4).map(|ts| (ts, 0.0)).collect();
        let series = build_cvd_series(&zeros, 4, 0.05);
        assert!(series.slope.is_finite());
        assert_eq!(series.regime, CvdRegime::Flat);
    }

    #[test]
    fn cvd_ignores_non_finite_points() {
        let cvd = [
            (1, 1.0),
            (2, f64::NAN),
            (3, f64::INFINITY),
            (4, 2.0),
            (5, 3.0),
        ];
        let series = build_cvd_series(&cvd, 5, 0.05);
        assert_eq!(series.points.len(), 3);
        assert!(series.slope.is_finite());
    }

    // --- Property / fuzz-ish ---------------------------------------------

    /// Deterministic pseudo-random generator so the property test is invariant
    /// across platforms and repeats. xorshift is plenty for volume noise.
    fn next_volume(state: &mut u64) -> f64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state % 1_000) as f64 / 7.0
    }

    #[test]
    fn every_numeric_field_stays_finite_on_noisy_input() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for trial in 0..50u32 {
            let mut bars = Vec::new();
            let mut cvd = Vec::new();
            let mut running = 0.0;
            for index in 0..(trial % 12 + 1) as usize {
                // Prices sometimes collapse to one level, sometimes are holes.
                let price = 100.0 + ((index * (trial as usize % 5 + 1)) % 4) as f64;
                let buy = next_volume(&mut seed);
                let sell = next_volume(&mut seed);
                // Occasionally an all-zero or NaN row to exercise guards.
                let buy = if index % 7 == 6 { 0.0 } else { buy };
                let sell = if index % 11 == 10 { f64::NAN } else { sell };
                running += buy - sell.max(0.0);
                bars.push(flow_bar(
                    index as i64 + 1,
                    vec![price_row(price, buy, sell)],
                ));
                cvd.push((index as i64 + 1, running));
            }

            let profile = build_visible_profile(&bars, 0.5, DEFAULT_VALUE_AREA_PCT);
            for row in &profile.rows {
                for value in [
                    row.price,
                    row.price_low,
                    row.price_high,
                    row.buy_volume,
                    row.sell_volume,
                    row.total,
                ] {
                    assert!(value.is_finite(), "profile row leak: {row:?}");
                }
            }
            for value in [
                profile.total_volume,
                profile.total_buy,
                profile.total_sell,
                profile.value_area_coverage,
            ] {
                assert!(value.is_finite(), "profile total leak: {value}");
            }
            for value in [profile.poc, profile.value_area_low, profile.value_area_high]
                .into_iter()
                .flatten()
            {
                assert!(value.is_finite(), "profile level leak: {value}");
            }

            for marker in detect_absorption(&bars) {
                for value in [
                    marker.price,
                    marker.strength,
                    marker.delta,
                    marker.volume,
                    marker.range,
                ] {
                    assert!(value.is_finite(), "absorption leak: {marker:?}");
                }
                assert!((0.0..=1.0).contains(&marker.strength));
            }

            for divergence in detect_divergences(&bars, &cvd, 8) {
                for value in [
                    divergence.first_price,
                    divergence.first_delta,
                    divergence.second_price,
                    divergence.second_delta,
                ] {
                    assert!(value.is_finite(), "divergence leak: {divergence:?}");
                }
            }

            let series = build_cvd_series(&cvd, 8, 0.05);
            assert!(series.slope.is_finite(), "cvd slope leak");

            let bar = footprint_bar(index_to_ts(trial), stacked_rows(trial));
            for stack in detect_stacked_imbalances(&bar, 2) {
                for value in [stack.price_low, stack.price_high, stack.delta, stack.volume] {
                    assert!(value.is_finite(), "stack leak: {stack:?}");
                }
            }
        }
    }

    fn index_to_ts(trial: u32) -> i64 {
        i64::from(trial) + 1
    }

    /// Alternating imbalance flags derived from the trial index, so different
    /// runs exercise different run lengths and boundaries.
    fn stacked_rows(trial: u32) -> Vec<FootprintRowView> {
        (0..(trial as usize % 6))
            .map(|index| {
                footprint_row(
                    index,
                    if (index + trial as usize) % 3 == 0 {
                        1
                    } else {
                        -1
                    },
                )
            })
            .collect()
    }
}
