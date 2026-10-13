#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub start: f64,
    pub visible: f64,
    pub pinned: bool,
}

impl Viewport {
    pub fn zoom(self, factor: f64, anchor: f64, max: f64) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        let visible = (self.visible * factor).clamp(1.0, max.max(1.0));
        let anchor = anchor.clamp(0.0, 1.0);
        Self {
            start: (self.start + self.visible * anchor - visible * anchor)
                .clamp(0.0, (max - visible).max(0.0)),
            visible,
            pinned: false,
        }
    }

    pub fn pan(self, delta: f64, max: f64) -> Self {
        if !delta.is_finite() {
            return self;
        }
        Self {
            start: (self.start + delta).clamp(0.0, (max - self.visible).max(0.0)),
            pinned: false,
            ..self
        }
    }

    pub fn latest(self, max: f64) -> Self {
        Self {
            start: (max - self.visible).max(0.0),
            pinned: true,
            ..self
        }
    }

    pub fn prepend(self, count: f64) -> Self {
        Self {
            start: self.start + count.max(0.0),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_cursor_anchor() {
        let v = Viewport { start: 100.0, visible: 50.0, pinned: false };
        let z = v.zoom(0.5, 0.25, 1000.0);
        assert_eq!(z.start + z.visible * 0.25, 112.5);
        assert_eq!(z.visible, 25.0);
    }

    #[test]
    fn pan_unpins_and_clamps() {
        let v = Viewport { start: 20.0, visible: 50.0, pinned: true };
        let p = v.pan(-100.0, 100.0);
        assert_eq!(p.start, 0.0);
        assert!(!p.pinned);
        assert_eq!(p.latest(100.0).start, 50.0);
    }

    #[test]
    fn prepend_preserves_visible_candles() {
        let v = Viewport { start: 20.0, visible: 50.0, pinned: false };
        assert_eq!(v.prepend(10.0).start, 30.0);
    }

    #[test]
    fn invalid_input_and_empty_data_are_safe() {
        let v = Viewport { start: 0.0, visible: 50.0, pinned: true };
        assert_eq!(v.zoom(f64::NAN, 0.5, 0.0), v);
        assert_eq!(v.pan(f64::NAN, 100.0), v);
    }
}
