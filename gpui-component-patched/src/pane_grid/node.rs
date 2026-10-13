use super::Axis;
use serde::{Deserialize, Serialize};

/// A unique identifier for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pane(pub usize);

/// A unique identifier for a split (divider between panes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Split(pub usize);

/// The internal representation of a pane grid layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Node {
    /// A split dividing the space into two regions.
    Split {
        /// The axis along which the split is made.
        axis: Axis,
        /// The ratio of space allocated to the first (left/top) region (0.0 to 1.0).
        ratio: f32,
        /// The unique identifier for this split.
        id: Split,
        /// The left or top child node.
        a: Box<Node>,
        /// The right or bottom child node.
        b: Box<Node>,
    },
    /// A leaf node containing a pane.
    Pane(Pane),
}

impl Node {
    /// Creates a new pane node.
    pub fn pane(id: Pane) -> Self {
        Node::Pane(id)
    }

    /// Creates a new split node.
    pub fn split(id: Split, axis: Axis, ratio: f32, a: Node, b: Node) -> Self {
        Node::Split {
            axis,
            ratio: ratio.clamp(0.0, 1.0),
            id,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Returns the pane identifier if this is a pane node.
    pub fn pane_id(&self) -> Option<Pane> {
        match self {
            Node::Pane(id) => Some(*id),
            _ => None,
        }
    }

    /// Returns all pane identifiers in the tree.
    pub fn panes(&self) -> Vec<Pane> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes(&self, panes: &mut Vec<Pane>) {
        match self {
            Node::Pane(id) => panes.push(*id),
            Node::Split { a, b, .. } => {
                a.collect_panes(panes);
                b.collect_panes(panes);
            }
        }
    }

    /// Returns all split identifiers in the tree.
    pub fn splits(&self) -> Vec<Split> {
        let mut splits = Vec::new();
        self.collect_splits(&mut splits);
        splits
    }

    fn collect_splits(&self, splits: &mut Vec<Split>) {
        match self {
            Node::Pane(_) => {}
            Node::Split { id, a, b, .. } => {
                splits.push(*id);
                a.collect_splits(splits);
                b.collect_splits(splits);
            }
        }
    }
}
