//! Pure footprint grid construction.
//!
//! The chart panel owns pixels and GPUI types; this module owns the
//! price-row bucketing, POC/value-area selection, and imbalance classification.
//! Keeping it pure makes the footprint behaviour unit-testable.
//!
//! ## Geometry
//!
//! Rows are anchored to absolute price buckets (`floor(price / step)`), never to
//! the visible `price_min`. Panning or zooming the viewport therefore cannot move
//! a trade into a different row. The row size comes from the backend
//! `FlowBar::row_size` first, then the symbol tick, then the observed source-row
//! gaps, and only then a viewport-derived fallback. `row_multiplier` merges that
//! source row size by whole multiples; merged rows are accumulated by price
//! overlap rather than by a fixed anchor.

use std::collections::{BTreeMap, HashSet};

use crate::data::orderflow_feed::{FlowBar, PriceFlow};

/// Default share of volume the value area must cover.
pub const DEFAULT_VALUE_AREA_PCT: f64 = 0.70;

/// Hard cap on how many target buckets a single source row may fan out into.
/// A corrupt row must never turn into a span-sized allocation.
const MAX_SPAN_BUCKETS: i64 = 4_096;

/// One rendered footprint row (a price bucket within one candle).
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintRowView {
    pub price_low: f64,
    pub price_high: f64,
    pub buy: f64,
    pub sell: f64,
    pub buy_count: u32,
    pub sell_count: u32,
    pub is_poc: bool,
    pub is_vah: bool,
    pub is_val: bool,
    /// `1` = ask-side / aggressive-buy imbalance (bullish, ask wins ties),
    /// `-1` = bid-side / aggressive-sell imbalance (bearish), `0` = none.
    pub imbalance: i8,
}

impl FootprintRowView {
    pub fn delta(&self) -> f64 {
        self.buy - self.sell
    }

    pub fn volume(&self) -> f64 {
        self.buy + self.sell
    }
}

