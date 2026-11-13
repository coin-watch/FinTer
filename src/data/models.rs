use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────
// Auth models (matches /api/auth/* responses)
// ──────────────────────────────────────────────

/// Credentials sent to POST /api/auth/login and /api/auth/signup.
#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Successful auth response from login, signup, or refresh.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthSession {
    pub user_id: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Option<i64>,
    /// User's display name (from OAuth provider)
    #[serde(default)]
    pub name: Option<String>,
    /// User's avatar URL (from OAuth provider, e.g. Google profile picture)
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// POST /api/auth/refresh request body.
#[derive(Debug, Serialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// GET /api/auth/status response.
#[derive(Debug, Deserialize)]
pub struct AuthStatusResponse {
    pub logged_in: bool,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
}

// ──────────────────────────────────────────────
// Exchange / CCXT models
// ──────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_spot() -> String {
    "spot".to_string()
}

/// An exchange as returned by GET /api/exchanges or /api/v1/exchanges.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExchangeInfo {
    pub id: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub label: String,
}

impl ExchangeInfo {
    pub fn new(id: impl Into<String>, value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            label: label.into(),
        }
    }

    pub fn from_id(id: impl Into<String>) -> Self {
        let id_str = id.into();
        let label = id_str.to_uppercase();
        Self {
            id: id_str.clone(),
            value: id_str,
            label,
        }
    }

    pub fn normalized(mut self) -> Self {
        if self.value.is_empty() {
            self.value = self.id.clone();
        }
        if self.label.is_empty() {
            self.label = self.id.to_uppercase();
        }
        self
    }
}

/// A market type as returned by GET /api/exchanges/{id}/markets.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketInfo {
    pub value: String,
    pub label: String,
    #[serde(rename = "type")]
    pub market_type: String,
}

/// A trading symbol as returned by GET /api/exchanges/{id}/symbols.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SymbolInfo {
    pub symbol: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub quote: String,
    #[serde(rename = "baseAsset", default)]
    pub base_asset: Option<String>,
    #[serde(rename = "quoteAsset", default)]
    pub quote_asset: Option<String>,
    #[serde(rename = "type", default = "default_spot")]
    pub symbol_type: String,
    #[serde(default = "default_true")]
    pub active: bool,
    /// Precision for price (number of decimal places)
    #[serde(default)]
    pub precision: Option<PrecisionInfo>,
}

impl SymbolInfo {
    pub fn from_symbol(symbol: impl Into<String>) -> Self {
        let sym_str = symbol.into();
        let mut parts = sym_str.split('/');
        let base = parts.next().unwrap_or(&sym_str).to_string();
        let quote = parts.next().unwrap_or("").to_string();
        Self {
            symbol: sym_str,
            base,
            quote,
            base_asset: None,
            quote_asset: None,
            symbol_type: "spot".to_string(),
            active: true,
            precision: None,
        }
    }

    pub fn normalized(mut self) -> Self {
        if self.base.is_empty() || self.quote.is_empty() {
            let mut parts = self.symbol.split('/');
            if self.base.is_empty() {
                self.base = parts.next().unwrap_or(&self.symbol).to_string();
            } else {
                let _ = parts.next();
            }
            if self.quote.is_empty() {
                self.quote = parts.next().unwrap_or("").to_string();
            }
        }
        if self.symbol_type.is_empty() {
            self.symbol_type = "spot".to_string();
        }
        self
    }
}

/// Price and amount precision information from exchange markets
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrecisionInfo {
    /// Number of decimal places for price
    #[serde(default)]
    pub price: Option<i32>,
    /// Number of decimal places for amount/quantity
    #[serde(default)]
    pub amount: Option<i32>,
    /// Minimum price increment (tickSize)
    #[serde(rename = "tickSize", default)]
    pub tick_size: Option<f64>,
}

/// A single OHLCV candle as returned by GET /api/exchanges/{id}/ohlcv.
///
/// Note: The timestamp field can be either:
/// - An i64 unix timestamp in milliseconds (from WebSocket messages)
/// - An ISO 8601 string like "2026-04-08T04:43:00.000Z" (from REST API)
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct OhlcvCandle {
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Custom deserializer for timestamp field that handles both i64 and ISO 8601 strings.
fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct TimestampVisitor;

    impl<'de> Visitor<'de> for TimestampVisitor {
        type Value = i64;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an integer timestamp or an ISO 8601 string")
        }

        fn visit_i64<E>(self, value: i64) -> Result<i64, E>
        where
            E: de::Error,
        {
            // Normalize second-precision timestamps to milliseconds so every
            // candle in FinTer is uniformly 13 digits.
            if value > 0 && value < 10_000_000_000 {
                Ok(value * 1000)
            } else {
                Ok(value)
            }
        }

        fn visit_u64<E>(self, value: u64) -> Result<i64, E>
        where
            E: de::Error,
        {
            // Normalize second-precision timestamps to milliseconds so every
            // candle in FinTer is uniformly 13 digits.
            if value > 0 && value < 10_000_000_000 {
                Ok((value * 1000) as i64)
            } else {
                Ok(value as i64)
            }
        }

        fn visit_str<E>(self, value: &str) -> Result<i64, E>
        where
            E: de::Error,
        {
            // Parse ISO 8601 timestamp string to i64 milliseconds
            use chrono::DateTime;
            let dt = DateTime::parse_from_rfc3339(value)
                .map_err(|e| E::custom(format!("Failed to parse timestamp: {}", e)))?;
            Ok(dt.timestamp_millis())
        }
    }

    deserializer.deserialize_any(TimestampVisitor)
}

