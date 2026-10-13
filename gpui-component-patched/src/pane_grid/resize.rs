use super::Split;
use serde::{Deserialize, Serialize};

/// A resize event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ResizeEvent {
    /// The split that was resized.
    pub split: Split,
    /// The new ratio for the split.
    pub ratio: f32,
}

impl ResizeEvent {
    /// Creates a new resize event.
    pub fn new(split: Split, ratio: f32) -> Self {
        ResizeEvent {
            split,
            ratio: ratio.clamp(0.0, 1.0),
        }
    }
}
