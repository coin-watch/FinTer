/// Price formatting utilities that respect exchange precision rules.
///
/// This module provides exchange-aware price formatting that follows the
/// precision rules defined by each exchange (e.g., tickSize, pricePrecision).
use crate::data::models::PrecisionInfo;

/// Format a price value according to exchange precision rules.
///
/// Priority order:
/// 1. Use exchange-provided precision (from PrecisionInfo)
/// 2. Fall back to magnitude-based formatting
///
/// # Arguments
/// * `price` - The price value to format
/// * `precision` - Optional precision metadata from the exchange
///
/// # Examples
/// ```
/// use finter::data::price_format::format_price;
/// use finter::data::models::PrecisionInfo;
///
/// // With exchange precision
/// let precision = Some(PrecisionInfo {
///     price: Some(2),
///     amount: None,
///     tick_size: Some(0.01),
/// });
/// assert_eq!(format_price(1234.5678, precision.as_ref()), "1234.57");
///
/// // Without exchange precision (magnitude-based fallback)
/// assert_eq!(format_price(0.00012345, None), "0.00012345");
/// ```
pub fn format_price(price: f64, precision: Option<&PrecisionInfo>) -> String {
    // If we have exchange precision metadata, use it
    if let Some(prec) = precision {
        // Prefer explicit price precision
        if let Some(decimals) = prec.price {
            return format_with_decimals(price, decimals);
        }

        // Fall back to inferring from tickSize
        if let Some(tick_size) = prec.tick_size {
            return format_with_tick_size(price, tick_size);
        }
    }

    // Fall back to magnitude-based formatting
    format_price_magnitude(price)
}

/// Format price with a specific number of decimal places.
fn format_with_decimals(price: f64, decimals: i32) -> String {
    if decimals < 0 {
        // Invalid precision, fall back to magnitude-based
        return format_price_magnitude(price);
    }

    // Clamp to reasonable range (0-12 decimals)
    let decimals = decimals.clamp(0, 12) as usize;
    format!("{:.width$}", price, width = decimals)
}

/// Format price based on tick size (minimum price increment).
///
/// The tick size determines the precision:
/// - tickSize = 0.01 → 2 decimals
/// - tickSize = 0.0001 → 4 decimals
/// - tickSize = 0.00000001 → 8 decimals
fn format_with_tick_size(price: f64, tick_size: f64) -> String {
    if tick_size <= 0.0 || !tick_size.is_finite() {
        return format_price_magnitude(price);
    }

    // Count decimal places in tick size
    let decimals = count_decimal_places(tick_size);
    format_with_decimals(price, decimals as i32)
}

/// Count the number of decimal places in a number.
fn count_decimal_places(value: f64) -> usize {
    if !value.is_finite() || value <= 0.0 {
        return 2; // default
    }

    // Convert to string and count digits after decimal point
    let s = format!("{:.12}", value); // Use high precision
    let s = s.trim_end_matches('0').trim_end_matches('.');

    if let Some(dot_pos) = s.find('.') {
        s.len() - dot_pos - 1
    } else {
        0
    }
}

/// Magnitude-based price formatting (fallback when no exchange precision available).
///
/// Rules:
/// - >= 10,000: 2 decimals (e.g., "50000.00")
/// - >= 1,000:  2 decimals (e.g., "1234.56")
/// - >= 1.0:    4 decimals (e.g., "123.4567")
/// - >= 0.01:   6 decimals (e.g., "0.123456")
/// - < 0.01:    8 decimals (e.g., "0.00012345")
pub fn format_price_magnitude(price: f64) -> String {
    let abs_price = price.abs();

    if abs_price >= 1_000.0 {
        format!("{:.2}", price)
    } else if abs_price >= 1.0 {
        format!("{:.4}", price)
    } else if abs_price >= 0.01 {
        format!("{:.6}", price)
    } else {
        format!("{:.8}", price)
    }
}