// ──────────────────────────────────────────────
// Module models (workspace layouts)
// ──────────────────────────────────────────────

/// A saved workspace module as returned by GET /api/modules.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Module {
    pub id: Option<String>,
    pub user_id: Option<String>,
    #[serde(default)]
    pub module_name: String,
    #[serde(default)]
    pub module: serde_json::Value,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Request body for POST /api/modules.
#[derive(Debug, Serialize)]
pub struct CreateModuleRequest {
    pub user_id: String,
    pub module_name: String,
    pub module: serde_json::Value,
}

/// Request body for PATCH /api/modules/{id}.
#[derive(Debug, Serialize)]
pub struct UpdateModuleRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<serde_json::Value>,
}

// ──────────────────────────────────────────────
// Layout Preset models
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LayoutPreset {
    pub id: Option<String>,
    pub user_id: Option<String>,
    pub name: String,
    pub tree: serde_json::Value,
    #[serde(default)]
    pub is_public: Option<bool>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateLayoutPresetRequest {
    pub user_id: String,
    pub name: String,
    pub tree: serde_json::Value,
    pub is_public: bool,
}

// ──────────────────────────────────────────────
// Settings models
// ──────────────────────────────────────────────

/// User settings as returned by GET /api/settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserSettings {
    pub id: Option<String>,
    pub user_id: String,
    pub settings: SettingsData,
}

/// The actual settings payload.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SettingsData {
    // Dashboard & Layout Settings
    #[serde(rename = "currentDashboard", default)]
    pub current_dashboard: Option<String>,

    // Theme & Appearance Settings
    #[serde(rename = "darkMode", default)]
    pub dark_mode: Option<String>,
    #[serde(default)]
    pub theme: Option<String>,

    // Network & Backend Server Settings
    #[serde(rename = "serverUrl", default)]
    pub server_url: Option<String>,

    // Locale & Time Settings
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(rename = "autoCheckUpdates", default)]
    pub auto_check_updates: Option<bool>,
    #[serde(rename = "updateChannel", default)]
    pub update_channel: Option<String>,

    // Dashboard settings shared with the CoinWatch web client.
    #[serde(rename = "defaultTickerExchange", default)]
    pub default_ticker_exchange: Option<String>,
    #[serde(rename = "linkDashboardTabs", default)]
    pub link_dashboard_tabs: Option<bool>,
    #[serde(rename = "surveyStatus", default)]
    pub survey_status: Option<String>,

    // Trading & Exchange Settings
    #[serde(rename = "defaultTradingPair", default)]
    pub default_trading_pair: Option<String>,

    // Keep new web settings intact when the desktop client reads and writes the
    // shared JSONB settings record. Known fields above remain strongly typed.
    #[serde(flatten, default)]
    pub additional: std::collections::HashMap<String, serde_json::Value>,

    // Alert Settings
    #[serde(rename = "enablePriceAlerts", default)]
    pub enable_price_alerts: Option<bool>,
    #[serde(rename = "enableVolumeAlerts", default)]
    pub enable_volume_alerts: Option<bool>,
    #[serde(rename = "alertSoundEnabled", default)]
    pub alert_sound_enabled: Option<bool>,
    #[serde(rename = "alertDesktopNotifications", default)]
    pub alert_desktop_notifications: Option<bool>,

    // Chart & Drawing Settings
    #[serde(rename = "chartBackground", default)]
    pub chart_background: Option<String>,
    #[serde(rename = "drawingToolsEnabled", default)]
    pub drawing_tools_enabled: Option<bool>,
    #[serde(rename = "magnetSnapEnabled", default)]
    pub magnet_snap_enabled: Option<bool>,
    #[serde(rename = "chartGridVisible", default)]
    pub chart_grid_visible: Option<bool>,

    // Performance Settings
    #[serde(rename = "maxFPS", default)]
    pub max_fps: Option<u32>,
    #[serde(rename = "enableAnimations", default)]
    pub enable_animations: Option<bool>,
    #[serde(rename = "dataUpdateFrequency", default)]
    pub data_update_frequency: Option<u32>,

    // Notification Settings
    #[serde(rename = "enableOrderNotifications", default)]
    pub enable_order_notifications: Option<bool>,
    #[serde(rename = "enableTradeNotifications", default)]
    pub enable_trade_notifications: Option<bool>,
    #[serde(rename = "enableSystemNotifications", default)]
    pub enable_system_notifications: Option<bool>,

    // Data & Backup Settings
    #[serde(rename = "autoBackupEnabled", default)]
    pub auto_backup_enabled: Option<bool>,
    #[serde(rename = "dataRetentionDays", default)]
    pub data_retention_days: Option<u32>,
}

