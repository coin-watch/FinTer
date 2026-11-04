use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::{SettingsData, UpsertSettingsRequest, UserSettings};

impl ApiClient {
    /// Get user settings.
    /// GET /api/settings?user_id=... (Bearer auth required)
    /// Returns None if no settings exist yet.
    ///
    /// Blocking — run from a background thread.
    pub fn get_settings(&self) -> Result<Option<UserSettings>> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_get(&format!("/api/settings?user_id={}", user_id));

        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();

            // Server returns `null` when no settings exist
            if body.trim() == "null" || body.trim().is_empty() {
                return Ok(None);
            }

            serde_json::from_str(&body).map_err(|e| {
                if status >= 400 {
                    crate::data::error::CoinWatchError::Api {
                        status,
                        message: body,
                    }
                } else {
                    crate::data::error::CoinWatchError::Deserialization(e.to_string())
                }
            })
        })
    }

    /// Create or update user settings (upsert).
    /// POST /api/settings (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn upsert_settings(&self, settings: SettingsData) -> Result<UserSettings> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self
            .auth_post("/api/settings")
            .json(&UpsertSettingsRequest { user_id, settings });

        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::data::models::{SettingsData, UpsertSettingsRequest};

    #[test]
    fn test_settings_data_default() {
        let settings = SettingsData::default();
        assert!(settings.current_dashboard.is_none());
        assert!(settings.dark_mode.is_none());
    }

    #[test]
    fn test_upsert_settings_serialization() {
        let req = UpsertSettingsRequest {
            user_id: "user-123".into(),
            settings: SettingsData {
                current_dashboard: Some("default".into()),
                dark_mode: Some("dark".into()),
                theme: Some("gruvbox".into()),
                locale: Some("en_US".into()),
                timezone: Some("UTC".into()),
                auto_check_updates: Some(false),
                update_channel: Some("beta".into()),
                default_ticker_exchange: Some("binance".into()),
                link_dashboard_tabs: Some(true),
                survey_status: Some("".into()),
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["settings"]["currentDashboard"], "default");
        assert_eq!(json["settings"]["darkMode"], "dark");
        assert_eq!(json["settings"]["theme"], "gruvbox");
        assert_eq!(json["settings"]["defaultTickerExchange"], "binance");
        assert_eq!(json["settings"]["linkDashboardTabs"], true);
        assert_eq!(json["settings"]["updateChannel"], "beta");
    }
}
