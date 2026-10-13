/// A linear scale that maps a continuous domain `[domain_min, domain_max]`
/// to a continuous pixel range `[range_min, range_max]`.
///
/// Set `range_min > range_max` to invert the axis (useful for Y-axis where
/// price increases upward but pixel-y increases downward).
///
/// Ported from `/home/mimix/cw-native/ironcharts/src/graphics/scaling.rs`.
#[derive(Debug, Clone, Copy)]
pub struct LinearScale {
    domain_min: f64,
    domain_max: f64,
    range_min: f32,
    range_max: f32,
}

impl LinearScale {
    pub fn new(domain_min: f64, domain_max: f64, range_min: f32, range_max: f32) -> Self {
        Self {
            domain_min,
            domain_max,
            range_min,
            range_max,
        }
    }

    /// Map a domain value to a pixel coordinate.
    pub fn scale(&self, value: f64) -> f32 {
        let domain_range = self.domain_max - self.domain_min;
        let range_range = self.range_max - self.range_min;

        if domain_range.abs() < f64::EPSILON {
            return self.range_min;
        }

        let percent = (value - self.domain_min) / domain_range;
        self.range_min + (percent as f32 * range_range)
    }

    /// Map a pixel coordinate back to a domain value.
    pub fn inverse(&self, pixel: f32) -> f64 {
        let domain_range = self.domain_max - self.domain_min;
        let range_range = self.range_max - self.range_min;

        if range_range.abs() < f32::EPSILON {
            return self.domain_min;
        }

        let percent = (pixel - self.range_min) / range_range;
        self.domain_min + (percent as f64 * domain_range)
    }

    pub fn domain_min(&self) -> f64 {
        self.domain_min
    }

    pub fn domain_max(&self) -> f64 {
        self.domain_max
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_normal() {
        let s = LinearScale::new(0.0, 100.0, 0.0, 500.0);
        assert_eq!(s.scale(0.0), 0.0);
        assert_eq!(s.scale(50.0), 250.0);
        assert_eq!(s.scale(100.0), 500.0);
    }

    #[test]
    fn inverse_normal() {
        let s = LinearScale::new(0.0, 100.0, 0.0, 500.0);
        assert_eq!(s.inverse(0.0), 0.0);
        assert_eq!(s.inverse(250.0), 50.0);
        assert_eq!(s.inverse(500.0), 100.0);
    }

    #[test]
    fn scale_inverted_y_axis() {
        // price 0 → bottom (500 px), price 100 → top (0 px)
        let s = LinearScale::new(0.0, 100.0, 500.0, 0.0);
        assert_eq!(s.scale(0.0), 500.0);
        assert_eq!(s.scale(50.0), 250.0);
        assert_eq!(s.scale(100.0), 0.0);
    }

    #[test]
    fn inverse_inverted_y_axis() {
        let s = LinearScale::new(0.0, 100.0, 500.0, 0.0);
        assert_eq!(s.inverse(500.0), 0.0);
        assert_eq!(s.inverse(250.0), 50.0);
    }

    #[test]
    fn zero_domain_returns_range_min() {
        let s = LinearScale::new(10.0, 10.0, 0.0, 100.0);
        assert_eq!(s.scale(10.0), 0.0);
    }

    #[test]
    fn scale_roundtrip() {
        let s = LinearScale::new(43000.0, 45000.0, 400.0, 10.0);
        let price = 44000.0;
        let px = s.scale(price);
        let back = s.inverse(px);
        assert!((back - price).abs() < 0.001, "roundtrip failed: {back}");
    }
}
