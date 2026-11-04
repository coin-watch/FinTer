use crate::data::client::{ApiClient, tokio_runtime};
use crate::data::error::{CoinWatchError, Result};
use crate::data::models::{AuthResponse, AuthSession, LoginRequest, RefreshRequest};
use crate::data::session_store;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Escape text before interpolating it into the loopback callback page.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Public web origin used for links on the loopback callback page.
/// Falls back to the production site when the app has no server configured.
fn web_origin(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "https://coinwat.ch".to_string()
    } else {
        trimmed.to_string()
    }
}

fn callback_header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static callback header is valid")
}

/// CORS + Private Network Access headers let the coinwat.ch handoff page
/// deliver the session to this loopback listener from a normal HTTPS page.
fn callback_cors_headers() -> Vec<tiny_http::Header> {
    vec![
        callback_header("Access-Control-Allow-Origin", "*"),
        callback_header("Access-Control-Allow-Methods", "GET, OPTIONS"),
        callback_header("Access-Control-Allow-Headers", "*"),
        callback_header("Access-Control-Allow-Private-Network", "true"),
        callback_header("Access-Control-Max-Age", "600"),
    ]
}

fn with_callback_cors<R: std::io::Read>(
    response: tiny_http::Response<R>,
) -> tiny_http::Response<R> {
    callback_cors_headers()
        .into_iter()
        .fold(response, |response, header| response.with_header(header))
}

