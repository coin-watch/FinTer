use std::sync::{Arc, OnceLock, RwLock};

use crate::data::error::{CoinWatchError, Result};
use crate::data::models::AuthSession;
use crate::data::session_store;
use webbrowser;

/// Default API endpoint for guest mode — unconfigured so the user can point the
/// app at their own backend. Authenticated sessions use [`DEFAULT_CLOUD_API_URL`]
/// instead, because a session can only have been issued by a real backend.
pub const DEFAULT_FINTER_API_URL: &str = "";

/// Hosted CoinWatch backend used by authenticated sessions when no explicit
/// endpoint has been configured. This is the development/staging origin used by
/// the native client; override it with `FINTER_API_URL` or Settings → Network.
pub const DEFAULT_CLOUD_API_URL: &str = "https://staging.coinwat.ch";

/// Origins that older builds used as the API base but that do not serve the
/// native API today. `https://coinwat.ch` is the public marketing site; API
/// paths there return an HTML page (often a Cloudflare challenge/403), which
/// surfaced as `expected value at line 1 column 1` when signing in. Treat them
/// as unconfigured so every platform resolves the same hosted API origin.
const LEGACY_API_HOSTS: &[&str] = &["coinwat.ch", "www.coinwat.ch"];

/// Normalize a configured API origin.
///
/// Returns `None` for empty, whitespace-only, relative, non-HTTP(S), or legacy
/// marketing origins so callers can fall back to the platform-independent
/// default. Trailing slashes are removed.
pub fn normalize_api_origin(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }

    let Ok(url) = url::Url::parse(trimmed) else {
        return None;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    let host = url.host_str()?.to_ascii_lowercase();
    if LEGACY_API_HOSTS.contains(&host.as_str()) {
        return None;
    }

    Some(trimmed.to_string())
}

/// Base URL for the FinTer backend API.
///
/// Discovered in priority order:
/// 1. Runtime active base URL (if set via `set_base_url`).
/// 2. `FINTER_API_URL` environment variable.
/// 3. `server_url` field stored in `cloud_settings.json` or `guest_settings.json`.
/// 4. Default endpoint (empty for guests, the hosted cloud origin for sessions).
static ACTIVE_BASE_URL: RwLock<Option<String>> = RwLock::new(None);

/// Default API origin for the current session.
///
/// Guests stay unconfigured so the app remains local-first. A persisted session
/// means the user signed in against a real backend, so fall back to the hosted
/// cloud origin instead of sending requests to a relative URL.
pub fn default_api_url() -> String {
    if session_store::load_session().ok().flatten().is_some() {
        DEFAULT_CLOUD_API_URL.to_string()
    } else {
        DEFAULT_FINTER_API_URL.to_string()
    }
}

