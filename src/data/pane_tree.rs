use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModuleData {
    #[serde(rename = "type")]
    pub module_type: Option<String>,
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum PaneNode {
    #[serde(rename = "pane")]
    Pane { id: String, data: ModuleData },
    #[serde(rename = "split")]
    Split {
        id: String,
        axis: Axis,
        ratio: f32,
        a: Box<PaneNode>,
        b: Box<PaneNode>,
    },
}

impl PaneNode {
    pub fn generate_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// Recursively collect all configured exchanges from all panes in this tree.
    pub fn collect_exchanges(&self) -> Vec<String> {
        let mut set = std::collections::HashSet::new();
        self.collect_exchanges_into(&mut set);
        let mut list: Vec<_> = set.into_iter().collect();
        list.sort();
        list
    }

    fn collect_exchanges_into(&self, set: &mut std::collections::HashSet<String>) {
        match self {
            PaneNode::Pane { data, .. } => {
                if let Some(exchange) = data.config.get("exchange").and_then(|v| v.as_str()) {
                    let trimmed = exchange.trim().to_lowercase();
                    if !trimmed.is_empty() {
                        set.insert(trimmed);
                    }
                }
            }
            PaneNode::Split { a, b, .. } => {
                a.collect_exchanges_into(set);
                b.collect_exchanges_into(set);
            }
        }
    }
}
