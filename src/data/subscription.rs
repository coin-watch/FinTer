use crate::data::client::{ApiClient, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::Subscription;

impl ApiClient {
    /// Get subscription status for the current user.
    /// GET /api/subscription?user_id=... (Bearer auth required)
    /// Returns None if no subscription exists.
    ///
    /// Blocking — run from a background thread.
    pub fn get_subscription(&self) -> Result<Option<Subscription>> {
        let user_id = self
            .user_id()
            .ok_or_else(|| crate::data::error::CoinWatchError::Auth("Not logged in".into()))?;

        let rt = tokio_runtime();
        let req = self.auth_get(&format!("/api/subscription?user_id={}", user_id));

        rt.block_on(async {
            let resp = req.send().await?;
            let body = resp.text().await.unwrap_or_default();

            if body.trim() == "null" || body.trim().is_empty() {
                return Ok(None);
            }

            serde_json::from_str(&body)
                .map_err(|e| crate::data::error::CoinWatchError::Deserialization(e.to_string()))
        })
    }
}