/// Format volume with K/M/B suffixes for compact display.
pub fn format_volume(volume: f64) -> String {
    if volume >= 1_000_000_000.0 {
        format!("{:.2}B", volume / 1_000_000_000.0)
    } else if volume >= 1_000_000.0 {
        format!("{:.2}M", volume / 1_000_000.0)
    } else if volume >= 1_000.0 {
        format!("{:.2}K", volume / 1_000.0)
    } else {
        format!("{:.2}", volume)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_with_decimals() {
        assert_eq!(format_with_decimals(1234.5678, 2), "1234.57");
        assert_eq!(format_with_decimals(1234.5678, 4), "1234.5678");
        assert_eq!(format_with_decimals(0.00012345, 8), "0.00012345");
        assert_eq!(format_with_decimals(50000.0, 2), "50000.00");
    }

    #[test]
    fn test_count_decimal_places() {
        assert_eq!(count_decimal_places(0.01), 2);
        assert_eq!(count_decimal_places(0.0001), 4);
        assert_eq!(count_decimal_places(0.00000001), 8);
        assert_eq!(count_decimal_places(1.0), 0);
        assert_eq!(count_decimal_places(0.1), 1);
    }

    #[test]
    fn test_format_with_tick_size() {
        // BTC/USDT on Binance: tickSize = 0.01
        assert_eq!(format_with_tick_size(50000.12345, 0.01), "50000.12");

        // ETH/BTC: tickSize = 0.000001
        assert_eq!(format_with_tick_size(0.05123456, 0.000001), "0.051235");

        // Altcoin with high precision: tickSize = 0.00000001
        assert_eq!(format_with_tick_size(0.00012345, 0.00000001), "0.00012345");
    }

    #[test]
    fn test_format_price_with_precision_info() {
        // Test with explicit price decimals
        let precision = PrecisionInfo {
            price: Some(2),
            amount: None,
            tick_size: None,
        };
        assert_eq!(format_price(1234.5678, Some(&precision)), "1234.57");

        // Test with tick size
        let precision = PrecisionInfo {
            price: None,
            amount: None,
            tick_size: Some(0.0001),
        };
        assert_eq!(format_price(0.12345678, Some(&precision)), "0.1235");
    }

    #[test]
    fn test_format_price_magnitude_fallback() {
        // No precision info → magnitude-based
        assert_eq!(format_price(50000.0, None), "50000.00");
        assert_eq!(format_price(1234.5678, None), "1234.57");
        assert_eq!(format_price(12.345678, None), "12.3457");
        assert_eq!(format_price(0.12345678, None), "0.123457");
        assert_eq!(format_price(0.00012345, None), "0.00012345");
    }

    #[test]
    fn test_format_volume() {
        assert_eq!(format_volume(1_234_567_890.0), "1.23B");
        assert_eq!(format_volume(12_345_678.0), "12.35M");
        assert_eq!(format_volume(12_345.0), "12.35K");
        assert_eq!(format_volume(123.45), "123.45");
    }

    #[test]
    fn test_binance_btc_usdt() {
        // Binance BTC/USDT: tickSize = 0.01, price precision = 2
        let precision = PrecisionInfo {
            price: Some(2),
            amount: Some(6),
            tick_size: Some(0.01),
        };

        assert_eq!(format_price(50123.456789, Some(&precision)), "50123.46");
        assert_eq!(format_price(49999.99, Some(&precision)), "49999.99");
        assert_eq!(format_price(50000.001, Some(&precision)), "50000.00");
    }

    #[test]
    fn test_binance_eth_btc() {
        // Binance ETH/BTC: tickSize = 0.000001, price precision = 6
        let precision = PrecisionInfo {
            price: Some(6),
            amount: Some(5),
            tick_size: Some(0.000001),
        };

        assert_eq!(format_price(0.0512345678, Some(&precision)), "0.051235");
        assert_eq!(format_price(0.051234, Some(&precision)), "0.051234");
    }

    #[test]
    fn test_kraken_xbt_usd() {
        // Kraken XBT/USD: tickSize = 0.1, price precision = 1
        let precision = PrecisionInfo {
            price: Some(1),
            amount: Some(8),
            tick_size: Some(0.1),
        };

        assert_eq!(format_price(50123.456, Some(&precision)), "50123.5");
        assert_eq!(format_price(49999.99, Some(&precision)), "50000.0");
    }
}
