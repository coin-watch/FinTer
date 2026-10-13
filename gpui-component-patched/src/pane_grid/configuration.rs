use super::Axis;
use serde::{Deserialize, Serialize};

/// The initial configuration of a pane grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Configuration<T> {
    /// A split configuration.
    Split {
        /// The axis along which to split.
        axis: Axis,
        /// The ratio of space for the first pane (0.0 to 1.0).
        ratio: f32,
        /// The first (left/top) configuration.
        a: Box<Configuration<T>>,
        /// The second (right/bottom) configuration.
        b: Box<Configuration<T>>,
    },
    /// A single pane with content.
    Pane(T),
}

impl<T> Configuration<T> {
    /// Creates a configuration with a single pane.
    pub fn pane(content: T) -> Self {
        Configuration::Pane(content)
    }

    /// Creates a split configuration.
    pub fn split(axis: Axis, ratio: f32, a: Configuration<T>, b: Configuration<T>) -> Self {
        Configuration::Split {
            axis,
            ratio: ratio.clamp(0.0, 1.0),
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Creates a horizontal split.
    pub fn hsplit(ratio: f32, a: Configuration<T>, b: Configuration<T>) -> Self {
        Self::split(Axis::Horizontal, ratio, a, b)
    }

    /// Creates a vertical split.
    pub fn vsplit(ratio: f32, a: Configuration<T>, b: Configuration<T>) -> Self {
        Self::split(Axis::Vertical, ratio, a, b)
    }
}
