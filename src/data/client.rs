use std::sync::{Arc, OnceLock, RwLock};

use crate::data::error::{CoinWatchError, Result};
use crate::data::models::AuthSession;
use crate::data::session_store;
use webbrowser;

/// Default hosted API endpoint
pub const DEFAULT_FINTER_API_URL: &str = "https://coinwat.ch";

/// Base URL for the FinTer backend API.
///
/// Discovered in priority order:
/// 1. `FINTER_API_URL` environment variable.
/// 2. `server_url` field stored in `~/.config/finter/guest_settings.json` or `cloud_settings.json`.
/// 3. Default hosted endpoint `https://coinwat.ch`.
fn get_base_url() -> String {
    if let Ok(url) = std::env::var("FINTER_API_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_end_matches('/').to_string();
        }
    }

    if let Some(base_dir) = dirs::config_dir() {
        for filename in &["guest_settings.json", "cloud_settings.json", "local_settings.json"] {
            let path = base_dir.join("finter").join(filename);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str::<crate::data::models::SettingsData>(&content) {
                    if let Some(url) = settings.server_url {
                        let trimmed = url.trim();
                        if !trimmed.is_empty() {
                            return trimmed.trim_end_matches('/').to_string();
                        }
                    }
                }
            }
        }
    }

    DEFAULT_FINTER_API_URL.to_string()
}

/// Use a separately configured REST gateway when `FINTER_REST_URL` is set;
/// otherwise use the configured API origin.
pub(crate) fn service_base_url() -> String {
    std::env::var("FINTER_REST_URL")
        .unwrap_or_else(|_| get_base_url())
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn unconfigured_default_endpoint_is_coinwatch() {
        assert_eq!(DEFAULT_FINTER_API_URL, "https://coinwat.ch");
    }
}

/// Global Tokio runtime for running reqwest HTTP calls.
/// GPUI has its own async executor that is NOT Tokio, so reqwest
/// (which depends on Tokio) needs its own runtime.
static TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to create Tokio runtime for HTTP")
    })
}

