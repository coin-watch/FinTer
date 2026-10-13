use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::{CreateLayoutPresetRequest, LayoutPreset};

impl ApiClient {
    /// List all layout presets for the current user.
    /// GET /api/layout-presets?user_id=... (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn list_layout_presets(&self) -> Result<Vec<LayoutPreset>> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_get(&format!("/api/layout-presets?user_id={}", user_id));

        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status();
            let body = resp.text().await?;

            if !status.is_success() {
                return Err(crate::data::error::CoinWatchError::Api {
                    status: status.as_u16(),
                    message: body,
                });
            }

            let trimmed = body.trim();
            if trimmed.is_empty() {
                return Ok(vec![]);
            }

            serde_json::from_str(trimmed).map_err(|e| {
                tracing::error!(
                    "✗ list_layout_presets: deserialization failed. status={}, body={:?}, err={}",
                    status,
                    &trimmed[..trimmed.len().min(200)],
                    e
                );
                crate::data::error::CoinWatchError::Deserialization(e.to_string())
            })
        })
    }

    /// Create a new layout preset.
    /// POST /api/layout-presets (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn create_layout_preset(
        &self,
        name: &str,
        tree: serde_json::Value,
    ) -> Result<LayoutPreset> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self
            .auth_post("/api/layout-presets")
            .json(&CreateLayoutPresetRequest {
                user_id,
                name: name.to_string(),
                tree,
                is_public: false,
            });

        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Delete a layout preset.
    /// DELETE /api/layout-presets/{id} (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn delete_layout_preset(&self, preset_id: &str) -> Result<()> {
        let path = format!("/api/layout-presets/{}", preset_id);
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
}
