use super::Edge;
use serde::{Deserialize, Serialize};

/// A region within a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Region {
    /// An edge region (for splitting).
    Edge(Edge),
    /// The center region (for swapping).
    Center,
}
