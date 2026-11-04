//! CoinWatch price-alert bridge.
//!
//! The web client stores alerts in Supabase and exposes them through
//! `/api/alerts` and `/api/alerts/logs`. The native client speaks the same
//! endpoints so an alert created here appears in the browser (and vice versa).

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::{CoinWatchError, Result};
use crate::data::trade_store::symbol_matches;

/// A saved price alert as returned by `GET /api/alerts`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Alert {
    pub id: String,
    pub exchange: String,
    pub symbol: String,
    /// Normalized condition used by the alert engine.
    #[serde(default)]
    pub condition: String,
    /// Original TradingView-style condition, when the web client supplied one.
    #[serde(rename = "originalCondition", default)]
    pub original_condition: Option<String>,
    pub price: f64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(rename = "triggerMode", default)]
    pub trigger_mode: Option<String>,
    #[serde(default)]
    pub expiration: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(rename = "notifyInApp", default)]
    pub notify_in_app: Option<bool>,
    #[serde(rename = "showToast", default)]
    pub show_toast: Option<bool>,
    #[serde(rename = "sendEmail", default)]
    pub send_email: Option<bool>,
    #[serde(rename = "webhookEnabled", default)]
    pub webhook_enabled: Option<bool>,
    #[serde(rename = "webhookUrl", default)]
    pub webhook_url: Option<String>,
    #[serde(rename = "playSound", default)]
    pub play_sound: Option<bool>,
    #[serde(rename = "soundStyle", default)]
    pub sound_style: Option<String>,
    #[serde(rename = "soundRepeat", default)]
    pub sound_repeat: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub triggered_at: Option<String>,
}

impl Alert {
    /// Condition to render. The web client preserves the user's original
    /// TradingView condition so the sidebar can show `Crossing Up` instead of
    /// the normalized engine condition.
    pub fn display_condition(&self) -> &str {
        self.original_condition
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.condition)
    }

    pub fn is_repeating(&self) -> bool {
        self.trigger_mode.as_deref() == Some("every_time")
    }

    pub fn is_expired(&self) -> bool {
        let Some(expiration) = self.expiration.as_deref() else {
            return false;
        };
        chrono::DateTime::parse_from_rfc3339(expiration)
            .map(|when| when < chrono::Utc::now())
            .unwrap_or(false)
    }

    pub fn is_active(&self) -> bool {
        self.enabled && !self.is_expired()
    }

    /// Direction used by chart overlays and the sidebar condition glyph.
    pub fn is_bullish(&self) -> bool {
        matches!(
            self.display_condition(),
            "crossing_up" | "greater_than" | "above"
        )
    }

    pub fn is_bearish(&self) -> bool {
        matches!(
            self.display_condition(),
            "crossing_down" | "less_than" | "below"
        )
    }

    pub fn glyph(&self) -> &'static str {
        if self.is_bullish() {
            "↗"
        } else if self.is_bearish() {
            "↘"
        } else {
            "⇄"
        }
    }
}

/// Process-wide alert snapshot shared by the alerts sidebar and chart panes.
///
/// The sidebar owns the network lifecycle; this store is the read-only mirror
/// that chart panels consult when painting alert levels. Keeping it separate
/// from the panel entity lets every pane render alerts even when the alerts
/// sidebar has never been opened.
pub struct AlertStore {
    alerts: Vec<Alert>,
    version: u64,
}

impl Default for AlertStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AlertStore {
    pub fn global() -> &'static Mutex<AlertStore> {
        static STORE: OnceLock<Mutex<AlertStore>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(AlertStore::new()))
    }

    pub fn new() -> Self {
        Self {
            alerts: Vec::new(),
            version: 0,
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn set_alerts(&mut self, alerts: Vec<Alert>) {
        if self.alerts == alerts {
            return;
        }
        self.alerts = alerts;
        self.version = self.version.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        if self.alerts.is_empty() {
            return;
        }
        self.alerts.clear();
        self.version = self.version.wrapping_add(1);
    }

    /// Alerts that belong on the chart for `exchange`/`symbol`.
    ///
    /// Symbol matching is normalized (BTC/USDT == BTCUSDT). Exchange matching
    /// is case-insensitive; an alert with no exchange is treated as global.
    pub fn alerts_for(&self, exchange: &str, symbol: &str) -> Vec<Alert> {
        let exchange = exchange.trim();
        self.alerts
            .iter()
            .filter(|alert| {
                if !symbol_matches(&alert.symbol, symbol) {
                    return false;
                }
                let alert_exchange = alert.exchange.trim();
                alert_exchange.is_empty()
                    || exchange.is_empty()
                    || alert_exchange.eq_ignore_ascii_case(exchange)
            })
            .cloned()
            .collect()
    }
}

/// One triggered alert event from `GET /api/alerts/logs`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AlertLog {
    pub id: String,
    #[serde(rename = "alertId", default)]
    pub alert_id: Option<String>,
    pub exchange: String,
    pub symbol: String,
    #[serde(default)]
    pub condition: String,
    #[serde(rename = "originalCondition", default)]
    pub original_condition: Option<String>,
    pub price: f64,
    pub triggered_at: String,
}

