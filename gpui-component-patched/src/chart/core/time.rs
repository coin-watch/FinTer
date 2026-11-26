//! Time-axis tick selection.

use chrono::{DateTime, Months, Utc};

/// Nice wall-clock intervals used by the time axis, in milliseconds.
pub const TIME_INTERVALS_MS: [i64; 17] = [
    1_000,
    5_000,
    15_000,
    30_000,
    60_000,
    5 * 60_000,
    15 * 60_000,
    30 * 60_000,
    60 * 60_000,
    4 * 60 * 60_000,
    12 * 60 * 60_000,
    24 * 60 * 60_000,
    2 * 24 * 60 * 60_000,
    7 * 24 * 60 * 60_000,
    30 * 24 * 60 * 60_000,
    90 * 24 * 60 * 60_000,
    365 * 24 * 60 * 60_000,
];

/// Choose the smallest nice interval that yields at most `target_ticks` labels.
///
/// Returns the largest interval when even that would produce more labels than
/// requested (for example a multi-year view on a narrow pane).
pub fn choose_time_interval(span_ms: i64, target_ticks: usize) -> i64 {
    let target_ticks = target_ticks.max(1);
    let span_ms = span_ms.abs().max(1);
    let ideal = (span_ms as f64 / target_ticks as f64).max(1.0);
    TIME_INTERVALS_MS
        .iter()
        .copied()
        .find(|interval| *interval as f64 >= ideal)
        .unwrap_or_else(|| *TIME_INTERVALS_MS.last().unwrap())
}

/// Round a timestamp down to the nearest interval boundary.
pub fn align_down(timestamp_ms: i64, interval_ms: i64) -> i64 {
    if interval_ms <= 0 {
        return timestamp_ms;
    }
    timestamp_ms.div_euclid(interval_ms) * interval_ms
}

/// Round a timestamp up to the next interval boundary.
pub fn align_up(timestamp_ms: i64, interval_ms: i64) -> i64 {
    if interval_ms <= 0 {
        return timestamp_ms;
    }
    let down = align_down(timestamp_ms, interval_ms);
    if down == timestamp_ms {
        timestamp_ms
    } else {
        down + interval_ms
    }
}

/// Uniform duration of a chart timeframe in milliseconds.
///
/// Returns `None` for calendar timeframes (`1M`) and unknown values; callers
/// should fall back to a median interval or step calendar months explicitly.
pub fn timeframe_interval_ms(timeframe: &str) -> Option<i64> {
    Some(match timeframe {
        "1s" => 1_000,
        "1m" => 60_000,
        "3m" => 180_000,
        "5m" => 300_000,
        "15m" => 900_000,
        "30m" => 1_800_000,
        "1h" => 3_600_000,
        "2h" => 7_200_000,
        "4h" => 14_400_000,
        "6h" => 21_600_000,
        "8h" => 28_800_000,
        "12h" => 43_200_000,
        "1d" => 86_400_000,
        "1w" => 604_800_000,
        _ => return None,
    })
}

/// Resolve a timestamp for a possibly out-of-buffer candle slot.
///
/// * In-range indices return the real timestamp (real gaps are preserved).
/// * Indices before `0` extend backwards from `timestamps[0]`.
/// * Indices at or beyond `len` extend forwards from the last timestamp.
///
/// Uniform timeframes use their exact interval; `1M` steps calendar months so
/// blank monthly slots follow the exchange bucket boundaries. Unknown
/// timeframes fall back to `interval_ms`. All arithmetic saturates.
pub fn timestamp_at_buffer_index(
    timestamps: &[i64],
    index: f64,
    timeframe: &str,
    interval_ms: i64,
) -> Option<i64> {
    let len = timestamps.len();
    if len == 0 || !index.is_finite() {
        return None;
    }
    let rounded = index.round();
    if !rounded.is_finite() {
        return None;
    }
    let slot = rounded as i64;
    let len_i = len as i64;

    if slot >= 0 && slot < len_i {
        return timestamps.get(slot as usize).copied();
    }

    if timeframe == "1M" {
        let (base, months) = if slot < 0 {
            (*timestamps.first()?, slot)
        } else {
            (*timestamps.last()?, slot.saturating_sub(len_i - 1))
        };
        return shift_months(base, months);
    }

    let interval = timeframe_interval_ms(timeframe)
        .unwrap_or(interval_ms)
        .max(1);
    if slot < 0 {
        let steps = slot.checked_neg()?;
        let first = *timestamps.first()?;
        Some(first.saturating_sub(steps.saturating_mul(interval)))
    } else {
        let steps = slot.saturating_sub(len_i - 1);
        let last = *timestamps.last()?;
        Some(last.saturating_add(steps.saturating_mul(interval)))
    }
}

