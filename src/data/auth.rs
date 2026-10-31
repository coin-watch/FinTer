use crate::data::client::{ApiClient, tokio_runtime};
use crate::data::error::{CoinWatchError, Result};
use crate::data::models::{AuthResponse, AuthSession, LoginRequest, RefreshRequest};
use crate::data::session_store;

impl ApiClient {
    /// Log in with email and password.
    /// On success, stores the session internally and returns it.
    ///
    /// This is a blocking call — run it from a background thread
    /// (e.g. inside GPUI's `cx.spawn`).
    pub fn login(&self, email: &str, password: &str) -> Result<AuthSession> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url("/api/auth/login");
        let body = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };

        // Try the coinwat.ch proxy first
        let proxy_result: std::result::Result<AuthResponse, _> = rt.block_on(async {
            let resp = http.post(&url)
                .header("Origin", self.base_url())
                .json(&body)
                .send().await?;
            let status = resp.status();
            let bytes = resp.bytes().await?;
            if status.is_server_error() {
                // 5xx — proxy is broken, signal to use direct path
                return Err(CoinWatchError::Api {
                    status: status.as_u16(),
                    message: "proxy unavailable".into(),
                });
            }
            serde_json::from_slice::<AuthResponse>(&bytes)
                .map_err(|e| CoinWatchError::Deserialization(e.to_string()))
        });

        let auth = proxy_result?;
        if auth.success {
            let session = AuthSession {
                user_id: auth.user_id.ok_or_else(|| {
                    CoinWatchError::Auth("Missing user_id in login response".into())
                })?,
                email: auth.email.ok_or_else(|| {
                    CoinWatchError::Auth("Missing email in login response".into())
                })?,
                access_token: auth.access_token.ok_or_else(|| {
                    CoinWatchError::Auth("Missing access_token in login response".into())
                })?,
                refresh_token: auth.refresh_token.ok_or_else(|| {
                    CoinWatchError::Auth("Missing refresh_token in login response".into())
                })?,
                expires_at: auth.expires_at,
                name: auth.name.filter(|s| !s.is_empty()),
                avatar_url: auth.avatar_url.filter(|s| !s.is_empty()),
            };
            let _ = self.set_session(session.clone()).map_err(|e| {
                tracing::warn!("session stored in memory but persistence failed: {}", e);
            });
            Ok(session)
        } else {
            let msg = auth.error.unwrap_or_else(|| "Login failed".into());
            Err(CoinWatchError::Auth(msg))
        }
    }

    /// Create a new account with email and password.
    /// Returns the session if auto-confirm is enabled, or an error if email verification is needed.
    ///
    /// Blocking — run from a background thread.
    pub fn signup(&self, email: &str, password: &str) -> Result<AuthSession> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url("/api/auth/signup");
        let body = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };

        let auth: AuthResponse = rt.block_on(async {
            let resp = http.post(&url)
                .header("Origin", self.base_url())
                .json(&body)
                .send().await?;
            resp.json::<AuthResponse>()
                .await
                .map_err(|e| CoinWatchError::Deserialization(e.to_string()))
        })?;

        if auth.success {
            if let (Some(user_id), Some(email), Some(access_token), Some(refresh_token)) = (
                auth.user_id.clone(),
                auth.email.clone(),
                auth.access_token.clone(),
                auth.refresh_token.clone(),
            ) {
                let session = AuthSession {
                    user_id,
                    email,
                    access_token,
                    refresh_token,
                    expires_at: auth.expires_at,
                    // Use name/avatar_url from API response if available
                    name: auth.name.filter(|s| !s.is_empty()),
                    avatar_url: auth.avatar_url.filter(|s| !s.is_empty()),
                };
                let _ = self.set_session(session.clone()).map_err(|e| {
                    tracing::warn!("session stored in memory but persistence failed: {}", e);
                });
                Ok(session)
            } else {
                // Email confirmation required
                Err(CoinWatchError::Auth(auth.message.unwrap_or_else(|| {
                    "Please check your email to confirm your account.".into()
                })))
            }
        } else {
            Err(CoinWatchError::Auth(
                auth.error.unwrap_or_else(|| "Signup failed".into()),
            ))
        }
    }

    /// Log out the current user. Clears the local session.
    ///
    /// Blocking — run from a background thread.
    pub fn logout(&self) -> Result<()> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url("/api/auth/logout");

        let _ = rt.block_on(async { 
            http.post(&url)
                .header("Origin", self.base_url())
                .send().await 
        });

        self.clear_session();
        Ok(())
    }

    /// Refresh the access token using the stored refresh token.
    /// Updates the session on success.
    ///
    /// Blocking — run from a background thread.
    pub fn refresh_token(&self) -> Result<AuthSession> {
        let current = self
            .get_session()
            .ok_or_else(|| CoinWatchError::Auth("No session to refresh".into()))?;

        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url("/api/auth/refresh");
        let body = RefreshRequest {
            refresh_token: current.refresh_token.clone(),
        };

        // Try the proxy first
        let proxy_result: std::result::Result<AuthResponse, _> = rt.block_on(async {
            let resp = http.post(&url)
                .header("Origin", self.base_url())
                .json(&body)
                .send().await?;
            let status = resp.status();
            let body_bytes = resp
                .bytes()
                .await
                .map_err(|e| CoinWatchError::Auth(e.to_string()))?;
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::BAD_REQUEST
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(CoinWatchError::TokenExpired);
            }
            if status.is_server_error() {
                tracing::warn!("Coinwatch refresh API returned {}", status);
                return Err(CoinWatchError::Api {
                    status: status.as_u16(),
                    message: "proxy unavailable".into(),
                });
            }
            if !status.is_success() {
                return Err(CoinWatchError::Api {
                    status: status.as_u16(),
                    message: String::from_utf8_lossy(&body_bytes).into_owned(),
                });
            }
            serde_json::from_slice::<AuthResponse>(&body_bytes)
                .map_err(|e| CoinWatchError::Deserialization(e.to_string()))
        });

        let auth = proxy_result?;
        if auth.success {
            let session = AuthSession {
                user_id: auth.user_id().unwrap_or(current.user_id),
                email: auth.email().unwrap_or(current.email),
                access_token: auth.access_token().ok_or_else(|| {
                    CoinWatchError::Auth("Missing access_token in refresh response".into())
                })?,
                refresh_token: auth.refresh_token().unwrap_or(current.refresh_token),
                expires_at: auth.expires_at().or(current.expires_at),
                name: auth.name().filter(|s| !s.is_empty()).or(current.name),
                avatar_url: auth
                    .avatar_url()
                    .filter(|s| !s.is_empty())
                    .or(current.avatar_url),
            };
            let _ = self.set_session(session.clone()).map_err(|e| {
                tracing::warn!("session stored in memory but persistence failed: {}", e);
            });
            Ok(session)
        } else {
            if auth.expired.unwrap_or(false) {
                self.clear_session();
                return Err(CoinWatchError::TokenExpired);
            }
            Err(CoinWatchError::Auth(
                auth.error.unwrap_or_else(|| "Token refresh failed".into()),
            ))
        }
    }

    /// Check whether we are currently authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.get_session().is_some()
    }

    /// Try to restore a previous session from persistent storage.
    ///
    /// Steps:
    /// 1. Load the stored `AuthSession` from keyring / file fallback.
    /// 2. Call `POST /api/auth/refresh` with the stored refresh token.
    /// 3. On success: store the new session and return `Some(session)`.
    /// 4. On failure (expired / network): clear storage and return `None`.
    ///
    /// Blocking — run from a background thread (e.g. inside GPUI's `cx.spawn`).
    pub fn restore_session(&self) -> Option<AuthSession> {
        // Step 1 — load stored session
        tracing::info!("Attempting to restore session from storage...");
        let stored = match session_store::load_session() {
            Ok(Some(s)) => {
                tracing::info!(
                    "Session loaded from storage for user: {}, user_id: {}",
                    s.email,
                    s.user_id
                );
                s
            }
            Ok(None) => {
                tracing::debug!("no stored session found in keyring or file");
                return None;
            }
            Err(e) => {
                tracing::error!("CRITICAL: failed to load stored session: {}", e);
                tracing::error!("Session persistence is broken! User will need to re-login.");
                return None;
            }
        };

        // Step 2 — pre-load it into memory immediately (so client can make calls)
        self.set_session_raw(stored.clone());

        // Check if token is still valid (give 5 mins buffer)
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        if let Some(exp) = stored.expires_at {
            if exp > now + 300 {
                tracing::info!(
                    "session token still valid (expires in {}s), skipping aggressive refresh",
                    exp - now
                );
                return Some(stored);
            }
        }

        // Step 3 — refresh if expired or missing expiry
        match self.refresh_token() {
            Ok(new_session) => {
                tracing::info!("session restored & refreshed for {}", new_session.email);
                Some(new_session)
            }
            Err(e) => {
                if let CoinWatchError::TokenExpired = e {
                    tracing::warn!("session expired or invalid ({}), clearing session", e);
                    self.clear_session();
                    None
                } else {
                    tracing::warn!(
                        "session refresh failed due to network/server ({}), keeping stored session",
                        e
                    );
                    Some(stored) // Keep session so offline users aren't logged out
                }
            }
        }
    }

    /// Login with Google OAuth via website.
    ///
    /// This method:
    /// 1. Starts a local HTTP server on a random port
    /// 2. Opens the system browser to the OAuth URL
    /// 3. Waits for the callback with session tokens
    /// 4. Stores the session and returns it
    ///
    /// Blocking — run from a background thread.
    pub fn login_with_google(&self) -> Result<AuthSession> {
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use url::Url;

        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| CoinWatchError::Auth(format!("Failed to bind local server: {}", e)))?;
        let port = listener
            .local_addr()
            .map_err(|e| CoinWatchError::Auth(format!("Failed to get local port: {}", e)))?
            .port();

        tracing::info!("OAuth callback server listening on port {}", port);

        // Build OAuth URL - use the url() method to construct the full URL
        let full_url = self.url("/auth/native");
        let mut oauth_url = Url::parse(&full_url)
            .map_err(|e| CoinWatchError::Auth(format!("Invalid URL: {}", e)))?;
        oauth_url
            .query_pairs_mut()
            .append_pair("port", &port.to_string());

        // Open browser
        let oauth_url_str = oauth_url.to_string();
        tracing::info!("Opening browser to: {}", oauth_url_str);
        webbrowser::open(&oauth_url_str)
            .map_err(|e| CoinWatchError::Auth(format!("Failed to open browser: {}", e)))?;

        // Wait for callback
        let session_result: Arc<Mutex<Option<Result<AuthSession>>>> = Arc::new(Mutex::new(None));
        let session_result_clone = session_result.clone();

        // Use tiny_http to handle the callback
        let server = tiny_http::Server::from_listener(listener, None)
            .map_err(|e| CoinWatchError::Auth(format!("Failed to create HTTP server: {}", e)))?;

        // Timeout after 5 minutes
        let timeout = std::time::Duration::from_secs(300);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if let Ok(Some(request)) = server.recv_timeout(std::time::Duration::from_secs(1)) {
                let url_str = format!("http://localhost{}", request.url());

                if let Ok(url) = Url::parse(&url_str)
                    && url.path() == "/callback"
                {
                    // Extract tokens from query params
                    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

                    let session = if let (
                        Some(access_token),
                        Some(refresh_token),
                        Some(user_id),
                        Some(email),
                    ) = (
                        params.get("access_token").map(|s| s.to_string()),
                        params.get("refresh_token").map(|s| s.to_string()),
                        params.get("user_id").map(|s| s.to_string()),
                        params.get("email").map(|s| s.to_string()),
                    ) {
                        let expires_at =
                            params.get("expires_at").and_then(|s| s.parse::<i64>().ok());
                        // Extract optional name and avatar_url from OAuth callback
                        let name = params
                            .get("name")
                            .map(|s| s.to_string())
                            .filter(|s| !s.is_empty());
                        let avatar_url = params
                            .get("avatar_url")
                            .map(|s| s.to_string())
                            .filter(|s| !s.is_empty());

                        tracing::info!(
                            "OAuth callback received - name: {:?}, avatar_url: {:?}",
                            name,
                            avatar_url
                        );

                        let session = AuthSession {
                            user_id,
                            email,
                            access_token,
                            refresh_token,
                            expires_at,
                            name,
                            avatar_url,
                        };

                        let _ = self.set_session(session.clone()).map_err(|e| {
                            tracing::warn!(
                                "session stored in memory but persistence failed: {}",
                                e
                            );
                        });
                        Ok(session)
                    } else {
                        Err(CoinWatchError::Auth(
                            "Missing required OAuth parameters".into(),
                        ))
                    };

                    // Send success response to browser
                    let response_body = r#"
                            <!DOCTYPE html>
                            <html>
                            <head>
                                <title>Login Successful</title>
                                <style>
                                    body { font-family: system-ui; text-align: center; padding: 50px; }
                                    h1 { color: #10b981; }
                                </style>
                            </head>
                            <body>
                                <h1>✓ Login Successful</h1>
                                <p>You can now close this window and return to CoinWatch.</p>
                                <script>window.close();</script>
                            </body>
                            </html>
                        "#;

                    let response = tiny_http::Response::from_string(response_body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    );
                    let _ = request.respond(response);

                    *session_result_clone.lock().unwrap() = Some(session);
                    break;
                }
            }

            // Check if we got a result
            if session_result.lock().unwrap().is_some() {
                break;
            }
        }

        // Extract result
        let result = session_result.lock().unwrap().take();
        result.unwrap_or_else(|| {
            Err(CoinWatchError::Auth(
                "OAuth timeout or user cancelled".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_serialization() {
        let req = LoginRequest {
            email: "test@example.com".into(),
            password: "secret123".into(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["email"], "test@example.com");
        assert_eq!(json["password"], "secret123");
    }

    #[test]
    fn test_refresh_request_serialization() {
        let req = RefreshRequest {
            refresh_token: "tok-123".into(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["refresh_token"], "tok-123");
    }
}
