use crate::data::pane_tree::PaneNode;

/// A flat, agent-facing description of one pane in a dashboard.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneDescriptor {
    /// 1-based depth-first index, stable for a given tree shape.
    pub index: usize,
    pub id: String,
    pub module_type: String,
    pub config: serde_json::Value,
    /// Position path such as `root/a`, useful for reasoning about layout.
    pub path: String,
}

impl PaneNode {
    /// Flattens every pane leaf into a stable, depth-first list.
    pub fn describe_panes(&self) -> Vec<PaneDescriptor> {
        let mut out = Vec::new();
        self.collect_descriptors(&mut out, "root");
        for (offset, descriptor) in out.iter_mut().enumerate() {
            descriptor.index = offset + 1;
        }
        out
    }

    fn collect_descriptors(&self, out: &mut Vec<PaneDescriptor>, path: &str) {
        match self {
            PaneNode::Pane { id, data } => out.push(PaneDescriptor {
                index: 0,
                id: id.clone(),
                module_type: data
                    .module_type
                    .clone()
                    .unwrap_or_else(|| "empty".to_string()),
                config: data.config.clone(),
                path: path.to_string(),
            }),
            PaneNode::Split { a, b, .. } => {
                a.collect_descriptors(out, &format!("{path}/a"));
                b.collect_descriptors(out, &format!("{path}/b"));
            }
        }
    }

    /// Resolves an agent-supplied pane reference to a concrete pane id.
    ///
    /// Accepts, in order of precedence:
    /// - an exact pane id,
    /// - `#3`, `3` or `pane 3` — the 1-based depth-first index,
    /// - a module type (`chart`, `ticker`, …) when exactly one pane matches,
    /// - a unique id prefix (the first 8 characters of a UUID, for example).
    pub fn resolve_pane_reference(&self, reference: &str) -> Option<String> {
        let raw = reference.trim();
        if raw.is_empty() {
            return None;
        }
        let panes = self.describe_panes();

        if let Some(exact) = panes.iter().find(|pane| pane.id == raw) {
            return Some(exact.id.clone());
        }

        let numeric = raw
            .trim_start_matches('#')
            .trim_start_matches("pane")
            .trim()
            .trim_start_matches('#')
            .trim();
        if let Ok(index) = numeric.parse::<usize>() {
            if let Some(pane) = panes.iter().find(|pane| pane.index == index) {
                return Some(pane.id.clone());
            }
        }

        let lowered = raw.to_ascii_lowercase();
        let by_type: Vec<&PaneDescriptor> = panes
            .iter()
            .filter(|pane| pane.module_type.eq_ignore_ascii_case(&lowered))
            .collect();
        if by_type.len() == 1 {
            return Some(by_type[0].id.clone());
        }

        let by_prefix: Vec<&PaneDescriptor> = panes
            .iter()
            .filter(|pane| pane.id.starts_with(raw))
            .collect();
        if by_prefix.len() == 1 {
            return Some(by_prefix[0].id.clone());
        }

        None
    }

    /// Resolves a split reference to a concrete split id.
    ///
    /// Accepts an exact split id, a `#N` index over splits in depth-first order,
    /// or a unique id prefix.
    pub fn resolve_split_reference(&self, reference: &str) -> Option<String> {
        let raw = reference.trim();
        if raw.is_empty() {
            return None;
        }
        let splits = self.split_ids();
        if let Some(exact) = splits.iter().find(|id| id.as_str() == raw) {
            return Some(exact.clone());
        }
        let numeric = raw.trim_start_matches('#').trim_start_matches("split").trim();
        if let Ok(index) = numeric.parse::<usize>() {
            if index >= 1 && index <= splits.len() {
                return Some(splits[index - 1].clone());
            }
        }
        let prefixed: Vec<&String> = splits.iter().filter(|id| id.starts_with(raw)).collect();
        if prefixed.len() == 1 {
            return Some(prefixed[0].clone());
        }
        None
    }

