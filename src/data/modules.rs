use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::{CreateModuleRequest, Module, UpdateModuleRequest};

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

/// Create a default single empty pane layout JSON for new users.
/// This creates a simple layout with one empty panel that the user can configure.
///
/// The format matches Configuration<PaneData> which serializes as:
/// `{ "Pane": { "name": "...", "module_type": "...", ... } }`
pub fn create_default_module_json() -> serde_json::Value {
    serde_json::json!({
        "Pane": {
            "name": "Empty",
            "module_type": "Empty",
            "exchange": "",
            "symbol": "",
            "timeframe": ""
        }
    })
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
    /// List all modules (workspace layouts) for the current user.
    /// GET /api/modules?user_id=... (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn list_modules(&self) -> Result<Vec<Module>> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_get(&format!("/api/modules?user_id={}", user_id));

        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status();
            // Server returns raw array (not wrapped in { success, data })
            let body = resp.text().await?;

            if !status.is_success() {
                return Err(crate::data::error::CoinWatchError::Api {
                    status: status.as_u16(),
                    message: body,
                });
            }

            // Empty body means no modules yet — treat as empty list
            let trimmed = body.trim();
            if trimmed.is_empty() {
                return Ok(vec![]);
            }

            serde_json::from_str(trimmed).map_err(|e| {
                tracing::error!(
                    "✗ list_modules: deserialization failed. status={}, body={:?}, err={}",
                    status,
                    &trimmed[..trimmed.len().min(200)],
                    e
                );
                crate::data::error::CoinWatchError::Deserialization(e.to_string())
            })
        })
    }

    /// Create a new module (workspace layout).
    /// POST /api/modules (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn create_module(&self, module_name: &str, module: serde_json::Value) -> Result<Module> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_post("/api/modules").json(&CreateModuleRequest {
            user_id,
            module_name: module_name.to_string(),
            module,
        });

        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Update an existing module.
    /// PATCH /api/modules/{id} (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn update_module(
        &self,
        module_id: &str,
        module_name: Option<&str>,
        module: Option<serde_json::Value>,
    ) -> Result<Module> {
        tracing::info!(
            "→ API: update_module called for ID: {}, has_name: {}, has_module: {}",
            module_id,
            module_name.is_some(),
            module.is_some()
        );

        if let Some(ref json) = module {
            let json_str = serde_json::to_string(json).unwrap_or_default();
            tracing::info!("  Module JSON size: {} bytes", json_str.len());
            tracing::debug!(
                "  Module JSON preview: {}",
                &json_str[..json_str.len().min(200)]
            );
        }

        let path = format!("/api/modules/{}", module_id);
        let rt = tokio_runtime();
        let req = self.auth_patch(&path).json(&UpdateModuleRequest {
            module_name: module_name.map(|s| s.to_string()),
            module,
        });

        rt.block_on(async {
            tracing::info!("  Sending PATCH request to: {}", path);
            let resp = req.send().await?;
            let status = resp.status();
            tracing::info!("  Response status: {}", status);

            let result: Result<Module> = parse_api_response(resp).await;
            match &result {
                Ok(module) => {
                    tracing::info!("✓ API: Module updated successfully: {}", module.module_name);
                }
                Err(e) => {
                    tracing::error!("✗ API: Module update failed: {:?}", e);
                }
            }
            result
        })
    }

    /// Delete a module.
    /// DELETE /api/modules/{id} (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn delete_module(&self, module_id: &str) -> Result<()> {
        // Coinwatch API is the only persistence path.
        let path = format!("/api/modules/{}", module_id);
        let rt = tokio_runtime();
        let req = self.auth_delete(&path);

        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                Ok(())
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(crate::data::error::CoinWatchError::Api {
                    status,
                    message: body,
                })
            }
        })
    }

    /// Create a default module for new users.
    /// This creates a simple "Default Layout" with one chart panel.
    ///
    /// Blocking — run from a background thread.
    pub fn create_default_module(&self) -> Result<Module> {
        self.create_module(DEFAULT_MODULE_NAME, create_default_module_json())
    }

    /// Create a new empty layout module with a given name.
    /// Uses the default single-pane chart configuration.
    ///
    /// Blocking — run from a background thread.
    pub fn create_empty_layout(&self, name: &str) -> Result<Module> {
        self.create_module(name, create_default_module_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::models::{CreateModuleRequest, UpdateModuleRequest};
    use crate::data::modules::PaneData;
    use gpui_component::pane_grid::Configuration;

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
        // Should be Configuration::Pane format with "Pane" key
        assert!(json.get("Pane").is_some());
        let content = &json["Pane"];
        assert_eq!(content["name"], "Empty");
        assert_eq!(content["module_type"], "Empty");
        // Empty pane has empty strings for exchange/symbol/timeframe
        assert_eq!(content["exchange"], "");
        assert_eq!(content["symbol"], "");
        assert_eq!(content["timeframe"], "");
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
    fn test_default_module_json_parses_as_configuration() {
        // Verify the JSON can be parsed as Configuration<PaneData>
        let json = create_default_module_json();
        let config: Configuration<PaneData> = serde_json::from_value(json)
            .expect("Default module JSON should parse as Configuration<PaneData>");

        match config {
            Configuration::Pane(content) => {
                assert_eq!(content.name, "Empty");
                assert_eq!(content.module_type, "Empty");
            }
            _ => panic!("Expected Configuration::Pane variant"),
        }
    }
}