/// Branded fallback page shown if the browser navigates directly to the
/// loopback listener instead of using the coinwat.ch handoff page.
fn native_callback_page(web_origin: &str, success: bool, email: &str, message: &str) -> String {
    let host = url::Url::parse(web_origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "coinwat.ch".to_string());

    let (title, body, icon, accent, status) = if success {
        (
            "You're signed in",
            "FinTer is ready. You can close this tab and return to the app.",
            r#"<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>"#,
            "#039952",
            "HANDOFF COMPLETE",
        )
    } else {
        (
            "Sign-in didn't complete",
            message,
            r#"<svg width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 9v4"/><path d="M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z"/></svg>"#,
            "#f87171",
            "ACTION NEEDED",
        )
    };

    let email_row = if success && !email.is_empty() {
        format!(
            r#"<div class="email">{}<span>Signed in with Google</span></div>"#,
            escape_html(email)
        )
    } else {
        String::new()
    };

    r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<meta name="robots" content="noindex, nofollow">
<title>__TITLE__ — FinTer</title>
<style>
:root{color-scheme:dark}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;display:flex;flex-direction:column;background:#060908;color:#e8edea;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
.grid{position:fixed;inset:0;background-image:linear-gradient(to right,rgba(255,255,255,.035) 1px,transparent 1px),linear-gradient(to bottom,rgba(255,255,255,.035) 1px,transparent 1px);background-size:56px 56px;mask-image:radial-gradient(ellipse at center,black 20%,transparent 75%);pointer-events:none}
.glow{position:fixed;top:-18rem;left:50%;width:36rem;height:36rem;transform:translateX(-50%);border-radius:999px;background:__ACCENT__22;filter:blur(120px);pointer-events:none}
header{position:relative;z-index:1;display:flex;align-items:center;justify-content:space-between;gap:16px;padding:20px 24px}
.brand{display:flex;align-items:center;gap:12px;color:inherit;text-decoration:none}
.brand img{width:36px;height:36px;border-radius:10px;border:1px solid rgba(255,255,255,.1);background:#0c1410}
.brand strong{display:block;font-size:15px;letter-spacing:-.02em}
.brand small{display:block;margin-top:4px;font-size:9px;letter-spacing:.18em;text-transform:uppercase;color:#78857e}
.badge{border:1px solid rgba(255,255,255,.1);background:rgba(255,255,255,.03);border-radius:999px;padding:6px 12px;font-size:9px;letter-spacing:.16em;text-transform:uppercase;color:#9aa69f;white-space:nowrap}
main{position:relative;z-index:1;flex:1;display:flex;align-items:center;justify-content:center;padding:32px 16px}
.card{width:100%;max-width:520px;overflow:hidden;border:1px solid rgba(255,255,255,.1);border-radius:16px;background:rgba(10,15,12,.92);box-shadow:0 30px 80px -20px rgba(0,0,0,.9)}
.line{height:1px;background:linear-gradient(90deg,transparent,__ACCENT__99,transparent)}
.body{padding:32px}
.status{display:flex;align-items:flex-start;justify-content:space-between;gap:16px;margin-bottom:24px}
.icon{width:56px;height:56px;display:flex;align-items:center;justify-content:center;border-radius:16px;border:1px solid __ACCENT__55;background:__ACCENT__18;color:__ACCENT__}
h1{margin:0;font-size:28px;line-height:1.15;letter-spacing:-.03em}
p{margin:8px 0 0;font-size:12px;line-height:1.7;color:#8b9891}
.email{margin-top:24px;border:1px solid rgba(255,255,255,.1);background:rgba(255,255,255,.025);border-radius:12px;padding:12px;font-size:11px;color:#c7d2cc;overflow-wrap:anywhere}
.email span{display:block;margin-top:3px;font-size:10px;color:#78857e}
.actions{display:flex;gap:8px;margin-top:28px;flex-wrap:wrap}
a.button,button.button{flex:1;min-width:140px;display:inline-flex;align-items:center;justify-content:center;gap:8px;border-radius:8px;padding:11px 16px;font:inherit;font-size:11px;font-weight:600;text-decoration:none;cursor:pointer;border:1px solid transparent}
.primary{background:#039952;color:#fff}
.primary:hover{background:#04b862}
.secondary{border-color:rgba(255,255,255,.1);background:rgba(255,255,255,.03);color:#c7d2cc}
.secondary:hover{background:rgba(255,255,255,.06)}
.foot{border-top:1px solid rgba(255,255,255,.1);background:rgba(255,255,255,.02);padding:16px 32px;font-size:9px;color:#6f7c75;display:flex;justify-content:space-between;gap:8px;flex-wrap:wrap}
.foot strong{color:#a7b3ac;font-weight:500}
footer.page{position:relative;z-index:1;padding:0 24px 20px;text-align:center;font-size:9px;letter-spacing:.16em;text-transform:uppercase;color:#4f5a54}
@media(max-width:520px){.body{padding:24px}.foot{padding:16px 24px}h1{font-size:24px}}
</style>
</head>
<body>
<div class="grid"></div>
<div class="glow"></div>
<header>
<a class="brand" href="__ORIGIN__">
<img src="__ORIGIN__/finter-logo.svg" alt="" referrerpolicy="no-referrer">
<span><strong>FinTer</strong><small>CoinWatch terminal</small></span>
</a>
<span class="badge">__STATUS__</span>
</header>
<main>
<div class="card">
<div class="line"></div>
<div class="body">
<div class="status"><div class="icon">__ICON__</div></div>
<h1>__TITLE__</h1>
<p>__BODY__</p>
__EMAIL_ROW__
<div class="actions">
<a class="button primary" href="__ORIGIN__">Open __HOST__</a>
<button class="button secondary" type="button" onclick="window.close()">Close this tab</button>
</div>
</div>
<div class="foot"><span>Served by <strong>FinTer</strong> on this device</span><span>__HOST__</span></div>
</div>
</main>
<footer class="page">CoinWatch · FinTer native terminal</footer>
</body>
</html>"#
        .replace("__ORIGIN__", &escape_html(web_origin))
        .replace("__HOST__", &escape_html(&host))
        .replace("__TITLE__", &escape_html(title))
        .replace("__BODY__", &escape_html(body))
        .replace("__ICON__", icon)
        .replace("__ACCENT__", accent)
        .replace("__STATUS__", status)
        .replace("__EMAIL_ROW__", &email_row)
}

/// Decode an auth response body.
///
/// Some endpoints (notably the legacy `https://coinwat.ch` origin persisted by
/// older builds) return an HTML page instead of the JSON envelope. Reporting
/// that as `expected value at line 1 column 1` hides the real problem, so turn
/// non-JSON responses into a descriptive API error that names the URL.
fn parse_auth_response(
    bytes: &[u8],
    status: reqwest::StatusCode,
    url: &str,
) -> Result<AuthResponse> {
    match serde_json::from_slice::<AuthResponse>(bytes) {
        Ok(auth) => Ok(auth),
        Err(error) => {
            let body = String::from_utf8_lossy(bytes);
            let trimmed = body.trim();
            if !status.is_success() || trimmed.is_empty() || trimmed.starts_with('<') {
                let detail: String = trimmed.chars().take(160).collect();
                Err(CoinWatchError::Api {
                    status: status.as_u16(),
                    message: format!(
                        "{url} returned a non-JSON response (HTTP {status}). Check the API server URL in Settings → Network.{}",
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!(" Response: {detail}")
                        }
                    ),
                })
            } else {
                Err(CoinWatchError::Deserialization(error.to_string()))
            }
        }
    }
}

impl ApiClient {
    /// Ensure API requests have an absolute, usable origin.
    ///
    /// A session can only exist if the user signed in against a real backend,
    /// so fall back to the hosted cloud origin when no endpoint was configured
    /// (or when a legacy marketing origin was persisted by an older build).
    /// Without this, reqwest rejects `/api/...` paths with
    /// `RelativeUrlWithoutBase` and every account/dashboard request fails.
    fn ensure_base_url(&self) {
        if crate::data::client::normalize_api_origin(&self.base_url()).is_none() {
            tracing::info!(
                "No usable API endpoint configured for authentication; using {}",
                crate::data::client::DEFAULT_CLOUD_API_URL
            );
            self.set_base_url(crate::data::client::DEFAULT_CLOUD_API_URL);
        }
    }

    /// Log in with email and password.
    /// On success, stores the session internally and returns it.
    ///
    /// This is a blocking call — run it from a background thread
    /// (e.g. inside GPUI's `cx.spawn`).
    pub fn login(&self, email: &str, password: &str) -> Result<AuthSession> {
        self.ensure_base_url();
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
            parse_auth_response(&bytes, status, &url)
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
        self.ensure_base_url();
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
            let status = resp.status();
            let bytes = resp.bytes().await?;
            parse_auth_response(&bytes, status, &url)
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
        self.ensure_base_url();
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
            parse_auth_response(&body_bytes, status, &url)
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

        // A stored session implies a real backend. If no endpoint is configured
        // (for example after upgrading from a build with an empty default), use
        // the hosted cloud origin instead of issuing relative URLs.
        self.ensure_base_url();

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

    /// Login with Google OAuth via the CoinWatch website.
    ///
    /// This method:
    /// 1. Starts a local HTTP server on a random port
    /// 2. Opens the system browser to the configured CoinWatch origin
    ///    (staging.coinwat.ch / coinwat.ch), never a localhost page
    /// 3. The website hands the session to the local listener in the background
    /// 4. Stores the session and returns it
    ///
    /// Blocking — run from a background thread.
    pub fn login_with_google(&self) -> Result<AuthSession> {
        self.login_with_google_cancellable(Arc::new(AtomicBool::new(false)))
    }

    /// Same as [`Self::login_with_google`], but the wait loop aborts promptly
    /// once `cancel` is set. This lets the UI stop waiting when the user
    /// changes their mind instead of blocking until the 5 minute timeout.
    ///
    /// Blocking — run from a background thread.
    pub fn login_with_google_cancellable(&self, cancel: Arc<AtomicBool>) -> Result<AuthSession> {
        use std::net::TcpListener;
        use std::sync::Mutex;
        use url::Url;

        // A Google sign-in is a cloud flow; make sure the handoff and the
        // subsequent API calls use an absolute backend origin.
        self.ensure_base_url();

        // Find an available port
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| CoinWatchError::Auth(format!("Failed to bind local server: {}", e)))?;
        let port = listener
            .local_addr()
            .map_err(|e| CoinWatchError::Auth(format!("Failed to get local port: {}", e)))?
            .port();

        tracing::info!("OAuth callback server listening on port {}", port);

        // Always send the browser to a real CoinWatch origin. The configured
        // server URL is preferred; an unconfigured app falls back to production
        // instead of trying to open a localhost URL.
        let web_base = web_origin(&self.base_url());
        let mut oauth_url = Url::parse(&format!("{}/auth/native", web_base))
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
            // Abort promptly when the user cancels (or the page is closed)
            // instead of keeping the loopback listener alive for 5 minutes.
            if cancel.load(Ordering::Relaxed) {
                return Err(CoinWatchError::Auth("Sign in cancelled".into()));
            }

            let request = match server.recv_timeout(std::time::Duration::from_millis(250)) {
                Ok(Some(request)) => request,
                Ok(None) => {
                    if session_result.lock().unwrap().is_some() {
                        break;
                    }
                    continue;
                }
                Err(_) => break,
            };

            let method = request.method().clone();
            let request_url = request.url().to_string();

            // Browsers send a Private Network Access preflight before a public
            // HTTPS page is allowed to call the loopback listener.
            if method == tiny_http::Method::Options {
                let response = with_callback_cors(tiny_http::Response::empty(204));
                let _ = request.respond(response);
                continue;
            }

            let url_str = format!("http://127.0.0.1{}", request_url);
            let Ok(url) = Url::parse(&url_str) else {
                let response = with_callback_cors(
                    tiny_http::Response::from_string("Bad request").with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/plain; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
                let _ = request.respond(response);
                continue;
            };

            if method != tiny_http::Method::Get || url.path() != "/callback" {
                let response = with_callback_cors(
                    tiny_http::Response::from_string("Not found").with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/plain; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
                let _ = request.respond(response);
                continue;
            }

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
                let expires_at = params.get("expires_at").and_then(|s| s.parse::<i64>().ok());
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
                    tracing::warn!("session stored in memory but persistence failed: {}", e);
                });
                Ok(session)
            } else {
                Err(CoinWatchError::Auth(
                    "Missing required OAuth parameters".into(),
                ))
            };

            let success = session.is_ok();
            let email = session
                .as_ref()
                .map(|session| session.email.as_str())
                .unwrap_or("");
            let message = session
                .as_ref()
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            let response_body = native_callback_page(&web_base, success, email, &message);

            let response = with_callback_cors(
                tiny_http::Response::from_string(response_body).with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/html; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            );
            let _ = request.respond(response);

            *session_result_clone.lock().unwrap() = Some(session);
            break;
        }

        // Extract result
        let result = session_result.lock().unwrap().take();
        result.unwrap_or_else(|| {
            Err(CoinWatchError::Auth(
                "Timed out waiting for sign in".into(),
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

    #[test]
    fn html_auth_response_is_reported_as_api_error() {
        let body = b"<!DOCTYPE html><html><head><title>Attention Required!</title></head></html>";
        let error = parse_auth_response(
            body,
            reqwest::StatusCode::FORBIDDEN,
            "https://coinwat.ch/api/auth/login",
        )
        .expect_err("HTML must not deserialize as an auth response");

        match error {
            CoinWatchError::Api { status, message } => {
                assert_eq!(status, 403);
                assert!(message.contains("non-JSON response"));
                assert!(message.contains("https://coinwat.ch/api/auth/login"));
            }
            other => panic!("expected API error, got {other:?}"),
        }
    }

    #[test]
    fn json_auth_response_still_parses() {
        let body = br#"{"success":false,"error":"Invalid login credentials"}"#;
        let auth = parse_auth_response(
            body,
            reqwest::StatusCode::BAD_REQUEST,
            "https://staging.coinwat.ch/api/auth/login",
        )
        .expect("JSON auth response should parse");
        assert!(!auth.success);
        assert_eq!(auth.error.as_deref(), Some("Invalid login credentials"));
    }
}
