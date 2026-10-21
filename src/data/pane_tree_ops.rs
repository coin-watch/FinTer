use crate::data::pane_tree::PaneNode;

impl PaneNode {
    pub fn split_matrix(&mut self, target_id: &str, rows: usize, cols: usize) -> bool {
        self.modify_node(target_id, &mut |node| {
            if let PaneNode::Pane { id, data } = node.clone() {
                if rows <= 1 && cols <= 1 { return; }
                let mut row_nodes = Vec::new();
                for r in 0..rows {
                    let mut col_nodes = Vec::new();
                    for c in 0..cols {
                        let cell_id = if r == 0 && c == 0 { id.clone() } else { PaneNode::generate_id() };
                        let cell_data = if r == 0 && c == 0 { data.clone() } else { crate::data::pane_tree::ModuleData::default() };
                        col_nodes.push(PaneNode::Pane { id: cell_id, data: cell_data });
                    }
                    
                    let mut current_row = col_nodes.pop().unwrap();
                    
                    let mut splits_made = 1;
                    
                    while let Some(left_pane) = col_nodes.pop() {
                        current_row = PaneNode::Split {
                            id: PaneNode::generate_id(),
                            axis: crate::data::pane_tree::Axis::Vertical,
                            ratio: 1.0 / (splits_made + 1) as f32, // evenly distribute space
                            a: Box::new(left_pane),
                            b: Box::new(current_row),
                        };
                        splits_made += 1;
                    }
                    row_nodes.push(current_row);
                }
                
                let mut current_tree = row_nodes.pop().unwrap();
                
                let mut splits_made = 1;
                
                while let Some(top_pane) = row_nodes.pop() {
                    current_tree = PaneNode::Split {
                        id: PaneNode::generate_id(),
                        axis: crate::data::pane_tree::Axis::Horizontal,
                        ratio: 1.0 / (splits_made + 1) as f32,
                        a: Box::new(top_pane),
                        b: Box::new(current_tree),
                    };
                    splits_made += 1;
                }
                
                *node = current_tree;
            }
        })
    }
    pub fn modify_node<F>(&mut self, target_id: &str, f: &mut F) -> bool
    where
        F: FnMut(&mut PaneNode),
    {
        match self {
            PaneNode::Pane { id, .. } => {
                if id == target_id {
                    f(self);
                    true
                } else {
                    false
                }
            }
            PaneNode::Split { id, a, b, .. } => {
                if id == target_id {
                    f(self);
                    true
                } else {
                    a.modify_node(target_id, f) || b.modify_node(target_id, f)
                }
            }
        }
    }

    pub fn find_node(&self, target_id: &str, f: &mut impl FnMut(&PaneNode)) -> bool {
        match self {
            PaneNode::Pane { id, .. } => {
                if id == target_id {
                    f(self);
                    true
                } else {
                    false
                }
            }
            PaneNode::Split { id, a, b, .. } => {
                if id == target_id {
                    f(self);
                    true
                } else {
                    a.find_node(target_id, f) || b.find_node(target_id, f)
                }
            }
        }
    }
    pub fn remove_node(self, target_id: &str) -> Option<PaneNode> {
        match self {
            PaneNode::Pane { id, data } => {
                if id == target_id {
                    None
                } else {
                    Some(PaneNode::Pane { id, data })
                }
            }
            PaneNode::Split {
                id,
                axis,
                ratio,
                a,
                b,
            } => {
                if id == target_id {
                    None
                } else {
                    let new_a = a.remove_node(target_id);
                    let new_b = b.remove_node(target_id);

                    match (new_a, new_b) {
                        (Some(a_survives), Some(b_survives)) => Some(PaneNode::Split {
                            id,
                            axis,
                            ratio,
                            a: Box::new(a_survives),
                            b: Box::new(b_survives),
                        }),
                        (Some(a_survives), None) => Some(a_survives),
                        (None, Some(b_survives)) => Some(b_survives),
                        (None, None) => None,
                    }
                }
            }
        }
    }

    pub fn swap_panes(&mut self, id_a: &str, id_b: &str) -> bool {
        if id_a == id_b {
            return false;
        }

        let mut node_a = None;
        self.find_node(id_a, &mut |n| {
            if let PaneNode::Pane { .. } = n {
                node_a = Some(n.clone());
            }
        });

        let mut node_b = None;
        self.find_node(id_b, &mut |n| {
            if let PaneNode::Pane { .. } = n {
                node_b = Some(n.clone());
            }
        });

        if let (Some(a), Some(b)) = (node_a, node_b) {
            Self::swap_nodes_recursive(self, id_a, id_b, &a, &b);
            true
        } else {
            false
        }
    }

    fn swap_nodes_recursive(node: &mut PaneNode, id_a: &str, id_b: &str, node_a: &PaneNode, node_b: &PaneNode) {
        match node {
            PaneNode::Pane { id, .. } => {
                if id == id_a {
                    *node = node_b.clone();
                } else if id == id_b {
                    *node = node_a.clone();
                }
            }
            PaneNode::Split { a, b, .. } => {
                Self::swap_nodes_recursive(a, id_a, id_b, node_a, node_b);
                Self::swap_nodes_recursive(b, id_a, id_b, node_a, node_b);
            }
        }
    }

    pub fn swap_data(&mut self, id_a: &str, id_b: &str) -> bool {
        self.swap_panes(id_a, id_b)
    }
}

#[cfg(test)]
mod tests {
    use crate::data::pane_tree::{Axis, ModuleData, PaneNode};

    #[test]
    fn test_swap_panes() {
        let mut tree = PaneNode::Split {
            id: "split-1".to_string(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            a: Box::new(PaneNode::Pane {
                id: "pane-1".to_string(),
                data: ModuleData {
                    module_type: Some("chart".to_string()),
                    config: serde_json::json!({ "symbol": "BTC/USDT" }),
                },
            }),
            b: Box::new(PaneNode::Pane {
                id: "pane-2".to_string(),
                data: ModuleData {
                    module_type: Some("ticker".to_string()),
                    config: serde_json::json!({ "symbol": "ETH/USDT" }),
                },
            }),
        };

        assert!(tree.swap_data("pane-1", "pane-2"));

        if let PaneNode::Split { a, b, .. } = tree {
            if let PaneNode::Pane { id, data } = *a {
                assert_eq!(id, "pane-2");
                assert_eq!(data.module_type.as_deref(), Some("ticker"));
            } else {
                panic!("expected pane a");
            }

            if let PaneNode::Pane { id, data } = *b {
                assert_eq!(id, "pane-1");
                assert_eq!(data.module_type.as_deref(), Some("chart"));
            } else {
                panic!("expected pane b");
            }
        }
    }
}