/// Central HTTP client shared across all API modules.
///
/// Holds a `reqwest::Client` (connection-pooled) and an optional
/// `AuthSession` behind an `Arc<RwLock>` so any module can read
/// the current token and the auth module can update it atomically.
///
/// All HTTP calls are dispatched onto a background Tokio runtime
/// because GPUI's async executor is not Tokio-compatible.
#[derive(Clone)]
pub struct ApiClient {
    pub(crate) http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) session: Arc<RwLock<Option<AuthSession>>>,
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiClient {
    /// Create a new API client using the optional `FINTER_API_URL`.
    /// The URL is empty when no compatible backend has been configured.
    pub fn new() -> Self {
        Self::with_base_url(&get_base_url())
    }

    /// Create a new API client with a custom base URL (useful for tests).
    pub fn with_base_url(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let initial_session = session_store::load_session().ok().flatten();

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            session: Arc::new(RwLock::new(initial_session)),
        }
    }

    /// Get the current base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Set a new base URL at runtime.
    pub fn set_base_url(&mut self, base_url: &str) {
        self.base_url = base_url.trim_end_matches('/').to_string();
    }

    /// Full URL for a given API path (e.g. `/api/exchanges`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Open the signup page in the browser.
    pub fn open_signup_page(&self) -> Result<()> {
        let signup_url = format!("{}/signup", self.base_url);
        webbrowser::open(&signup_url)
            .map_err(|e| CoinWatchError::Auth(format!("Failed to open browser: {}", e)))
    }

    /// Reference to the underlying reqwest client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    // ── Session management (synchronous — uses std RwLock) ──────

    /// Store a new auth session (called after login/signup/refresh).
    /// Also persists to OS keyring / config-file so sessions survive app restarts.
    /// Returns an error if session persistence failed (session is still saved in memory).
    pub fn set_session(&self, session: AuthSession) -> Result<()> {
        let mut guard = self.session.write().expect("session lock poisoned");
        *guard = Some(session.clone());
        let email = session.email.clone();
        drop(guard); // release lock before I/O

        tracing::info!("Persisting session for user: {}...", email);
        session_store::save_session(&session).map_err(|e| {
            tracing::error!(
                "CRITICAL: failed to persist session for {}. User will need to re-login on next launch. Error: {}",
                email,
                e
            );
            CoinWatchError::Storage(e.to_string())
        })
    }

    /// Store a session in memory only — no persistence side-effect.
    /// Used internally by `restore_session` to load the stored token before
    /// calling refresh (which will then call `set_session` and persist the new token).
    pub(crate) fn set_session_raw(&self, session: AuthSession) {
        let mut guard = self.session.write().expect("session lock poisoned");
        *guard = Some(session);
    }

    /// Clear the current auth session (called on logout).
    /// Also removes the persisted session from storage.
    pub fn clear_session(&self) {
        let mut guard = self.session.write().expect("session lock poisoned");
        *guard = None;
        drop(guard); // release lock before I/O
        if let Err(e) = session_store::delete_session() {
            tracing::warn!("failed to delete persisted session: {}", e);
        }
    }

    /// Get a clone of the current session, if any.
    pub fn get_session(&self) -> Option<AuthSession> {
        let guard = self.session.read().expect("session lock poisoned");
        guard.clone()
    }

    /// Get the current access token, if logged in.
    pub fn access_token(&self) -> Option<String> {
        let guard = self.session.read().expect("session lock poisoned");
        guard.as_ref().map(|s| s.access_token.clone())
    }

    /// Get the current user ID, if logged in.
    pub fn user_id(&self) -> Option<String> {
        let guard = self.session.read().expect("session lock poisoned");
        guard.as_ref().map(|s| s.user_id.clone())
    }

    // ── Authenticated request helpers ───────────────

    /// Build a GET request with Bearer auth (if logged in).
    pub fn auth_get(&self, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .get(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", self.base_url.clone())
            .header("Referer", format!("{}/", self.base_url));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a POST request with Bearer auth (if logged in).
    pub fn auth_post(&self, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", self.base_url.clone())
            .header("Referer", format!("{}/", self.base_url));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a PATCH request with Bearer auth (if logged in).
    pub fn auth_patch(&self, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .patch(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", self.base_url.clone())
            .header("Referer", format!("{}/", self.base_url));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a DELETE request with Bearer auth (if logged in).
    pub fn auth_delete(&self, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .delete(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", self.base_url.clone())
            .header("Referer", format!("{}/", self.base_url));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }
}

/// Parse a standard `{ success, data, error }` response body.
/// Returns the `data` field on success, or a `CoinWatchError::Api` on failure.
pub async fn parse_api_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T> {
    let status = response.status().as_u16();
    let body = response.text().await.map_err(CoinWatchError::Network)?;

    // Try to parse as standard API envelope
    let envelope: std::result::Result<crate::data::models::ApiResponse<T>, _> =
        serde_json::from_str(&body);

    if status == 402 {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            if value.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
                return Err(CoinWatchError::InsufficientCredits {
                    message: value.get("message").and_then(|v| v.as_str()).unwrap_or("Not enough credits").to_string(),
                    balance: value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0),
                    required: value.get("required").and_then(|v| v.as_i64()).unwrap_or(0),
                });
            }
        }
    }

    match envelope {
        Ok(resp) => {
            if resp.success {
                resp.data.ok_or_else(|| CoinWatchError::Api {
                    status,
                    message: "Response success=true but data is null".into(),
                })
            } else {
                let msg = resp
                    .error
                    .or(resp.message)
                    .unwrap_or_else(|| "Unknown API error".into());
                Err(CoinWatchError::Api {
                    status,
                    message: msg,
                })
            }
        }
        Err(_) => {
            // If not a standard envelope, try to parse T directly
            // (some endpoints return raw arrays, e.g. GET /api/modules)
            serde_json::from_str::<T>(&body).map_err(|e| {
                if status >= 400 {
                    let msg = if body.trim_start().starts_with("<!doctype") || body.trim_start().starts_with("<html") {
                        format!("HTTP {status}")
                    } else {
                        body
                    };
                    CoinWatchError::Api {
                        status,
                        message: msg,
                    }
                } else {
                    CoinWatchError::Deserialization(format!("{}: {}", e, body))
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_construction() {
        let client = ApiClient::with_base_url("https://example.com/");
        assert_eq!(
            client.url("/api/exchanges"),
            "https://example.com/api/exchanges"
        );
    }

    #[test]
    fn test_url_no_trailing_slash() {
        let client = ApiClient::with_base_url("https://example.com");
        assert_eq!(
            client.url("/api/auth/login"),
            "https://example.com/api/auth/login"
        );
    }

    #[test]
    fn test_session_lifecycle() {
        let client = ApiClient::with_base_url("https://example.com");

        // Initially no session
        assert!(client.get_session().is_none());
        assert!(client.access_token().is_none());

        // Set session
        let session = AuthSession {
            user_id: "user-123".into(),
            email: "test@example.com".into(),
            access_token: "tok-abc".into(),
            refresh_token: "ref-xyz".into(),
            expires_at: Some(9999999999),
            name: None,
            avatar_url: None,
        };
        client
            .set_session(session.clone())
            .expect("set_session should succeed in test");

        assert_eq!(client.access_token(), Some("tok-abc".into()));
        assert_eq!(client.user_id(), Some("user-123".into()));

        // Clear session
        client.clear_session();
        assert!(client.get_session().is_none());
    }
}