/// Shift a Unix-millisecond timestamp by calendar months.
///
/// `chrono` clamps to the last valid day of shorter months (for example
/// January 31 + 1 month becomes February 28/29), which matches how exchanges
/// bucket monthly candles.
fn shift_months(timestamp_ms: i64, months: i64) -> Option<i64> {
    if months == 0 {
        return Some(timestamp_ms);
    }
    let dt = DateTime::<Utc>::from_timestamp_millis(timestamp_ms)?;
    let amount = Months::new(u32::try_from(months.unsigned_abs()).ok()?);
    let shifted = if months > 0 {
        dt.checked_add_months(amount)?
    } else {
        dt.checked_sub_months(amount)?
    };
    Some(shifted.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    #[test]
    fn selects_minute_interval_for_short_spans() {
        let interval = choose_time_interval(10 * MINUTE, 5);
        assert_eq!(interval, 5 * MINUTE);
    }

    #[test]
    fn selects_hour_for_intraday_spans() {
        let interval = choose_time_interval(12 * HOUR, 6);
        assert_eq!(interval, 4 * HOUR);
    }

    #[test]
    fn selects_day_for_week_spans() {
        let interval = choose_time_interval(7 * DAY, 7);
        assert_eq!(interval, DAY);
    }

    #[test]
    fn falls_back_to_largest_interval() {
        assert_eq!(
            choose_time_interval(100 * 365 * DAY, 10),
            *TIME_INTERVALS_MS.last().unwrap()
        );
    }

    #[test]
    fn zero_and_negative_spans_are_safe() {
        assert_eq!(choose_time_interval(0, 4), 1_000);
        assert_eq!(choose_time_interval(-1, 4), 1_000);
        assert_eq!(choose_time_interval(DAY, 0), DAY);
    }

    #[test]
    fn alignment_handles_negative_timestamps() {
        assert_eq!(align_down(-1, MINUTE), -MINUTE);
        assert_eq!(align_down(-MINUTE, MINUTE), -MINUTE);
        assert_eq!(align_up(-1, MINUTE), 0);
        assert_eq!(align_up(-MINUTE, MINUTE), -MINUTE);
    }

    #[test]
    fn alignment_is_idempotent_on_boundaries() {
        let ts = 1_700_000_000_000;
        let down = align_down(ts, HOUR);
        assert_eq!(align_down(down, HOUR), down);
        assert_eq!(align_up(down, HOUR), down);
    }

    #[test]
    fn buffer_index_returns_in_range_timestamp_and_preserves_gaps() {
        let timestamps = [0, MINUTE, 7 * MINUTE, 8 * MINUTE];
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 2.0, "1m", MINUTE),
            Some(7 * MINUTE)
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 1.4, "1m", MINUTE),
            Some(MINUTE)
        );
    }

    #[test]
    fn buffer_index_extends_past_and_future() {
        let timestamps = [100 * MINUTE, 101 * MINUTE, 102 * MINUTE];
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, -1.0, "1m", 0),
            Some(99 * MINUTE)
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, -3.0, "1m", 0),
            Some(97 * MINUTE)
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 3.0, "1m", 0),
            Some(103 * MINUTE)
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 5.0, "1m", 0),
            Some(105 * MINUTE)
        );
    }

    #[test]
    fn buffer_index_uses_exact_timeframe_over_median_interval() {
        let timestamps = [1_000];
        // A single candle has no median, but the selected timeframe is exact.
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 1.0, "1h", MINUTE),
            Some(1_000 + HOUR)
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, -1.0, "1d", MINUTE),
            Some(1_000 - DAY)
        );
    }

    #[test]
    fn buffer_index_falls_back_to_interval_for_unknown_timeframe() {
        assert_eq!(
            timestamp_at_buffer_index(&[1_000], 1.0, "unknown", 30_000),
            Some(31_000)
        );
        assert_eq!(timeframe_interval_ms("unknown"), None);
    }

    #[test]
    fn buffer_index_steps_calendar_months() {
        use chrono::TimeZone;
        let jan = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let feb = Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        let mar = Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let dec = Utc.with_ymd_and_hms(2023, 12, 1, 0, 0, 0).unwrap();
        let apr = Utc.with_ymd_and_hms(2024, 4, 1, 0, 0, 0).unwrap();
        let timestamps = [
            jan.timestamp_millis(),
            feb.timestamp_millis(),
            mar.timestamp_millis(),
        ];
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, 3.0, "1M", 30 * DAY),
            Some(apr.timestamp_millis())
        );
        assert_eq!(
            timestamp_at_buffer_index(&timestamps, -1.0, "1M", 30 * DAY),
            Some(dec.timestamp_millis())
        );
    }

    #[test]
    fn buffer_index_handles_empty_single_and_invalid() {
        assert_eq!(timestamp_at_buffer_index(&[], -1.0, "1m", MINUTE), None);
        assert_eq!(
            timestamp_at_buffer_index(&[5_000], 0.0, "1m", MINUTE),
            Some(5_000)
        );
        assert_eq!(
            timestamp_at_buffer_index(&[5_000], 2.0, "1m", MINUTE),
            Some(5_000 + 2 * MINUTE)
        );
        assert_eq!(
            timestamp_at_buffer_index(&[5_000], f64::NAN, "1m", MINUTE),
            None
        );
    }
}
