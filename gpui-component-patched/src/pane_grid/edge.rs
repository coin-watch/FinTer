use serde::{Deserialize, Serialize};

/// An edge of a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Edge {
    /// The top edge.
    Top,
    /// The bottom edge.
    Bottom,
    /// The left edge.
    Left,
    /// The right edge.
    Right,
}

impl Edge {
    /// Returns all four edges.
    pub fn all() -> [Edge; 4] {
        [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right]
    }
}
