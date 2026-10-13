use super::{Axis, Configuration, Node, Pane, Split};
use std::collections::HashMap;

/// The state of a pane grid.
#[derive(Debug, Clone)]
pub struct State<T> {
    /// The root node of the layout tree.
    pub(crate) root: Node,
    /// The content associated with each pane.
    pub(crate) panes: HashMap<Pane, T>,
    /// The currently active (focused) pane.
    pub(crate) active: Option<Pane>,
    /// Counter for generating unique pane identifiers.
    pane_counter: usize,
    /// Counter for generating unique split identifiers.
    split_counter: usize,
}

impl<T> State<T> {
    /// Creates a new state with a single pane.
    pub fn new(content: T) -> Self {
        let pane = Pane(0);
        let mut panes = HashMap::new();
        panes.insert(pane, content);

        State {
            root: Node::pane(pane),
            panes,
            active: Some(pane),
            pane_counter: 1,
            split_counter: 0,
        }
    }

    /// Creates a new state from a configuration.
    pub fn with_configuration(config: Configuration<T>) -> Self {
        let mut state = State {
            root: Node::pane(Pane(0)),
            panes: HashMap::new(),
            active: None,
            pane_counter: 0,
            split_counter: 0,
        };

        state.root = state.build_from_config(config);
        if let Some(first_pane) = state.root.panes().first() {
            state.active = Some(*first_pane);
        }

        state
    }

    fn build_from_config(&mut self, config: Configuration<T>) -> Node {
        match config {
            Configuration::Pane(content) => {
                let pane = Pane(self.pane_counter);
                self.pane_counter += 1;
                self.panes.insert(pane, content);
                Node::pane(pane)
            }
            Configuration::Split { axis, ratio, a, b } => {
                let split = Split(self.split_counter);
                self.split_counter += 1;
                let node_a = self.build_from_config(*a);
                let node_b = self.build_from_config(*b);
                Node::split(split, axis, ratio, node_a, node_b)
            }
        }
    }

    /// Splits a pane along the given axis.
    ///
    /// Returns the split identifier and the new pane identifier, or None if the pane doesn't exist.
    pub fn split(&mut self, axis: Axis, pane: Pane, content: T) -> Option<(Split, Pane)> {
        if !self.panes.contains_key(&pane) {
            return None;
        }

        let split = Split(self.split_counter);
        self.split_counter += 1;

        let new_pane = Pane(self.pane_counter);
        self.pane_counter += 1;

        self.panes.insert(new_pane, content);

        // Replace the pane node with a split node
        self.root = self.replace_pane_with_split(self.root.clone(), pane, split, axis, new_pane);
        self.active = Some(new_pane);

        Some((split, new_pane))
    }

    fn replace_pane_with_split(
        &self,
        node: Node,
        target_pane: Pane,
        split: Split,
        axis: Axis,
        new_pane: Pane,
    ) -> Node {
        match node {
            Node::Pane(pane) if pane == target_pane => {
                Node::split(split, axis, 0.5, Node::pane(pane), Node::pane(new_pane))
            }
            Node::Split {
                axis: old_axis,
                ratio,
                id,
                a,
                b,
            } => {
                let new_a = self.replace_pane_with_split(*a, target_pane, split, axis, new_pane);
                let new_b = self.replace_pane_with_split(*b, target_pane, split, axis, new_pane);
                Node::split(id, old_axis, ratio, new_a, new_b)
            }
            other => other,
        }
    }

    /// Swaps the positions of two panes.
    pub fn swap(&mut self, a: Pane, b: Pane) {
        if a == b || !self.panes.contains_key(&a) || !self.panes.contains_key(&b) {
            return;
        }

        self.root = self.swap_panes(self.root.clone(), a, b);
    }

    fn swap_panes(&self, node: Node, a: Pane, b: Pane) -> Node {
        match node {
            Node::Pane(pane) => {
                if pane == a {
                    Node::pane(b)
                } else if pane == b {
                    Node::pane(a)
                } else {
                    Node::pane(pane)
                }
            }
            Node::Split {
                axis,
                ratio,
                id,
                a: node_a,
                b: node_b,
            } => {
                let new_a = self.swap_panes(*node_a, a, b);
                let new_b = self.swap_panes(*node_b, a, b);
                Node::split(id, axis, ratio, new_a, new_b)
            }
        }
    }

    /// Resizes a split by setting a new ratio.
    pub fn resize(&mut self, split: Split, ratio: f32) {
        self.root = self.update_split_ratio(self.root.clone(), split, ratio.clamp(0.0, 1.0));
    }