/// One candle column in the footprint.
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintBarView {
    pub timestamp: i64,
    pub rows: Vec<FootprintRowView>,
    pub delta: f64,
    pub total_volume: f64,
    pub poc: Option<f64>,
    pub vah: Option<f64>,
    pub val: Option<f64>,
    pub is_closed: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FootprintGridView {
    pub bars: Vec<FootprintBarView>,
    /// Lowest emitted row boundary. Derived from the rows, not the viewport, so
    /// changing `price_min` does not change the result.
    pub price_min: f64,
    /// Highest emitted row boundary.
    pub price_max: f64,
    /// Effective row height in price units (source row size × multiplier).
    pub step: f64,
    /// Largest number of emitted rows in one bar.
    pub rows: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct Bucket {
    buy: f64,
    sell: f64,
    buy_count: u32,
    sell_count: u32,
}

impl Bucket {
    fn volume(&self) -> f64 {
        self.buy + self.sell
    }

    fn add(&mut self, buy: f64, sell: f64, buy_count: u32, sell_count: u32) {
        self.buy += buy;
        self.sell += sell;
        self.buy_count = self.buy_count.saturating_add(buy_count);
        self.sell_count = self.sell_count.saturating_add(sell_count);
    }
}

pub struct FootprintBuildInput<'a> {
    pub bars: &'a [FlowBar],
    pub timestamps: &'a HashSet<i64>,
    pub price_min: f64,
    pub price_max: f64,
    pub max_rows: usize,
    pub tick_size: Option<f64>,
    /// Merge the source row size into this multiple (1/2/4/8).
    pub row_multiplier: u32,
    pub show_value_area: bool,
    pub show_imbalance: bool,
    pub imbalance_ratio: f64,
}

/// Normalises a persisted multiplier to one of the supported steps.
pub fn normalize_row_multiplier(multiplier: u32) -> u32 {
    match multiplier {
        0 | 1 => 1,
        2 => 2,
        3 | 4 => 4,
        _ => 8,
    }
}

/// Absolute bucket index for a price and row size.
///
/// Prices that sit within rounding noise of a bucket boundary are snapped to
/// the boundary before flooring. Without that, a price like `100.3` divided by
/// `0.1` can land at `1002.999…` and move one row below its real bucket.
pub fn bucket_index(price: f64, step: f64) -> i64 {
    if !price.is_finite() || !step.is_finite() || step <= 0.0 {
        return 0;
    }
    let ratio = price / step;
    if !ratio.is_finite() {
        return 0;
    }
    let nearest = ratio.round();
    // Snap only values that are within rounding noise of a bucket boundary.
    // The cap keeps an extreme price/tick ratio from swallowing real fractions.
    let tolerance = (ratio.abs() * 1e-12).clamp(1e-9, 1e-3);
    let value = if (ratio - nearest).abs() <= tolerance {
        nearest
    } else {
        ratio
    };
    value.floor().clamp(i64::MIN as f64, i64::MAX as f64) as i64
}

/// Effective row size, in priority order:
///
/// 1. the backend/source `FlowBar::row_size` (median positive across the bars),
/// 2. the symbol's tick size,
/// 3. the smallest positive gap between observed source rows,
/// 4. the viewport span divided by `max_rows`.
///
/// The chosen source size is multiplied by the normalised row multiplier.
pub fn effective_row_size(
    bars: &[FlowBar],
    tick_size: Option<f64>,
    price_min: f64,
    price_max: f64,
    max_rows: usize,
    row_multiplier: u32,
) -> Option<f64> {
    let multiplier = normalize_row_multiplier(row_multiplier) as f64;
    let source = source_row_size(bars)
        .or_else(|| tick_size.filter(|tick| tick.is_finite() && *tick > 0.0))
        .or_else(|| infer_source_row_size(bars));

    let step = match source {
        Some(source) => source * multiplier,
        None => {
            let span = price_max - price_min;
            if max_rows == 0 || !span.is_finite() || span <= 0.0 {
                return None;
            }
            span / max_rows as f64
        }
    };
    (step.is_finite() && step > 0.0).then_some(step)
}

/// Median positive backend row size reported by the visible bars.
///
/// The median keeps one malformed event from collapsing the whole ladder to a
/// near-zero row size; a genuine mix usually means a backend change mid-session,
/// where the median still describes the current resolution.
pub fn source_row_size(bars: &[FlowBar]) -> Option<f64> {
    let mut sizes: Vec<f64> = bars
        .iter()
        .filter(|bar| !bar.price_rows.is_empty())
        .map(|bar| bar.row_size)
        .filter(|size| size.is_finite() && *size > 0.0)
        .collect();
    if sizes.is_empty() {
        return None;
    }
    sizes.sort_by(f64::total_cmp);
    sizes.get((sizes.len() - 1) / 2).copied()
}

/// Smallest positive gap between observed source rows. Only used when neither
/// the backend row size nor the tick size is available.
pub fn infer_source_row_size(bars: &[FlowBar]) -> Option<f64> {
    let mut prices = Vec::new();
    for bar in bars {
        for row in &bar.price_rows {
            let low = if row.price_low.is_finite() {
                row.price_low
            } else if row.price.is_finite() {
                row.price
            } else {
                continue;
            };
            prices.push(low);
        }
    }
    if prices.len() < 2 {
        return None;
    }
    prices.sort_by(f64::total_cmp);
    let mut best: Option<f64> = None;
    for pair in prices.windows(2) {
        let gap = pair[1] - pair[0];
        if gap.is_finite() && gap > 1e-12 {
            best = Some(best.map_or(gap, |current| current.min(gap)));
        }
    }
    best
}

pub fn build_footprint_grid(input: &FootprintBuildInput<'_>) -> FootprintGridView {
    if input.bars.is_empty() || input.max_rows == 0 {
        return FootprintGridView::default();
    }

    let multiplier = normalize_row_multiplier(input.row_multiplier);
    let Some(step) = effective_row_size(
        input.bars,
        input.tick_size,
        input.price_min,
        input.price_max,
        input.max_rows,
        multiplier,
    ) else {
        return FootprintGridView::default();
    };
    if !step.is_finite() || step <= 0.0 {
        return FootprintGridView::default();
    }

    let trust_backend = multiplier == 1;
    let mut bars = Vec::new();
    let mut grid_min = f64::INFINITY;
    let mut grid_max = f64::NEG_INFINITY;
    let mut grid_rows = 0usize;

    for bar in input.bars {
        if !input.timestamps.contains(&bar.timestamp) || bar.price_rows.is_empty() {
            continue;
        }

        let mut buckets: BTreeMap<i64, Bucket> = BTreeMap::new();
        for row in &bar.price_rows {
            add_flow_row(&mut buckets, row, step);
        }
        if buckets.is_empty() {
            continue;
        }

        let total_volume: f64 = buckets.values().map(Bucket::volume).sum();
        if total_volume <= 0.0 || !total_volume.is_finite() {
            continue;
        }

        let mut rows: Vec<FootprintRowView> = buckets
            .iter()
            .map(|(&index, bucket)| FootprintRowView {
                price_low: index as f64 * step,
                price_high: index.saturating_add(1) as f64 * step,
                buy: bucket.buy,
                sell: bucket.sell,
                buy_count: bucket.buy_count,
                sell_count: bucket.sell_count,
                is_poc: false,
                is_vah: false,
                is_val: false,
                imbalance: 0,
            })
            .collect();

        let (computed_low, computed_high, _) = value_area_bounds(&rows);
        let computed_poc = poc_index(&rows);

        let mut poc_price = None;
        let mut vah_price = None;
        let mut val_price = None;
        let mut marked_poc = false;
        let mut marked_va = false;

        if trust_backend
            && let Some(price) = bar.poc.filter(|price| price.is_finite())
            && mark_poc(&mut rows, price)
        {
            poc_price = Some(price);
            marked_poc = true;
        }
        if input.show_value_area
            && trust_backend
            && let (Some(vah), Some(val)) = (
                bar.vah.filter(|price| price.is_finite()),
                bar.val.filter(|price| price.is_finite()),
            )
            && mark_vah(&mut rows, vah)
            && mark_val(&mut rows, val)
        {
            vah_price = Some(vah);
            val_price = Some(val);
            marked_va = true;
        }

        if !marked_poc {
            let index = computed_poc.min(rows.len().saturating_sub(1));
            rows[index].is_poc = true;
            poc_price = Some((rows[index].price_low + rows[index].price_high) / 2.0);
        }
        if input.show_value_area && !marked_va {
            rows[computed_low].is_val = true;
            rows[computed_high].is_vah = true;
            val_price = Some(rows[computed_low].price_low);
            vah_price = Some(rows[computed_high].price_high);
        }

        if input.show_imbalance {
            classify_imbalances(&mut rows, step, input.imbalance_ratio);
        }

        let total_buy: f64 = rows.iter().map(|row| row.buy).sum();
        let total_sell: f64 = rows.iter().map(|row| row.sell).sum();
        let delta = total_buy - total_sell;

        for row in &rows {
            if row.price_low.is_finite() {
                grid_min = grid_min.min(row.price_low);
            }
            if row.price_high.is_finite() {
                grid_max = grid_max.max(row.price_high);
            }
        }
        grid_rows = grid_rows.max(rows.len());

        bars.push(FootprintBarView {
            timestamp: bar.timestamp,
            rows,
            delta,
            total_volume: total_buy + total_sell,
            poc: poc_price,
            vah: vah_price,
            val: val_price,
            is_closed: bar.is_closed,
        });
    }

    if bars.is_empty() {
        return FootprintGridView::default();
    }
    if !grid_min.is_finite() || !grid_max.is_finite() {
        return FootprintGridView::default();
    }

    FootprintGridView {
        bars,
        price_min: grid_min,
        price_max: grid_max,
        step,
        rows: grid_rows,
    }
}

fn add_flow_row(buckets: &mut BTreeMap<i64, Bucket>, row: &PriceFlow, step: f64) {
    let buy = sanitize_volume(row.buy_volume);
    let sell = sanitize_volume(row.sell_volume);
    if buy + sell <= 0.0 {
        return;
    }

    let mut low = row.price_low;
    let mut high = row.price_high;
    if !low.is_finite() {
        low = row.price;
    }
    if !high.is_finite() {
        high = row.price;
    }
    if !low.is_finite() || !high.is_finite() {
        return;
    }
    if high < low {
        std::mem::swap(&mut low, &mut high);
    }

    let span = high - low;
    if span <= 0.0 || span <= step * 1e-9 {
        let price = if row.price.is_finite() {
            row.price
        } else {
            (low + high) / 2.0
        };
        let bucket = buckets.entry(bucket_index(price, step)).or_default();
        bucket.add(buy, sell, row.buy_count, row.sell_count);
        return;
    }

    let first = bucket_index(low, step);
    let last = bucket_index(high, step);
    if last.saturating_sub(first) > MAX_SPAN_BUCKETS {
        // Pathological span: keep the volume at the row midpoint rather than
        // fanning out into an unbounded number of buckets.
        let bucket = buckets
            .entry(bucket_index((low + high) / 2.0, step))
            .or_default();
        bucket.add(buy, sell, row.buy_count, row.sell_count);
        return;
    }

    for index in first..=last {
        let bucket_low = index as f64 * step;
        let bucket_high = bucket_low + step;
        let overlap = high.min(bucket_high) - low.max(bucket_low);
        if overlap <= 0.0 {
            continue;
        }
        let fraction = (overlap / span).clamp(0.0, 1.0);
        let bucket = buckets.entry(index).or_default();
        bucket.add(
            buy * fraction,
            sell * fraction,
            scale_count(row.buy_count, fraction),
            scale_count(row.sell_count, fraction),
        );
    }
}

fn scale_count(count: u32, fraction: f64) -> u32 {
    if count == 0 {
        return 0;
    }
    ((count as f64 * fraction).round() as u32).max(1)
}

fn sanitize_volume(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// Earliest row holding the highest volume (deterministic on ties).
fn poc_index(rows: &[FootprintRowView]) -> usize {
    let mut best = 0usize;
    for (index, row) in rows.iter().enumerate() {
        if row.volume() > rows[best].volume() {
            best = index;
        }
    }
    best
}

/// Contiguous value-area expansion from the POC.
///
/// At each step compare the row immediately above and below the current area and
/// take the larger (upward on ties). Expansion stops at the target coverage or
/// when both neighbours are exhausted. Returns `(low_index, high_index,
/// coverage)` into `rows` (sorted ascending by price).
fn value_area_bounds(rows: &[FootprintRowView]) -> (usize, usize, f64) {
    if rows.is_empty() {
        return (0, 0, 0.0);
    }
    let total: f64 = rows.iter().map(FootprintRowView::volume).sum();
    if total <= 0.0 || !total.is_finite() {
        return (0, 0, 0.0);
    }

    let poc = poc_index(rows);
    let target = total * DEFAULT_VALUE_AREA_PCT;
    let mut low = poc;
    let mut high = poc;
    let mut accumulated = rows[poc].volume();

    // Each row can be visited at most once in either direction.
    for _ in 0..rows.len().saturating_mul(2).max(2) {
        if accumulated >= target {
            break;
        }
        let up = rows.get(high + 1).map(FootprintRowView::volume);
        let down = low
            .checked_sub(1)
            .and_then(|index| rows.get(index))
            .map(FootprintRowView::volume);
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

    let coverage = (accumulated / total).clamp(0.0, 1.0);
    (low, high, coverage)
}

/// Marks the row whose price range contains `price`, or the nearest midpoint.
fn mark_poc(rows: &mut [FootprintRowView], price: f64) -> bool {
    let Some(index) = row_containing_price(rows, price)
        .or_else(|| nearest_row(rows, price, |row| (row.price_low, row.price_high)))
    else {
        return false;
    };
    rows[index].is_poc = true;
    true
}

/// Half-open bucket containment: a price exactly on an interior boundary
/// belongs to the bucket above it. The top bucket includes its upper edge.
fn row_containing_price(rows: &[FootprintRowView], price: f64) -> Option<usize> {
    let last = rows.len().checked_sub(1)?;
    rows.iter().enumerate().find_map(|(index, row)| {
        let contained = price >= row.price_low
            && (price < row.price_high || (index == last && price <= row.price_high));
        contained.then_some(index)
    })
}

fn mark_vah(rows: &mut [FootprintRowView], price: f64) -> bool {
    let Some((index, _)) = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (index, (row.price_high - price).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return false;
    };
    rows[index].is_vah = true;
    true
}

fn mark_val(rows: &mut [FootprintRowView], price: f64) -> bool {
    let Some((index, _)) = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (index, (row.price_low - price).abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return false;
    };
    rows[index].is_val = true;
    true
}

fn nearest_row(
    rows: &[FootprintRowView],
    price: f64,
    bounds: impl Fn(&FootprintRowView) -> (f64, f64),
) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let (low, high) = bounds(row);
            let distance = if price >= low && price <= high {
                0.0
            } else if price < low {
                low - price
            } else {
                price - high
            };
            (index, distance)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// Standard footprint diagonal imbalance.
///
/// * Ask imbalance at row `i`: aggressive buy at `i` versus aggressive sell at
///   the row one tick below (`i - 1`), when that sell volume is positive.
/// * Bid imbalance at row `i`: aggressive sell at `i` versus aggressive buy at
///   the row one tick above (`i + 1`), when that buy volume is positive.
///
/// Both directions are evaluated independently; when both match, ask wins
/// because `1` is written first. Rows that are not exactly one tick apart are
/// not compared, so a sparse price ladder cannot create a false diagonal.
fn classify_imbalances(rows: &mut [FootprintRowView], step: f64, ratio: f64) {
    let ratio = if ratio.is_finite() {
        ratio.max(1.0)
    } else {
        1.0
    };
    let tolerance = step.max(1e-12) * 1e-6;
    for index in 0..rows.len() {
        let ask = index > 0
            && (rows[index].price_low - rows[index - 1].price_low - step).abs() <= tolerance
            && rows[index - 1].sell > 0.0
            && rows[index].buy >= ratio * rows[index - 1].sell;
        if ask {
            rows[index].imbalance = 1;
            continue;
        }
        let bid = index + 1 < rows.len()
            && (rows[index + 1].price_low - rows[index].price_low - step).abs() <= tolerance
            && rows[index + 1].buy > 0.0
            && rows[index].sell >= ratio * rows[index + 1].buy;
        if bid {
            rows[index].imbalance = -1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(price: f64, buy: f64, sell: f64) -> PriceFlow {
        PriceFlow {
            price,
            price_low: price,
            price_high: price,
            buy_volume: buy,
            sell_volume: sell,
            buy_count: if buy > 0.0 { 1 } else { 0 },
            sell_count: if sell > 0.0 { 1 } else { 0 },
        }
    }

    fn span_row(low: f64, high: f64, buy: f64, sell: f64) -> PriceFlow {
        PriceFlow {
            price: (low + high) / 2.0,
            price_low: low,
            price_high: high,
            buy_volume: buy,
            sell_volume: sell,
            buy_count: if buy > 0.0 { 1 } else { 0 },
            sell_count: if sell > 0.0 { 1 } else { 0 },
        }
    }

    fn bar(timestamp: i64, rows: Vec<PriceFlow>) -> FlowBar {
        let buy_volume = rows.iter().map(|row| row.buy_volume).sum();
        let sell_volume = rows.iter().map(|row| row.sell_volume).sum();
        FlowBar {
            timestamp,
            buy_volume,
            sell_volume,
            trades: rows.len() as u32,
            price_rows: rows,
            total_volume: buy_volume + sell_volume,
            is_closed: false,
            ..FlowBar::default()
        }
    }

    fn bar_with_size(timestamp: i64, row_size: f64, rows: Vec<PriceFlow>) -> FlowBar {
        FlowBar {
            row_size,
            ..bar(timestamp, rows)
        }
    }

    fn timestamps(values: &[i64]) -> HashSet<i64> {
        values.iter().copied().collect()
    }

    fn build(
        bars: &[FlowBar],
        price_min: f64,
        price_max: f64,
        tick_size: Option<f64>,
        row_multiplier: u32,
    ) -> FootprintGridView {
        build_footprint_grid(&FootprintBuildInput {
            bars,
            timestamps: &timestamps(&bars.iter().map(|bar| bar.timestamp).collect::<Vec<_>>()),
            price_min,
            price_max,
            max_rows: 8,
            tick_size,
            row_multiplier,
            show_value_area: true,
            show_imbalance: true,
            imbalance_ratio: 3.0,
        })
    }

    #[test]
    fn buckets_rows_and_marks_poc_and_value_area() {
        let bars = vec![bar(
            1,
            vec![
                row(100.0, 2.0, 1.0),
                row(100.5, 0.5, 0.0),
                row(101.0, 1.0, 0.0),
            ],
        )];
        let view = build_footprint_grid(&FootprintBuildInput {
            bars: &bars,
            timestamps: &timestamps(&[1]),
            price_min: 100.0,
            price_max: 102.0,
            max_rows: 4,
            tick_size: Some(1.0),
            row_multiplier: 1,
            show_value_area: true,
            show_imbalance: true,
            imbalance_ratio: 3.0,
        });

        assert_eq!(view.bars.len(), 1);
        let bar = &view.bars[0];
        assert_eq!(bar.rows.len(), 2);
        assert_eq!(bar.rows[0].volume(), 3.5);
        assert!(bar.rows[0].is_poc);
        assert_eq!(bar.poc, Some(100.5));
        assert_eq!(bar.vah, Some(101.0));
        assert_eq!(bar.val, Some(100.0));
        assert!((bar.delta - 2.5).abs() < 1e-9);
        assert!((bar.total_volume - 4.5).abs() < 1e-9);
    }

    #[test]
    fn geometry_is_independent_of_the_viewport_price_min() {
        let bars = vec![bar_with_size(
            1,
            0.5,
            vec![row(100.0, 2.0, 1.0), row(100.5, 0.0, 2.0)],
        )];
        let low = build(&bars, 90.0, 110.0, None, 1);
        let high = build(&bars, 99.0, 104.0, None, 1);
        assert_eq!(low.bars, high.bars);
        assert_eq!(low.step, high.step);
        assert_eq!(low.price_min, high.price_min);
        assert_eq!(low.price_max, high.price_max);
        assert_eq!(low.step, 0.5);
    }

    #[test]
    fn backend_row_size_is_respected() {
        let bars = vec![bar_with_size(
            1,
            0.5,
            vec![row(100.0, 1.0, 0.0), row(100.5, 2.0, 0.0)],
        )];
        let view = build(&bars, 90.0, 110.0, Some(5.0), 1);
        assert_eq!(view.step, 0.5);
        assert_eq!(view.bars[0].rows.len(), 2);
        assert_eq!(view.bars[0].rows[0].price_low, 100.0);
        assert_eq!(view.bars[0].rows[1].price_low, 100.5);
    }

    #[test]
    fn a_single_outlier_row_size_does_not_collapse_the_ladder() {
        let bars = vec![
            bar_with_size(1, 1.0, vec![span_row(100.0, 101.0, 1.0, 0.0)]),
            bar_with_size(2, 1.0, vec![span_row(100.0, 101.0, 1.0, 0.0)]),
            bar_with_size(3, 0.0001, vec![row(100.0, 1.0, 0.0)]),
        ];
        let view = build(&bars, 90.0, 110.0, None, 1);
        assert!((view.step - 1.0).abs() < 1e-12);
    }

    #[test]
    fn row_multiplier_merges_source_rows() {
        let bars = vec![bar_with_size(
            1,
            1.0,
            vec![
                span_row(100.0, 101.0, 2.0, 1.0),
                span_row(101.0, 102.0, 3.0, 4.0),
                span_row(102.0, 103.0, 5.0, 0.5),
            ],
        )];
        let view = build(&bars, 90.0, 110.0, None, 2);
        assert_eq!(view.step, 2.0);
        assert_eq!(view.bars[0].rows.len(), 2);
        assert!((view.bars[0].rows[0].buy - 5.0).abs() < 1e-9);
        assert!((view.bars[0].rows[0].sell - 5.0).abs() < 1e-9);
        assert!((view.bars[0].rows[1].buy - 5.0).abs() < 1e-9);
        assert!((view.bars[0].rows[1].sell - 0.5).abs() < 1e-9);
    }

    #[test]
    fn volume_is_split_by_price_overlap() {
        let bars = vec![bar_with_size(
            1,
            1.0,
            vec![span_row(100.0, 102.0, 10.0, 4.0)],
        )];
        let view = build(&bars, 90.0, 110.0, None, 1);
        assert_eq!(view.step, 1.0);
        assert_eq!(view.bars[0].rows.len(), 2);
        assert!((view.bars[0].rows[0].buy - 5.0).abs() < 1e-9);
        assert!((view.bars[0].rows[0].sell - 2.0).abs() < 1e-9);
        assert!((view.bars[0].rows[1].buy - 5.0).abs() < 1e-9);
        assert!((view.bars[0].rows[1].sell - 2.0).abs() < 1e-9);
    }

    #[test]
    fn backend_poc_and_value_area_are_preserved_at_one_x() {
        let mut flow = bar_with_size(
            1,
            1.0,
            vec![
                row(100.0, 1.0, 0.0),
                row(101.0, 5.0, 0.0),
                row(102.0, 1.0, 0.0),
            ],
        );
        flow.poc = Some(101.0);
        flow.vah = Some(103.0);
        flow.val = Some(100.0);
        let bars = vec![flow];
        let view = build(&bars, 90.0, 110.0, None, 1);
        let bar = &view.bars[0];
        assert_eq!(bar.poc, Some(101.0));
        assert_eq!(bar.vah, Some(103.0));
        assert_eq!(bar.val, Some(100.0));
        assert!(bar.rows[1].is_poc);
        assert!(bar.rows[2].is_vah);
        assert!(bar.rows[0].is_val);
    }

    #[test]
    fn value_area_can_be_disabled() {
        let bars = vec![bar(
            1,
            vec![
                row(100.0, 2.0, 1.0),
                row(100.5, 0.5, 0.0),
                row(101.0, 1.0, 0.0),
            ],
        )];
        let view = build_footprint_grid(&FootprintBuildInput {
            bars: &bars,
            timestamps: &timestamps(&[1]),
            price_min: 100.0,
            price_max: 102.0,
            max_rows: 4,
            tick_size: Some(0.5),
            row_multiplier: 1,
            show_value_area: false,
            show_imbalance: false,
            imbalance_ratio: 3.0,
        });
        let bar = &view.bars[0];
        assert!(bar.rows.iter().all(|row| !row.is_vah && !row.is_val));
        assert_eq!(bar.vah, None);
        assert_eq!(bar.val, None);
    }

    #[test]
    fn value_area_expands_contiguously_and_covers_target() {
        let rows = vec![
            FootprintRowView {
                price_low: 100.0,
                price_high: 101.0,
                buy: 5.0,
                sell: 0.0,
                buy_count: 1,
                sell_count: 0,
                is_poc: false,
                is_vah: false,
                is_val: false,
                imbalance: 0,
            },
            FootprintRowView {
                price_low: 101.0,
                price_high: 102.0,
                buy: 4.0,
                sell: 0.0,
                buy_count: 1,
                sell_count: 0,
                is_poc: false,
                is_vah: false,
                is_val: false,
                imbalance: 0,
            },
            FootprintRowView {
                price_low: 102.0,
                price_high: 103.0,
                buy: 3.0,
                sell: 0.0,
                buy_count: 1,
                sell_count: 0,
                is_poc: false,
                is_vah: false,
                is_val: false,
                imbalance: 0,
            },
        ];
        let (low, high, coverage) = value_area_bounds(&rows);
        assert_eq!((low, high), (0, 1));
        assert!(coverage >= DEFAULT_VALUE_AREA_PCT);
        assert_eq!(high - low + 1, 2);
    }

    #[test]
    fn ask_imbalance_compares_buy_to_the_row_below() {
        let bars = vec![bar(1, vec![row(100.0, 0.0, 1.0), row(101.0, 6.0, 0.0)])];
        let view = build(&bars, 100.0, 102.0, Some(1.0), 1);
        let rows = &view.bars[0].rows;
        assert_eq!(rows[0].imbalance, 0);
        assert_eq!(rows[1].imbalance, 1);
    }

    #[test]
    fn bid_imbalance_compares_sell_to_the_row_above() {
        let bars = vec![bar(1, vec![row(100.0, 0.0, 6.0), row(101.0, 1.0, 0.0)])];
        let view = build(&bars, 100.0, 102.0, Some(1.0), 1);
        let rows = &view.bars[0].rows;
        assert_eq!(rows[0].imbalance, -1);
        assert_eq!(rows[1].imbalance, 0);
    }

    #[test]
    fn imbalance_requires_a_positive_opposite_side_and_adjacent_rows() {
        // No sell volume below -> no ask imbalance, even though buy is large.
        let no_opposite = vec![bar(1, vec![row(100.0, 0.0, 0.0), row(101.0, 6.0, 0.0)])];
        let view = build(&no_opposite, 100.0, 103.0, Some(1.0), 1);
        assert!(view.bars[0].rows.iter().all(|row| row.imbalance == 0));

        // A missing tick between the rows must not create a diagonal.
        let sparse = vec![bar(1, vec![row(100.0, 0.0, 1.0), row(102.0, 6.0, 0.0)])];
        let view = build(&sparse, 100.0, 104.0, Some(1.0), 1);
        assert!(view.bars[0].rows.iter().all(|row| row.imbalance == 0));
    }

    #[test]
    fn sparse_rows_do_not_allocate_a_span_sized_ladder() {
        let bars = vec![bar_with_size(
            1,
            0.5,
            vec![row(100.0, 1.0, 0.0), row(100.5, 2.0, 0.0)],
        )];
        // A viewport spanning a billion points must still emit only the traded rows.
        let view = build(&bars, 100.0, 1_000_000_000.0, None, 1);
        assert_eq!(view.rows, 2);
        assert_eq!(view.bars[0].rows.len(), 2);
    }

    #[test]
    fn ignores_bars_outside_the_visible_timestamps() {
        let bars = vec![bar(1, vec![row(100.0, 1.0, 0.0)])];
        let view = build_footprint_grid(&FootprintBuildInput {
            bars: &bars,
            timestamps: &timestamps(&[2]),
            price_min: 100.0,
            price_max: 101.0,
            max_rows: 4,
            tick_size: None,
            row_multiplier: 1,
            show_value_area: true,
            show_imbalance: true,
            imbalance_ratio: 3.0,
        });
        assert!(view.bars.is_empty());
    }
}
