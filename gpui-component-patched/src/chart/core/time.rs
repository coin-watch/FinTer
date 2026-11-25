//! Time-axis tick selection.

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
}
