use super::{Pane, Region};
use serde::{Deserialize, Serialize};

/// A drag-and-drop event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragEvent {
    /// A pane was successfully dropped.
    Dropped { pane: Pane, target: Target },
    /// The drag operation was cancelled.
    Cancelled,
}

/// A drop target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target {
    /// The pane that was targeted.
    pub pane: Pane,
    /// The region within the pane.
    pub region: Region,
}

impl Target {
    /// Creates a new drop target.
    pub fn new(pane: Pane, region: Region) -> Self {
        Target { pane, region }
    }
}
