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

use crate::chart::footprint::{DEFAULT_VALUE_AREA_PCT, bucket_index, infer_source_row_size};
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
