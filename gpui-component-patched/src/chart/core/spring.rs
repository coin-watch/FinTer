//! Critically damped spring used by pan inertia, zoom easing, and flag dodging.
//!
//! The analytic critically damped solution is used instead of Euler integration
//! so settle time is independent of the frame rate.

/// A one-dimensional critically damped spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
    /// Undamped angular frequency (radians per second).
    omega: f32,
}

impl Spring {
    pub const DEFAULT_OMEGA: f32 = 18.0;

    pub fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            omega: Self::DEFAULT_OMEGA,
        }
    }

    pub fn with_omega(value: f32, omega: f32) -> Self {
        let omega = if omega.is_finite() && omega > 0.0 {
            omega
        } else {
            Self::DEFAULT_OMEGA
        };
        Self {
            value,
            velocity: 0.0,
            target: value,
            omega,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target;
        }
    }

    /// Continue moving toward the target with an initial velocity.
    pub fn fling(&mut self, target: f32, velocity: f32) {
        self.set_target(target);
        if velocity.is_finite() {
            self.velocity = velocity;
        }
    }

    pub fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value;
            self.target = value;
            self.velocity = 0.0;
        }
    }

    pub fn is_active(&self) -> bool {
        (self.value - self.target).abs() > 0.01 || self.velocity.abs() > 0.05
    }

    /// Advance the spring by `dt` seconds.
    ///
    /// Returns `true` while the spring is still active.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !dt.is_finite() || dt <= 0.0 {
            return self.is_active();
        }
        let dt = dt.min(0.064);
        let displacement = self.value - self.target;
        let b = self.velocity + self.omega * displacement;
        let decay = (-self.omega * dt).exp();
        self.value = self.target + (displacement + b * dt) * decay;
        self.velocity = (b - self.omega * (displacement + b * dt)) * decay;

        if !self.is_active() {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle_seconds(fps: f32) -> f32 {
        let mut spring = Spring::new(0.0);
        spring.set_target(100.0);
        let dt = 1.0 / fps;
        let mut elapsed = 0.0;
        for _ in 0..(fps as usize * 5) {
            elapsed += dt;
            if !spring.tick(dt) {
                break;
            }
        }
        elapsed
    }

    #[test]
    fn settles_near_target() {
        let mut spring = Spring::new(0.0);
        spring.set_target(100.0);
        for _ in 0..240 {
            spring.tick(1.0 / 60.0);
        }
        assert!((spring.value() - 100.0).abs() < 0.01);
        assert!(!spring.is_active());
    }

    #[test]
    fn settle_time_is_frame_rate_independent() {
        let at_60 = settle_seconds(60.0);
        let at_120 = settle_seconds(120.0);
        let at_144 = settle_seconds(144.0);
        assert!((at_60 - at_120).abs() < 0.02, "{at_60} vs {at_120}");
        assert!((at_120 - at_144).abs() < 0.02, "{at_120} vs {at_144}");
    }

    #[test]
    fn crititically_damped_does_not_overshoot_materially() {
        let mut spring = Spring::new(0.0);
        spring.set_target(100.0);
        let mut max = 0.0f32;
        for _ in 0..120 {
            spring.tick(1.0 / 60.0);
            max = max.max(spring.value());
        }
        assert!(max <= 100.01, "overshoot was {max}");
    }

    #[test]
    fn invalid_dt_and_target_are_safe() {
        let mut spring = Spring::new(5.0);
        spring.set_target(f32::NAN);
        assert_eq!(spring.target(), 5.0);
        spring.tick(f32::NAN);
        assert_eq!(spring.value(), 5.0);
    }

    #[test]
    fn snap_stops_animation() {
        let mut spring = Spring::new(0.0);
        spring.set_target(100.0);
        spring.tick(1.0 / 60.0);
        spring.snap(42.0);
        assert_eq!(spring.value(), 42.0);
        assert!(!spring.is_active());
    }
}
