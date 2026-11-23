//! Pure footprint grid construction.
//!
//! The chart panel owns pixels and GPUI types; this module owns the
//! price-row bucketing, POC/value-area selection, and imbalance classification.
//! Keeping it pure makes the footprint behaviour unit-testable.

use std::collections::HashSet;

use crate::data::orderflow_feed::FlowBar;

const MAX_GRID_ROWS: usize = 120;

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
    /// `1` = bid imbalance, `-1` = ask imbalance, `0` = none.
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
    pub price_min: f64,
    pub price_max: f64,
    pub step: f64,
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
}

pub struct FootprintBuildInput<'a> {
    pub bars: &'a [FlowBar],
    pub timestamps: &'a HashSet<i64>,
    pub price_min: f64,
    pub price_max: f64,
    pub max_rows: usize,
    pub tick_size: Option<f64>,
    pub show_value_area: bool,
    pub show_imbalance: bool,
    pub imbalance_ratio: f64,
}

pub fn build_footprint_grid(input: &FootprintBuildInput<'_>) -> FootprintGridView {
    if !input.price_min.is_finite()
        || !input.price_max.is_finite()
        || input.price_max <= input.price_min
        || input.max_rows == 0
    {
        return FootprintGridView::default();
    }

    let span = input.price_max - input.price_min;
    let mut step = span / input.max_rows as f64;
    if let Some(tick) = input
        .tick_size
        .filter(|tick| *tick > 0.0 && tick.is_finite())
    {
        step = ((step / tick).ceil() * tick).max(tick);
    }
    if !step.is_finite() || step <= 0.0 {
        return FootprintGridView::default();
    }

    let rows = ((span / step).ceil() as usize).max(1).min(MAX_GRID_ROWS);
    let mut bars = Vec::new();

    for bar in input.bars {
        if !input.timestamps.contains(&bar.timestamp) || bar.price_rows.is_empty() {
            continue;
        }

        let mut buckets = vec![Bucket::default(); rows];
        for row in &bar.price_rows {
            if !row.price.is_finite() {
                continue;
            }
            let index = (((row.price - input.price_min) / step).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            let bucket = &mut buckets[index];
            bucket.buy += row.buy_volume;
            bucket.sell += row.sell_volume;
            bucket.buy_count += row.buy_count;
            bucket.sell_count += row.sell_count;
        }

        let total_volume: f64 = buckets.iter().map(Bucket::volume).sum();
        if total_volume <= 0.0 {
            continue;
        }

        let poc_index = buckets
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.volume().total_cmp(&b.1.volume()))
            .filter(|(_, bucket)| bucket.volume() > 0.0)
            .map(|(index, _)| index);

        let value_area = if input.show_value_area {
            value_area_indices(&buckets)
        } else {
            None
        };
        let imbalances = if input.show_imbalance {
            classify_imbalances(&buckets, input.imbalance_ratio)
        } else {
            vec![0; rows]
        };

        let mut out_rows = Vec::new();
        for (index, bucket) in buckets.iter().enumerate() {
            if bucket.volume() <= 0.0 {
                continue;
            }
            out_rows.push(FootprintRowView {
                price_low: input.price_min + index as f64 * step,
                price_high: input.price_min + (index + 1) as f64 * step,
                buy: bucket.buy,
                sell: bucket.sell,
                buy_count: bucket.buy_count,
                sell_count: bucket.sell_count,
                is_poc: poc_index == Some(index),
                is_vah: value_area.is_some_and(|(_, max)| max == index),
                is_val: value_area.is_some_and(|(min, _)| min == index),
                imbalance: imbalances[index],
            });
        }

        bars.push(FootprintBarView {
            timestamp: bar.timestamp,
            rows: out_rows,
            delta: bar.buy_volume - bar.sell_volume,
            total_volume: bar.total_volume.max(total_volume),
            poc: poc_index.map(|index| input.price_min + (index as f64 + 0.5) * step),
            vah: value_area.map(|(_, max)| input.price_min + (max + 1) as f64 * step),
            val: value_area.map(|(min, _)| input.price_min + min as f64 * step),
            is_closed: bar.is_closed,
        });
    }

    FootprintGridView {
        bars,
        price_min: input.price_min,
        price_max: input.price_max,
        step,
        rows,
    }
}

fn value_area_indices(buckets: &[Bucket]) -> Option<(usize, usize)> {
    let total: f64 = buckets.iter().map(Bucket::volume).sum();
    if total <= 0.0 {
        return None;
    }

    let mut order: Vec<(usize, f64)> = buckets
        .iter()
        .enumerate()
        .filter(|(_, bucket)| bucket.volume() > 0.0)
        .map(|(index, bucket)| (index, bucket.volume()))
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1));

    let target = total * 0.7;
    let mut accumulated = 0.0;
    let mut included = Vec::new();
    for (index, volume) in order {
        included.push(index);
        accumulated += volume;
        if accumulated >= target {
            break;
        }
    }

    let min = included.iter().copied().min()?;
    let max = included.iter().copied().max()?;
    Some((min, max))
}

fn classify_imbalances(buckets: &[Bucket], ratio: f64) -> Vec<i8> {
    let ratio = ratio.max(1.0);
    let mut out = vec![0i8; buckets.len()];
    for index in 0..buckets.len() {
        let bucket = &buckets[index];

        if bucket.buy > 0.0 {
            if let Some(next) = buckets.get(index + 1) {
                if next.sell > 0.0 && bucket.buy >= ratio * next.sell {
                    out[index] = 1;
                    continue;
                }
            }
        }

        if bucket.sell > 0.0 && index > 0 {
            let previous = &buckets[index - 1];
            if previous.buy > 0.0 && bucket.sell >= ratio * previous.buy {
                out[index] = -1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::orderflow_feed::PriceFlow;

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

    fn timestamps(values: &[i64]) -> HashSet<i64> {
        values.iter().copied().collect()
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
    }

    #[test]
    fn flags_diagonal_imbalance() {
        let bars = vec![bar(1, vec![row(100.0, 6.0, 0.0), row(101.0, 0.0, 1.0)])];
        let view = build_footprint_grid(&FootprintBuildInput {
            bars: &bars,
            timestamps: &timestamps(&[1]),
            price_min: 100.0,
            price_max: 102.0,
            max_rows: 2,
            tick_size: Some(1.0),
            show_value_area: false,
            show_imbalance: true,
            imbalance_ratio: 3.0,
        });

        assert_eq!(view.bars[0].rows[0].imbalance, 1);
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
            show_value_area: true,
            show_imbalance: true,
            imbalance_ratio: 3.0,
        });
        assert!(view.bars.is_empty());
    }
}