    /// All split ids in depth-first order.
    pub fn split_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_split_ids(&mut out);
        out
    }

    fn collect_split_ids(&self, out: &mut Vec<String>) {
        if let PaneNode::Split { id, a, b, .. } = self {
            out.push(id.clone());
            a.collect_split_ids(out);
            b.collect_split_ids(out);
        }
    }

    /// True when the tree contains a pane with this id.
    pub fn has_pane(&self, pane_id: &str) -> bool {
        self.describe_panes().iter().any(|pane| pane.id == pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::pane_tree::{Axis, ModuleData};

    fn sample() -> PaneNode {
        PaneNode::Split {
            id: "split-root".into(),
            axis: Axis::Vertical,
            ratio: 0.5,
            a: Box::new(PaneNode::Pane {
                id: "pane-chart".into(),
                data: ModuleData {
                    module_type: Some("chart".into()),
                    config: serde_json::json!({"symbol": "BTC/USDT"}),
                },
            }),
            b: Box::new(PaneNode::Split {
                id: "split-right".into(),
                axis: Axis::Horizontal,
                ratio: 0.4,
                a: Box::new(PaneNode::Pane {
                    id: "pane-ticker".into(),
                    data: ModuleData {
                        module_type: Some("ticker".into()),
                        config: serde_json::json!({}),
                    },
                }),
                b: Box::new(PaneNode::Pane {
                    id: "pane-agent".into(),
                    data: ModuleData {
                        module_type: Some("agent".into()),
                        config: serde_json::json!({}),
                    },
                }),
            }),
        }
    }

    #[test]
    fn describes_panes_depth_first_with_indices() {
        let panes = sample().describe_panes();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].index, 1);
        assert_eq!(panes[0].id, "pane-chart");
        assert_eq!(panes[0].path, "root/a");
        assert_eq!(panes[1].id, "pane-ticker");
        assert_eq!(panes[2].id, "pane-agent");
        assert_eq!(panes[2].path, "root/b/b");
    }

    #[test]
    fn resolves_by_id_index_type_and_prefix() {
        let tree = sample();
        assert_eq!(tree.resolve_pane_reference("pane-ticker").as_deref(), Some("pane-ticker"));
        assert_eq!(tree.resolve_pane_reference("#2").as_deref(), Some("pane-ticker"));
        assert_eq!(tree.resolve_pane_reference("2").as_deref(), Some("pane-ticker"));
        assert_eq!(tree.resolve_pane_reference("pane 3").as_deref(), Some("pane-agent"));
        assert_eq!(tree.resolve_pane_reference("agent").as_deref(), Some("pane-agent"));
        assert_eq!(tree.resolve_pane_reference("pane-ch").as_deref(), Some("pane-chart"));
        assert_eq!(tree.resolve_pane_reference("nope"), None);
        assert_eq!(tree.resolve_pane_reference(""), None);
    }

    #[test]
    fn resolves_splits() {
        let tree = sample();
        assert_eq!(tree.split_ids(), vec!["split-root", "split-right"]);
        assert_eq!(tree.resolve_split_reference("split-right").as_deref(), Some("split-right"));
        assert_eq!(tree.resolve_split_reference("#2").as_deref(), Some("split-right"));
        assert_eq!(tree.resolve_split_reference("#9"), None);
    }

    #[test]
    /// The full Agent path: a fenced `ui` reply is parsed, each action's pane
    /// reference is resolved against a real tree, and the mutation lands. This
    /// covers the exact sequence the panel runs, minus the GPUI plumbing.
    #[test]
    fn reply_directives_resolve_and_mutate_a_real_tree() {
        use crate::data::pane_control::{parse_ui_actions_in_text, ParsedUiAction};

        let mut tree: PaneNode = serde_json::from_value(serde_json::json!({
            "kind": "split",
            "id": "root",
            "axis": "vertical",
            "ratio": 0.5,
            "a": {"kind": "pane", "id": "p1", "data": {"type": "chart", "config": {"symbol": "BTC/USDT"}}},
            "b": {"kind": "pane", "id": "p2", "data": {"type": "ticker", "config": {}}},
        }))
        .expect("valid tree");

        let reply = "Here you go.\n\n```ui\n{\"action\":\"set_module\",\"pane\":\"#2\",\"module_type\":\"orderbook\",\"config\":{\"symbol\":\"ETH/USDT\"}}\n```\n\n```ui\n{\"action\":\"swap\",\"pane\":\"chart\",\"with\":\"#2\"}\n```";

        let actions: Vec<_> = parse_ui_actions_in_text(reply)
            .into_iter()
            .map(|parsed| match parsed {
                ParsedUiAction::Action(action) => action,
                ParsedUiAction::Invalid(message) => panic!("unexpected invalid block: {message}"),
            })
            .collect();
        assert_eq!(actions.len(), 2);

        // Action 1 targets pane #2 by index and replaces its module.
        let pane_id = tree
            .resolve_pane_reference(actions[0].pane().unwrap())
            .expect("index resolves");
        assert_eq!(pane_id, "p2");
        tree.modify_node(&pane_id, &mut |node| {
            if let PaneNode::Pane { data, .. } = node {
                data.module_type = Some("orderbook".into());
                data.config = serde_json::json!({"symbol": "ETH/USDT"});
            }
        });
        let panes = tree.describe_panes();
        assert_eq!(panes[1].module_type, "orderbook");
        assert_eq!(panes[1].config["symbol"], "ETH/USDT");

        // Action 2 addresses the remaining chart pane by module type.
        let chart = tree
            .resolve_pane_reference(actions[1].pane().unwrap())
            .expect("module type resolves");
        assert_eq!(chart, "p1");
        assert!(tree.swap_panes(&chart, &pane_id));
        let swapped = tree.describe_panes();
        assert_eq!(swapped[0].module_type, "orderbook");
        assert_eq!(swapped[1].module_type, "chart");
    }

    #[test]
    fn has_pane_checks_membership() {
        let tree = sample();
        assert!(tree.has_pane("pane-chart"));
        assert!(!tree.has_pane("pane-missing"));
    }
}
