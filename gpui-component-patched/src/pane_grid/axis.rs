use serde::{Deserialize, Serialize};

/// The axis along which a split is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    /// Split horizontally (top and bottom).
    Horizontal,
    /// Split vertically (left and right).
    Vertical,
}

impl Axis {
    /// Returns the opposite axis.
    pub fn opposite(self) -> Self {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}