/// Request body for POST /api/settings (upsert).
#[derive(Debug, Serialize)]
pub struct UpsertSettingsRequest {
    pub user_id: String,
    pub settings: SettingsData,
}

// ──────────────────────────────────────────────
// Indicator models
// ──────────────────────────────────────────────

/// A saved indicator as returned by GET /api/indicators.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Indicator {
    pub id: Option<String>,
    pub user_id: String,
    pub indicator_name: String,
    /// Arbitrary JSON representing indicator configuration.
    pub indicator: serde_json::Value,
    #[serde(rename = "type", default)]
    pub indicator_type: Option<String>,
    #[serde(default)]
    pub public: Option<bool>,
}

/// Request body for POST /api/indicators.
#[derive(Debug, Serialize)]
pub struct CreateIndicatorRequest {
    pub user_id: String,
    pub indicator_name: String,
    pub indicator: serde_json::Value,
    #[serde(rename = "type")]
    pub indicator_type: String,
    #[serde(default)]
    pub public: bool,
}

// ──────────────────────────────────────────────
// Subscription models
// ──────────────────────────────────────────────

/// Subscription status as returned by GET /api/subscription.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subscription {
    pub id: Option<String>,
    pub user_id: String,
    pub status: String,
}

// ──────────────────────────────────────────────
// Generic API response wrappers
// ──────────────────────────────────────────────

/// Standard `{ success: true, data: T }` response.
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthResponseSessionUser {
    pub id: Option<String>,
    pub email: Option<String>,
    #[serde(default)]
    pub user_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthResponseSession {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub user: Option<AuthResponseSessionUser>,
}

/// Auth-specific response that includes success + optional user fields.
#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub success: bool,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub error: Option<String>,
    pub message: Option<String>,
    #[serde(default)]
    pub expired: Option<bool>,
    /// User's display name (from OAuth provider)
    #[serde(default)]
    pub name: Option<String>,
    /// User's avatar URL (from OAuth provider)
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// Optional nested session from Supabase SSR
    pub session: Option<AuthResponseSession>,
}

impl AuthResponse {
    pub fn access_token(&self) -> Option<String> {
        self.access_token
            .clone()
            .or_else(|| self.session.as_ref().and_then(|s| s.access_token.clone()))
    }

    pub fn refresh_token(&self) -> Option<String> {
        self.refresh_token
            .clone()
            .or_else(|| self.session.as_ref().and_then(|s| s.refresh_token.clone()))
    }

    pub fn expires_at(&self) -> Option<i64> {
        self.expires_at
            .or_else(|| self.session.as_ref().and_then(|s| s.expires_at))
    }

    pub fn user_id(&self) -> Option<String> {
        self.user_id
            .clone()
            .or_else(|| self.session.as_ref().and_then(|s| s.user.as_ref()).and_then(|u| u.id.clone()))
    }

    pub fn email(&self) -> Option<String> {
        self.email
            .clone()
            .or_else(|| self.session.as_ref().and_then(|s| s.user.as_ref()).and_then(|u| u.email.clone()))
    }

    pub fn name(&self) -> Option<String> {
        self.name
            .clone()
            .or_else(|| {
                self.session.as_ref()
                    .and_then(|s| s.user.as_ref())
                    .and_then(|u| u.user_metadata.as_ref())
                    .and_then(|m| {
                        m.get("name")
                            .or_else(|| m.get("full_name"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
            })
    }

    pub fn avatar_url(&self) -> Option<String> {
        self.avatar_url
            .clone()
            .or_else(|| {
                self.session.as_ref()
                    .and_then(|s| s.user.as_ref())
                    .and_then(|u| u.user_metadata.as_ref())
                    .and_then(|m| {
                        m.get("avatar_url")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
            })
    }
}

#[cfg(test)]
mod timestamp_tests {
    use super::OhlcvCandle;

    fn parse(ts_json: &str) -> OhlcvCandle {
        serde_json::from_str(&format!(
            r#"{{"timestamp":{},"open":1.0,"high":2.0,"low":0.5,"close":1.5,"volume":10.0}}"#,
            ts_json
        ))
        .expect("candle should deserialize")
    }

    #[test]
    fn seconds_are_normalized_to_milliseconds() {
        assert_eq!(parse("1600000000").timestamp, 1_600_000_000_000);
    }

    #[test]
    fn millisecond_timestamps_are_preserved() {
        assert_eq!(parse("1600000000000").timestamp, 1_600_000_000_000);
        assert_eq!(parse("1700000000123").timestamp, 1_700_000_000_123);
    }

    #[test]
    fn zero_and_negative_timestamps_are_preserved() {
        assert_eq!(parse("0").timestamp, 0);
        assert_eq!(parse("-1000").timestamp, -1000);
    }

    #[test]
    fn iso_strings_stay_in_milliseconds() {
        let c = parse(r#""2026-04-08T04:43:00.000Z""#);
        assert_eq!(c.timestamp, 1_775_623_380_000);
    }
}