    fn update_split_ratio(&self, node: Node, target_split: Split, new_ratio: f32) -> Node {
        match node {
            Node::Split {
                axis,
                ratio,
                id,
                a,
                b,
            } => {
                let new_ratio = if id == target_split { new_ratio } else { ratio };
                let new_a = self.update_split_ratio(*a, target_split, new_ratio);
                let new_b = self.update_split_ratio(*b, target_split, new_ratio);
                Node::split(id, axis, new_ratio, new_a, new_b)
            }
            other => other,
        }
    }

    /// Closes a pane and returns its content.
    ///
    /// If this is the last pane, returns None.
    pub fn close(&mut self, pane: Pane) -> Option<T> {
        let content = self.panes.remove(&pane)?;

        // If this is the last pane, don't remove it
        if self.panes.is_empty() {
            self.panes.insert(pane, content);
            return None;
        }

        // Remove the pane from the tree
        self.root = self.remove_pane(self.root.clone(), pane)?;

        // Update active pane if needed
        if self.active == Some(pane) {
            self.active = self.root.panes().first().copied();
        }

        Some(content)
    }

    fn remove_pane(&self, node: Node, target_pane: Pane) -> Option<Node> {
        match node {
            Node::Pane(pane) if pane == target_pane => None,
            Node::Split {
                axis,
                ratio,
                id,
                a,
                b,
            } => {
                let new_a = self.remove_pane(*a.clone(), target_pane);
                let new_b = self.remove_pane(*b.clone(), target_pane);

                match (new_a, new_b) {
                    (Some(a), Some(b)) => Some(Node::split(id, axis, ratio, a, b)),
                    (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                    (None, None) => None,
                }
            }
            other => Some(other),
        }
    }

    /// Returns an iterator over all panes and their content.
    pub fn iter(&self) -> impl Iterator<Item = (Pane, &T)> {
        self.panes.iter().map(|(pane, content)| (*pane, content))
    }

    /// Returns a mutable iterator over all panes and their content.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Pane, &mut T)> {
        self.panes
            .iter_mut()
            .map(|(pane, content)| (*pane, content))
    }

    /// Returns the content of a pane.
    pub fn get(&self, pane: Pane) -> Option<&T> {
        self.panes.get(&pane)
    }

    /// Returns a mutable reference to the content of a pane.
    pub fn get_mut(&mut self, pane: Pane) -> Option<&mut T> {
        self.panes.get_mut(&pane)
    }

    /// Returns the currently active pane.
    pub fn active(&self) -> Option<Pane> {
        self.active
    }

    /// Returns a reference to the root node.
    pub fn root(&self) -> Option<&Node> {
        Some(&self.root)
    }

    /// Gets the ratio of a split.
    pub fn get_split_ratio(&self, split: Split) -> Option<f32> {
        self.find_split_ratio(&self.root, split)
    }

    fn find_split_ratio(&self, node: &Node, target: Split) -> Option<f32> {
        match node {
            Node::Pane(_) => None,
            Node::Split {
                id, ratio, a, b, ..
            } => {
                if *id == target {
                    Some(*ratio)
                } else {
                    self.find_split_ratio(a, target)
                        .or_else(|| self.find_split_ratio(b, target))
                }
            }
        }
    }

    /// Sets the active pane.
    pub fn set_active(&mut self, pane: Pane) {
        if self.panes.contains_key(&pane) {
            self.active = Some(pane);
        }
    }

    /// Returns all panes in the grid.
    pub fn panes(&self) -> Vec<Pane> {
        self.root.panes()
    }

    /// Returns the number of panes.
    pub fn len(&self) -> usize {
        self.panes.len()
    }

    /// Returns true if there are no panes.
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_pane() {
        let state = State::new("content");
        assert_eq!(state.len(), 1);
        assert!(state.active().is_some());
    }

    #[test]
    fn test_split() {
        let mut state = State::new("content1");
        let pane1 = state.panes()[0];

        let result = state.split(Axis::Vertical, pane1, "content2");
        assert!(result.is_some());
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn test_swap() {
        let mut state = State::new("content1");
        let pane1 = state.panes()[0];
        state.split(Axis::Vertical, pane1, "content2");

        let panes = state.panes();
        assert_eq!(panes.len(), 2);

        state.swap(panes[0], panes[1]);
        // State should still have 2 panes after swap
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn test_close() {
        let mut state = State::new("content1");
        let pane1 = state.panes()[0];
        state.split(Axis::Vertical, pane1, "content2");

        let panes = state.panes();
        let content = state.close(panes[1]);
        assert!(content.is_some());
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn test_close_last_pane() {
        let mut state = State::new("content");
        let pane = state.panes()[0];

        let content = state.close(pane);
        assert!(content.is_none());
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn test_resize() {
        let mut state = State::new("content1");
        let pane1 = state.panes()[0];

        let (split, _) = state.split(Axis::Vertical, pane1, "content2").unwrap();
        state.resize(split, 0.7);

        // Verify the split exists in the tree
        assert!(state.root.splits().contains(&split));
    }
}