impl AlertLog {
    pub fn display_condition(&self) -> &str {
        self.original_condition
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&self.condition)
    }
}

/// Request body for `POST /api/alerts` and `PATCH /api/alerts/{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct AlertPayload {
    pub exchange: String,
    pub symbol: String,
    pub condition: String,
    #[serde(rename = "originalCondition")]
    pub original_condition: String,
    pub price: f64,
    pub enabled: bool,
    #[serde(rename = "triggerMode")]
    pub trigger_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(rename = "notifyInApp")]
    pub notify_in_app: bool,
    #[serde(rename = "showToast")]
    pub show_toast: bool,
    #[serde(rename = "sendEmail")]
    pub send_email: bool,
    #[serde(rename = "webhookEnabled")]
    pub webhook_enabled: bool,
    #[serde(rename = "webhookUrl", skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(rename = "playSound")]
    pub play_sound: bool,
    #[serde(rename = "soundStyle")]
    pub sound_style: String,
    #[serde(rename = "soundRepeat")]
    pub sound_repeat: String,
}

impl AlertPayload {
    pub fn new(
        exchange: impl Into<String>,
        symbol: impl Into<String>,
        condition: impl Into<String>,
        price: f64,
    ) -> Self {
        let condition = condition.into();
        Self {
            exchange: exchange.into(),
            symbol: symbol.into(),
            original_condition: condition.clone(),
            condition,
            price,
            enabled: true,
            trigger_mode: "once".to_string(),
            expiration: None,
            message: None,
            notify_in_app: true,
            show_toast: true,
            send_email: false,
            webhook_enabled: false,
            webhook_url: None,
            play_sound: true,
            sound_style: "Thin".to_string(),
            sound_repeat: "Once".to_string(),
        }
    }
}

impl ApiClient {
    /// List every alert belonging to the signed-in user.
    /// `GET /api/alerts` (Bearer auth required).
    ///
    /// Blocking — run from a background thread.
    pub fn list_alerts(&self) -> Result<Vec<Alert>> {
        let rt = tokio_runtime();
        let req = self.auth_get("/api/alerts");
        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Create a price alert. `POST /api/alerts` (Bearer auth required).
    ///
    /// Blocking — run from a background thread.
    pub fn create_alert(&self, payload: &AlertPayload) -> Result<Alert> {
        let rt = tokio_runtime();
        let req = self.auth_post("/api/alerts").json(payload);
        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Update a price alert. `PATCH /api/alerts/{id}` (Bearer auth required).
    ///
    /// Blocking — run from a background thread.
    pub fn update_alert(&self, id: &str, payload: &AlertPayload) -> Result<Alert> {
        let rt = tokio_runtime();
        let req = self.auth_patch(&format!("/api/alerts/{id}")).json(payload);
        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Delete a price alert. `DELETE /api/alerts/{id}` (Bearer auth required).
    ///
    /// Blocking — run from a background thread.
    pub fn delete_alert(&self, id: &str) -> Result<()> {
        let rt = tokio_runtime();
        let req = self.auth_delete(&format!("/api/alerts/{id}"));
        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                Ok(())
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(CoinWatchError::Api {
                    status,
                    message: body,
                })
            }
        })
    }

