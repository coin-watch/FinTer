use crate::data::client::ApiClient;
use crate::data::error::Result;
use crate::data::models::Module;

use serde::{Deserialize, Serialize};

/// Data associated with a pane in the grid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneData {
    /// Display name for the pane.
    pub name: String,
    /// Module type (e.g., "Tape", "Chart", "OrderBook", "Empty").
    pub module_type: String,
    /// Exchange ID (e.g., "binance", "coinbase").
    pub exchange: String,
    /// Symbol ID (e.g., "BTC/USDT").
    pub symbol: String,
    /// Timeframe (e.g., "1m", "5m", "1h").
    pub timeframe: String,
}

/// Default module name for new users
pub const DEFAULT_MODULE_NAME: &str = "Default Layout";

/// Starter workspace, used only when creating a default layout, never on reload.
pub fn default_workspace() -> crate::data::pane_tree::PaneNode {
    use crate::data::pane_tree::{Axis, ModuleData, PaneNode};
    let pane = |kind: &str, config| PaneNode::Pane {
        id: PaneNode::generate_id(),
        data: ModuleData { module_type: Some(kind.into()), config },
    };
    PaneNode::Split {
        id: PaneNode::generate_id(), axis: Axis::Vertical, ratio: 0.7,
        a: Box::new(pane("chart", serde_json::json!({
            "exchange": "binance", "symbol": "BTC/USDT", "interval": "1h", "type": "candle_solid"
        }))),
        b: Box::new(PaneNode::Split {
            id: PaneNode::generate_id(), axis: Axis::Horizontal, ratio: 0.5,
            a: Box::new(pane("trades", serde_json::json!({
                "exchange": "binance", "symbol": "BTC/USDT"
            }))),
            b: Box::new(pane("watchlist", serde_json::json!({
                "exchange": "binance", "symbol": "BTC/USDT"
            }))),
        }),
    }
}

pub fn create_default_module_json() -> serde_json::Value {
    serde_json::to_value(default_workspace()).expect("starter workspace serializes")
}

/// Create a simple single-pane layout JSON with custom settings.
/// The format matches Configuration<PaneData>.
pub fn create_single_pane_module_json(
    name: &str,
    module_type: &str,
    exchange: &str,
    symbol: &str,
    timeframe: &str,
) -> serde_json::Value {
    serde_json::json!({
        "Pane": {
            "name": name,
            "module_type": module_type,
            "exchange": exchange,
            "symbol": symbol,
            "timeframe": timeframe
        }
    })
}

impl ApiClient {
    /// List all dashboards for the current user.
    ///
    /// Dashboards are stored as files on disk (`data::dashboard_files`), not in
    /// the `modules` table. This method is retained so existing call sites keep
    /// working; it no longer performs a network request and does not require
    /// authentication, which is what makes local-first guest mode work.
    ///
    /// Blocking — run from a background thread.
    pub fn list_modules(&self) -> Result<Vec<Module>> {
        crate::data::dashboard_files::list_modules()
    }

    /// Create a new dashboard.
    ///
    /// Writes a file to the dashboards directory. Blocking — run from a
    /// background thread.
    pub fn create_module(&self, module_name: &str, module: serde_json::Value) -> Result<Module> {
        let dashboard = crate::data::dashboard_files::create(module_name, module)?;
        Ok(dashboard.to_module())
    }

    /// Update an existing dashboard's name and/or tree.
    ///
    /// Blocking — run from a background thread.
    pub fn update_module(
        &self,
        module_id: &str,
        module_name: Option<&str>,
        module: Option<serde_json::Value>,
    ) -> Result<Module> {
        let dashboard = crate::data::dashboard_files::update(module_id, module_name, module)?;
        Ok(dashboard.to_module())
    }

    /// Delete a dashboard.
    ///
    /// Blocking — run from a background thread.
    pub fn delete_module(&self, module_id: &str) -> Result<()> {
        crate::data::dashboard_files::delete(module_id)
    }

    /// Create a default module for new users.
    /// This creates a simple "Default Layout" with one chart panel.
    ///
    /// Blocking — run from a background thread.
    pub fn create_default_module(&self) -> Result<Module> {
        self.create_module(DEFAULT_MODULE_NAME, create_default_module_json())
    }

    /// Create a new dashboard with a single unconfigured pane.
    /// The module payload uses the PaneNode format consumed by the dashboard renderer.
    ///
    /// Blocking — run from a background thread.
    pub fn create_empty_layout(&self, name: &str) -> Result<Module> {
        let tree = crate::data::pane_tree::PaneNode::Pane {
            id: crate::data::pane_tree::PaneNode::generate_id(),
            data: crate::data::pane_tree::ModuleData {
                module_type: Some("Empty".into()),
                config: serde_json::json!({}),
            },
        };
        let val = serde_json::to_value(tree)
            .map_err(|e| crate::data::error::CoinWatchError::Deserialization(e.to_string()))?;
        self.create_module(name, val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::models::{CreateModuleRequest, UpdateModuleRequest};
    use crate::data::pane_tree::PaneNode;

    #[test]
    fn test_create_module_request_serialization() {
        let req = CreateModuleRequest {
            user_id: "user-123".into(),
            module_name: "default".into(),
            module: serde_json::json!({"id": "root", "children": []}),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["module_name"], "default");
        assert_eq!(json["module"]["id"], "root");
    }

    #[test]
    fn test_update_module_request_skips_none() {
        let req = UpdateModuleRequest {
            module_name: None,
            module: Some(serde_json::json!({"updated": true})),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(!json.as_object().unwrap().contains_key("module_name"));
        assert_eq!(json["module"]["updated"], true);
    }

    #[test]
    fn test_default_module_json_structure() {
        let json = create_default_module_json();
        let tree: PaneNode = serde_json::from_value(json).expect("starter workspace should parse");
        assert!(matches!(tree, PaneNode::Split { .. }));
        let serialized = serde_json::to_value(tree).unwrap();
        let text = serialized.to_string();
        assert!(text.contains("\"chart\""));
        assert!(text.contains("\"trades\""));
        assert!(text.contains("\"watchlist\""));
        assert!(text.contains("\"1h\""));
        assert!(text.contains("BTC/USDT"));
    }

    #[test]
    fn test_single_pane_module_json_with_config() {
        let json = create_single_pane_module_json("Chart", "Chart", "coinbase", "ETH/USD", "1m");
        assert!(json.get("Pane").is_some());
        let content = &json["Pane"];
        assert_eq!(content["name"], "Chart");
        assert_eq!(content["module_type"], "Chart");
        assert_eq!(content["exchange"], "coinbase");
        assert_eq!(content["symbol"], "ETH/USD");
        assert_eq!(content["timeframe"], "1m");
    }

    #[test]
    fn test_single_pane_module_json_empty() {
        let json = create_single_pane_module_json("Empty", "Empty", "", "", "");
        assert!(json.get("Pane").is_some());
        let content = &json["Pane"];
        assert_eq!(content["name"], "Empty");
        assert_eq!(content["module_type"], "Empty");
        assert_eq!(content["exchange"], "");
    }

    #[test]
    fn test_default_module_json_parses_as_pane_tree() {
        let json = create_default_module_json();
        let tree: PaneNode = serde_json::from_value(json)
            .expect("Default module JSON should parse as PaneNode");
        assert_eq!(tree.collect_exchanges(), vec!["binance"]);
    }
}
