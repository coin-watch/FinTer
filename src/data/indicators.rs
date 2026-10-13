use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::{CreateIndicatorRequest, Indicator};

impl ApiClient {
    /// List all indicators for the current user.
    /// GET /api/indicators?user_id=... (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn list_indicators(&self) -> Result<Vec<Indicator>> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_get(&format!("/api/indicators?user_id={}", user_id));

        rt.block_on(async {
            let resp = req.send().await?;
            // Server returns raw array
            let body = resp.text().await?;
            serde_json::from_str(&body)
                .map_err(|e| crate::data::error::CoinWatchError::Deserialization(e.to_string()))
        })
    }

    /// Create a new indicator.
    /// POST /api/indicators (Bearer auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn create_indicator(
        &self,
        indicator_name: &str,
        indicator: serde_json::Value,
        indicator_type: &str,
        public: bool,
    ) -> Result<Indicator> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self
            .auth_post("/api/indicators")
            .json(&CreateIndicatorRequest {
                user_id,
                indicator_name: indicator_name.to_string(),
                indicator,
                indicator_type: indicator_type.to_string(),
                public,
            });

        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::data::models::Indicator;

    #[test]
    fn test_indicator_deserialization() {
        let json = r#"{
            "id": "ind-1",
            "user_id": "user-1",
            "indicator_name": "RSI",
            "indicator": {"period": 14},
            "type": "oscillator",
            "public": false
        }"#;
        let indicator: Indicator = serde_json::from_str(json).unwrap();
        assert_eq!(indicator.indicator_name, "RSI");
        assert_eq!(indicator.indicator_type, Some("oscillator".to_string()));
    }
}
