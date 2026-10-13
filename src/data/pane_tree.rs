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
}