fn get_base_url() -> String {
    if let Ok(guard) = ACTIVE_BASE_URL.read() {
        if let Some(origin) = guard.as_deref().and_then(normalize_api_origin) {
            return origin;
        }
    }

    if let Ok(url) = std::env::var("FINTER_API_URL") {
        if let Some(origin) = normalize_api_origin(&url) {
            return origin;
        }
    }

    if let Some(base_dir) = dirs::config_dir() {
        for filename in &["cloud_settings.json", "guest_settings.json", "local_settings.json"] {
            let path = base_dir.join("finter").join(filename);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(settings) = serde_json::from_str::<crate::data::models::SettingsData>(&content) {
                    if let Some(origin) = settings
                        .server_url
                        .as_deref()
                        .and_then(normalize_api_origin)
                    {
                        return origin;
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
    if let Ok(url) = std::env::var("FINTER_REST_URL") {
        if let Some(origin) = normalize_api_origin(&url) {
            return origin;
        }
    }
    get_base_url()
}

/// Check if an API service URL has been configured either via environment variable,
/// settings file, or runtime base URL.
pub fn is_api_url_configured() -> bool {
    !service_base_url().trim().is_empty()
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn unconfigured_guest_endpoint_is_empty() {
        assert_eq!(DEFAULT_FINTER_API_URL, "");
    }

    #[test]
    fn cloud_default_is_an_absolute_https_origin() {
        assert!(DEFAULT_CLOUD_API_URL.starts_with("https://"));
        assert!(!DEFAULT_CLOUD_API_URL.ends_with('/'));
    }

    #[test]
    fn legacy_marketing_origin_is_not_an_api_base() {
        // Older builds persisted this as `serverUrl`; it returns HTML for API
        // paths, which used to surface as a JSON deserialization error.
        assert_eq!(normalize_api_origin("https://coinwat.ch"), None);
        assert_eq!(normalize_api_origin("https://coinwat.ch/"), None);
        assert_eq!(normalize_api_origin("http://www.coinwat.ch"), None);
        assert_eq!(normalize_api_origin("  "), None);
        assert_eq!(normalize_api_origin("staging.coinwat.ch"), None);
        assert_eq!(normalize_api_origin("ftp://staging.coinwat.ch"), None);
    }

    #[test]
    fn valid_api_origins_are_normalized() {
        assert_eq!(
            normalize_api_origin("https://staging.coinwat.ch/"),
            Some("https://staging.coinwat.ch".into())
        );
        assert_eq!(
            normalize_api_origin("  http://localhost:5173/  "),
            Some("http://localhost:5173".into())
        );
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
    pub(crate) base_url: Arc<RwLock<String>>,
    pub(crate) session: Arc<RwLock<Option<AuthSession>>>,
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiClient {
    /// Create a new API client using the optional `FINTER_API_URL`.
    /// Guests are unconfigured; a restored session falls back to the hosted
    /// cloud origin so account and dashboard requests have a valid base URL.
    pub fn new() -> Self {
        let mut base = get_base_url();
        if base.trim().is_empty() {
            base = default_api_url();
        }

        let client = Self::with_base_url(&base);
        if !base.trim().is_empty() {
            // Publish the resolved origin so `service_base_url()` (market data,
            // gateway registry, bridge) uses the same backend as the API client.
            client.set_base_url(&base);
        }
        client
    }

    /// Create a new API client with a custom base URL (useful for tests).
    pub fn with_base_url(base_url: &str) -> Self {
        let initial_session = session_store::load_session().ok().flatten();
        Self::with_base_url_and_session(base_url, initial_session)
    }

    /// Create a client with an explicit session state. Tests use this to avoid
    /// depending on whatever session happens to be in the developer's keyring.
    fn with_base_url_and_session(base_url: &str, initial_session: Option<AuthSession>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: Arc::new(RwLock::new(base_url.trim_end_matches('/').to_string())),
            session: Arc::new(RwLock::new(initial_session)),
        }
    }

    /// Get the current base URL.
    pub fn base_url(&self) -> String {
        self.base_url.read().unwrap().clone()
    }

    /// Set a new base URL at runtime.
    ///
    /// Empty, relative, and legacy marketing origins are ignored so a stale
    /// `serverUrl` from an older build can never replace a working endpoint.
    pub fn set_base_url(&self, base_url: &str) {
        let Some(clean) = normalize_api_origin(base_url) else {
            tracing::warn!(
                "Ignoring invalid or legacy API origin {:?}; keeping {}",
                base_url,
                self.base_url()
            );
            return;
        };
        if let Ok(mut guard) = self.base_url.write() {
            *guard = clean.clone();
        }
        if let Ok(mut active) = ACTIVE_BASE_URL.write() {
            *active = Some(clean);
        }
    }

    /// Full URL for a given API path (e.g. `/api/exchanges`).
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }

    /// Open the signup page in the browser.
    pub fn open_signup_page(&self) -> Result<()> {
        let signup_url = format!("{}/signup", self.base_url());
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
        self.clear_session_in_memory();
        if let Err(e) = session_store::delete_session() {
            tracing::warn!("failed to delete persisted session: {}", e);
        }
    }

    /// Drop the in-memory session without touching the OS keyring or the
    /// fallback session file. Used by lifecycle tests so they never delete a
    /// developer's real saved credentials.
    fn clear_session_in_memory(&self) {
        let mut guard = self.session.write().expect("session lock poisoned");
        *guard = None;
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
    /// Fetch the redacted account snapshot used by the native Profile page.
    pub fn get_account_summary(&self) -> Result<serde_json::Value> {
        let rt = tokio_runtime();
        let response = rt.block_on(self.auth_get("/api/account-summary").send())?;
        let status = response.status();
        let body = rt.block_on(response.text())?;
        if !status.is_success() {
            let status_code = status.as_u16();
            if status_code == 402 || body.contains("insufficient_credits") {
                let base = self.base_url();
                crate::data::credits::CreditStore::record(
                    "Account",
                    "Credit balance exhausted. Add credits or upgrade your plan to resume market data.",
                    0,
                    1,
                    Some(&base),
                );
            }

            // Fallback for servers without /api/account-summary
            if status == reqwest::StatusCode::NOT_FOUND {
                if let Ok(Some(sub)) = self.get_subscription() {
                    let user_email = self.session.read().ok().and_then(|s| s.as_ref().map(|x| x.email.clone()));
                    let summary = serde_json::json!({
                        "user": {
                            "id": sub.user_id,
                            "email": user_email,
                        },
                        "subscription": {
                            "id": sub.id,
                            "status": sub.status,
                        },
                        "credits": {
                            "balance": 0,
                            "monthly_free_remaining": 0,
                        }
                    });
                    return Ok(summary);
                }
            }

            let detail = if body.trim_start().starts_with('<') {
                format!("Endpoint {} not supported on server ({status})", self.url("/api/account-summary"))
            } else {
                serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|value| value["error"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| body.chars().take(240).collect())
            };
            return Err(CoinWatchError::Auth(format!("Account summary request failed ({status}): {detail}")));
        }

        let value = serde_json::from_str::<serde_json::Value>(&body)
            .map_err(|error| CoinWatchError::Deserialization(error.to_string()))?;

        // Synchronize CreditStore with the credits info in account summary
        if let Some(credits) = value.get("credits") {
            let balance = credits.get("balance")
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            let free_remaining = credits.get("monthly_free_remaining")
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            let base = self.base_url();
            if balance <= 0 && free_remaining <= 0 {
                crate::data::credits::CreditStore::record(
                    "Account",
                    "Credit balance exhausted. Add credits or upgrade your plan to resume market data.",
                    balance,
                    1,
                    Some(&base),
                );
            } else if balance > 0 || free_remaining > 0 {
                crate::data::credits::CreditStore::clear();
            }
        }

        Ok(value)
    }

    pub fn auth_get(&self, path: &str) -> reqwest::RequestBuilder {
        let base = self.base_url();
        let mut req = self
            .http
            .get(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", base.clone())
            .header("Referer", format!("{}/", base));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a POST request with Bearer auth (if logged in).
    pub fn auth_post(&self, path: &str) -> reqwest::RequestBuilder {
        let base = self.base_url();
        let mut req = self
            .http
            .post(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", base.clone())
            .header("Referer", format!("{}/", base));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a PATCH request with Bearer auth (if logged in).
    pub fn auth_patch(&self, path: &str) -> reqwest::RequestBuilder {
        let base = self.base_url();
        let mut req = self
            .http
            .patch(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", base.clone())
            .header("Referer", format!("{}/", base));
        if let Some(token) = self.access_token().or_else(|| {
            session_store::load_session().ok().flatten().map(|s| s.access_token)
        }) {
            req = req.bearer_auth(token);
        }
        req
    }

    /// Build a DELETE request with Bearer auth (if logged in).
    pub fn auth_delete(&self, path: &str) -> reqwest::RequestBuilder {
        let base = self.base_url();
        let mut req = self
            .http
            .delete(self.url(path))
            .header("Accept", "application/json")
            .header("Origin", base.clone())
            .header("Referer", format!("{}/", base));
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

    // Check for 402 or credit exhaustion in error responses
    if status == 402 || body.contains("insufficient_credits") || body.contains("Not enough credits") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let message = value.get("message").and_then(|v| v.as_str())
                .or_else(|| value.get("error").and_then(|v| v.as_str()))
                .unwrap_or("Not enough credits")
                .to_string();
            let balance = value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let required = value.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
            let base = get_base_url();
            crate::data::credits::CreditStore::record(
                "API",
                &message,
                balance,
                required,
                Some(&base),
            );
        }
    }

    // Try to parse as standard API envelope
    let envelope: std::result::Result<crate::data::models::ApiResponse<T>, _> =
        serde_json::from_str(&body);

    if status == 402 {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("Not enough credits").to_string();
            let balance = value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let required = value.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
            crate::data::credits::CreditStore::record(
                "API",
                &message,
                balance,
                required,
                None,
            );
            return Err(CoinWatchError::InsufficientCredits {
                message,
                balance,
                required,
            });
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
        let client = ApiClient::with_base_url_and_session("https://example.com", None);

        // Initially no session
        assert!(client.get_session().is_none());
        assert!(client.access_token().is_none());

        // Set session in memory only; never persist test credentials over a
        // developer's real saved session.
        let session = AuthSession {
            user_id: "user-123".into(),
            email: "test@example.com".into(),
            access_token: "tok-abc".into(),
            refresh_token: "ref-xyz".into(),
            expires_at: Some(9999999999),
            name: None,
            avatar_url: None,
        };
        client.set_session_raw(session);

        assert_eq!(client.access_token(), Some("tok-abc".into()));
        assert_eq!(client.user_id(), Some("user-123".into()));

        // Clear session
        client.clear_session_in_memory();
        assert!(client.get_session().is_none());
    }

    #[test]
    fn test_runtime_base_url_shared_across_clones() {
        let client1 = ApiClient::with_base_url("https://initial.example.com");
        let client2 = client1.clone();

        assert_eq!(client1.base_url(), "https://initial.example.com");
        assert_eq!(client2.base_url(), "https://initial.example.com");

        client1.set_base_url("https://staging.coinwat.ch");
        assert_eq!(client2.base_url(), "https://staging.coinwat.ch");
        assert_eq!(
            client2.url("/api/account-summary"),
            "https://staging.coinwat.ch/api/account-summary"
        );
        assert_eq!(service_base_url(), "https://staging.coinwat.ch");

        // A stale legacy origin must not replace the working endpoint.
        client1.set_base_url("https://coinwat.ch");
        assert_eq!(client2.base_url(), "https://staging.coinwat.ch");
    }
}