    /// List the user's trigger history. `GET /api/alerts/logs`.
    ///
    /// Blocking — run from a background thread.
    pub fn list_alert_logs(&self) -> Result<Vec<AlertLog>> {
        let rt = tokio_runtime();
        let req = self.auth_get("/api/alerts/logs");
        rt.block_on(async {
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Clear the user's trigger history. `DELETE /api/alerts/logs`.
    ///
    /// Blocking — run from a background thread.
    pub fn clear_alert_logs(&self) -> Result<()> {
        let rt = tokio_runtime();
        let req = self.auth_delete("/api/alerts/logs");
        rt.block_on(async {
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                Ok(())
            } else {
                let body = resp.text().await.unwrap_or_default();
                Err(CoinWatchError::Api {
                    status,
                    message: body,
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_deserializes_coinwatch_payload() {
        let alert: Alert = serde_json::from_value(serde_json::json!({
            "id": "a1",
            "exchange": "binance",
            "symbol": "BTC/USDT",
            "condition": "crossing_up",
            "originalCondition": "crossing_up",
            "price": 65000.5,
            "enabled": true,
            "triggerMode": "every_time",
            "expiration": null,
            "playSound": true,
            "created_at": "2026-11-04T10:00:00Z"
        }))
        .expect("alert payload should deserialize");

        assert_eq!(alert.display_condition(), "crossing_up");
        assert!(alert.is_repeating());
        assert!(alert.is_active());
        assert!(!alert.is_expired());
        assert!(alert.is_bullish());
        assert!(!alert.is_bearish());
        assert_eq!(alert.glyph(), "↗");
    }

    #[test]
    fn expired_alert_is_not_active() {
        let alert = Alert {
            id: "a1".into(),
            exchange: "binance".into(),
            symbol: "BTC/USDT".into(),
            condition: "above".into(),
            original_condition: None,
            price: 1.0,
            enabled: true,
            trigger_mode: Some("once".into()),
            expiration: Some("2000-01-01T00:00:00Z".into()),
            message: None,
            notify_in_app: None,
            show_toast: None,
            send_email: None,
            webhook_enabled: None,
            webhook_url: None,
            play_sound: None,
            sound_style: None,
            sound_repeat: None,
            created_at: None,
            triggered_at: None,
        };

        assert!(alert.is_expired());
        assert!(!alert.is_active());
    }

    #[test]
    fn payload_uses_coinwatch_field_names() {
        let payload = AlertPayload::new("binance", "ETH/USDT", "above", 3500.0);
        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["originalCondition"], "above");
        assert_eq!(value["triggerMode"], "once");
        assert_eq!(value["notifyInApp"], true);
        assert_eq!(value["playSound"], true);
    }

    #[test]
    fn alert_log_deserializes() {
        let log: AlertLog = serde_json::from_value(serde_json::json!({
            "id": "l1",
            "alertId": "a1",
            "exchange": "binance",
            "symbol": "BTC/USDT",
            "condition": "above",
            "originalCondition": "crossing",
            "price": 65000,
            "triggered_at": "2026-11-04T10:00:00Z"
        }))
        .expect("log payload should deserialize");

        assert_eq!(log.alert_id.as_deref(), Some("a1"));
        assert_eq!(log.display_condition(), "crossing");
    }

    fn alert(exchange: &str, symbol: &str, price: f64) -> Alert {
        Alert {
            id: format!("{exchange}-{symbol}-{price}"),
            exchange: exchange.into(),
            symbol: symbol.into(),
            condition: "crossing".into(),
            original_condition: None,
            price,
            enabled: true,
            trigger_mode: Some("once".into()),
            expiration: None,
            message: None,
            notify_in_app: None,
            show_toast: None,
            send_email: None,
            webhook_enabled: None,
            webhook_url: None,
            play_sound: None,
            sound_style: None,
            sound_repeat: None,
            created_at: None,
            triggered_at: None,
        }
    }

    #[test]
    fn alert_store_filters_by_exchange_and_normalized_symbol() {
        let mut store = AlertStore::new();
        store.set_alerts(vec![
            alert("binance", "BTC/USDT", 65_000.0),
            alert("binanceusdm", "BTCUSDT", 66_000.0),
            alert("binance", "ETH/USDT", 3_000.0),
        ]);

        let btc = store.alerts_for("BINANCE", "BTCUSDT");
        assert_eq!(btc.len(), 1);
        assert_eq!(btc[0].price, 65_000.0);

        let all_btc = store.alerts_for("", "BTC/USDT");
        assert_eq!(all_btc.len(), 2);
    }

    #[test]
    fn alert_store_bumps_version_only_on_change() {
        let mut store = AlertStore::new();
        let version = store.version();
        store.set_alerts(vec![alert("binance", "BTC/USDT", 65_000.0)]);
        assert_ne!(store.version(), version);

        let version = store.version();
        store.set_alerts(vec![alert("binance", "BTC/USDT", 65_000.0)]);
        assert_eq!(store.version(), version);
    }
}
