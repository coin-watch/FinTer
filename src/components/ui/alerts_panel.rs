//! Alerts secondary sidebar.
//!
//! A compact "signal desk" for the price alerts that live in the user's
//! CoinWatch account. Alerts created here are written through the same
//! `/api/alerts` endpoints as the web client, so both surfaces stay in sync.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext, ClickEvent, Context, ElementId, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::notification::Notification;
use gpui_component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_component::switch::Switch;
use gpui_component::theme::ActiveTheme;
use gpui_component::{Disableable, Icon, Sizable, WindowExt, h_flex, v_flex};

use crate::components::ui::ExchangeSelectItem;
use crate::data::alerts::{Alert, AlertLog, AlertPayload, AlertStore};
use crate::data::client::{ApiClient, tokio_runtime};
use crate::data::error::CoinWatchError;
use crate::data::gateway_registry::GatewayRegistry;
use crate::data::terminal_audio::{TerminalAudio, Tone};
use crate::data::trade_store::TradeStore;

/// Events emitted by the alerts panel for the parent view.
#[derive(Clone, Debug)]
pub enum AlertsPanelEvent {
    /// The user wants to sign in to sync alerts.
    RequestLogin,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AlertsTab {
    Alerts,
    History,
}

#[derive(Clone)]
struct AlertEditor {
    id: Option<String>,
    exchange: String,
    symbol: String,
    condition: String,
    price: String,
    trigger_mode: String,
    play_sound: bool,
    enabled: bool,
}

#[derive(Clone)]
struct SymbolSelectItem {
    symbol: SharedString,
}

impl SymbolSelectItem {
    fn new(symbol: impl Into<SharedString>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }
}

impl SelectItem for SymbolSelectItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.symbol.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.symbol
    }
}

const CONDITIONS: &[(&str, &str, &str)] = &[
    ("crossing", "Crossing", "⇄"),
    ("crossing_up", "Cross Up", "↗"),
    ("crossing_down", "Cross Down", "↘"),
    ("greater_than", "Greater", ">"),
    ("less_than", "Less", "<"),
    ("above", "Above", "↑"),
    ("below", "Below", "↓"),
];

pub struct AlertsPanel {
    api_client: ApiClient,
    alerts: Vec<Alert>,
    logs: Vec<AlertLog>,
    active_tab: AlertsTab,
    loading: bool,
    logs_loading: bool,
    loaded_once: bool,
    error: Option<String>,
    editor: Option<AlertEditor>,
    submitting: bool,
    pending_delete: Option<String>,
    exchange_select: Option<Entity<SelectState<SearchableVec<ExchangeSelectItem>>>>,
    symbol_select: Option<Entity<SelectState<SearchableVec<SymbolSelectItem>>>>,
    price_input: Option<Entity<InputState>>,
    symbol_select_exchange: String,
    symbol_select_loaded: bool,
    last_seen_log_id: Option<String>,
    pending_notifications: Vec<AlertLog>,
}

impl EventEmitter<AlertsPanelEvent> for AlertsPanel {}

impl AlertsPanel {
    pub fn new(api_client: ApiClient, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            api_client,
            alerts: Vec::new(),
            logs: Vec::new(),
            active_tab: AlertsTab::Alerts,
            loading: false,
            logs_loading: false,
            loaded_once: false,
            error: None,
            editor: None,
            submitting: false,
            pending_delete: None,
            exchange_select: None,
            symbol_select: None,
            price_input: None,
            symbol_select_exchange: String::new(),
            symbol_select_loaded: false,
            last_seen_log_id: None,
            pending_notifications: Vec::new(),
        };
        panel.start_polling(cx);
        panel
    }

    /// Load alerts and trigger history. Safe to call repeatedly; the first
    /// call after sign-in is the one that populates the panel.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.submitting {
            return;
        }
        if self.api_client.get_session().is_none() {
            self.alerts.clear();
            self.logs.clear();
            self.loaded_once = true;
            self.error = None;
            clear_published_alerts();
            cx.notify();
            return;
        }

        self.loading = true;
        self.error = None;
        cx.notify();

        let client = self.api_client.clone();
        let rt = tokio_runtime();
        cx.spawn(async move |this, cx| {
            let result = rt
                .spawn_blocking(move || (client.list_alerts(), client.list_alert_logs()))
                .await;

            let _ = this.update(cx, |panel, cx| {
                panel.loading = false;
                panel.loaded_once = true;
                if panel.api_client.get_session().is_none() {
                    clear_published_alerts();
                    cx.notify();
                    return;
                }
                match result {
                    Ok((alerts_result, logs_result)) => {
                        match alerts_result {
                            Ok(alerts) => {
                                panel.alerts = alerts;
                                publish_alerts(&panel.alerts);
                                panel.error = None;
                            }
                            Err(error) => {
                                panel.error = Some(friendly_error(&error));
                            }
                        }
                        match logs_result {
                            Ok(logs) => panel.apply_logs(logs),
                            Err(error) => {
                                if panel.error.is_none() {
                                    panel.error = Some(friendly_error(&error));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        panel.error = Some(format!("Alert sync failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Clear local state when the session ends.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.alerts.clear();
        self.logs.clear();
        self.loaded_once = false;
        self.loading = false;
        self.logs_loading = false;
        self.submitting = false;
        self.error = None;
        self.editor = None;
        self.pending_delete = None;
        self.last_seen_log_id = None;
        self.pending_notifications.clear();
        clear_published_alerts();
        cx.notify();
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(25))
                    .await;

                let should_continue = this
                    .update(cx, |panel, cx| {
                        if !panel.loaded_once || panel.api_client.get_session().is_none() {
                            return true;
                        }
                        panel.poll(cx);
                        true
                    })
                    .unwrap_or(false);

                if !should_continue {
                    break;
                }
            }
        })
        .detach();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.logs_loading || self.loading || self.submitting {
            return;
        }
        self.logs_loading = true;
        let client = self.api_client.clone();
        let rt = tokio_runtime();
        cx.spawn(async move |this, cx| {
            let result = rt
                .spawn_blocking(move || (client.list_alerts(), client.list_alert_logs()))
                .await;
            let _ = this.update(cx, |panel, cx| {
                panel.logs_loading = false;
                if panel.api_client.get_session().is_none() {
                    clear_published_alerts();
                    cx.notify();
                    return;
                }
                if let Ok((alerts_result, logs_result)) = result {
                    let mut synced = false;
                    if let Ok(alerts) = alerts_result {
                        panel.alerts = alerts;
                        publish_alerts(&panel.alerts);
                        synced = true;
                    }
                    if let Ok(logs) = logs_result {
                        panel.apply_logs(logs);
                        synced = true;
                    }
                    if synced {
                        panel.error = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_logs(&mut self, logs: Vec<AlertLog>) {
        let newest = logs.first().map(|log| log.id.clone());
        let last_seen = self.last_seen_log_id.clone();

        match last_seen {
            None => {
                self.last_seen_log_id = newest;
            }
            Some(last_seen) => {
                let last_seen_still_present = logs.iter().any(|log| log.id == last_seen);
                if !last_seen_still_present {
                    self.last_seen_log_id = newest;
                } else if newest.as_deref() != Some(last_seen.as_str()) {
                    let mut fresh = Vec::new();
                    for log in &logs {
                        if log.id == last_seen {
                            break;
                        }
                        fresh.push(log.clone());
                    }
                    if !fresh.is_empty() {
                        for log in &fresh {
                            self.play_alert_cue(log);
                            if self.should_toast(log) {
                                self.pending_notifications.push(log.clone());
                            }
                        }
                        if self.pending_notifications.len() > 20 {
                            let excess = self.pending_notifications.len() - 20;
                            self.pending_notifications.drain(0..excess);
                        }
                        self.last_seen_log_id = newest;
                    }
                }
            }
        }

        self.logs = logs;
    }

    fn play_alert_cue(&self, log: &AlertLog) {
        let Some(alert_id) = log.alert_id.as_deref() else {
            return;
        };
        let Some(alert) = self.alerts.iter().find(|alert| alert.id == alert_id) else {
            return;
        };
        if !alert.play_sound.unwrap_or(false) {
            return;
        }

        let tone = match log.display_condition() {
            "crossing_down" | "less_than" | "below" => Tone::Sell,
            _ => Tone::Buy,
        };
        TerminalAudio::shared().play(tone);
    }

    fn should_toast(&self, log: &AlertLog) -> bool {
        log.alert_id
            .as_ref()
            .and_then(|id| self.alerts.iter().find(|alert| &alert.id == id))
            .and_then(|alert| alert.show_toast)
            .unwrap_or(true)
    }

    fn ensure_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.price_input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("0.00"));
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let Some(editor) = &mut this.editor {
                        editor.price = value;
                    }
                }
            })
            .detach();
            self.price_input = Some(input);
        }

        if self.exchange_select.is_none() {
            let items: Vec<ExchangeSelectItem> = GatewayRegistry::global()
                .exchange_infos()
                .into_iter()
                .map(|info| ExchangeSelectItem::new(info.id, info.label))
                .collect();
            let delegate = SearchableVec::new(items);
            let select = cx.new(|cx| SelectState::new(delegate, None, window, cx).searchable(true));
            cx.subscribe(
                &select,
                |this, _select, event: &SelectEvent<SearchableVec<ExchangeSelectItem>>, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        let value = value.to_string();
                        if let Some(editor) = &mut this.editor {
                            if editor.exchange != value {
                                editor.exchange = value;
                                editor.symbol.clear();
                                this.symbol_select_exchange.clear();
                                cx.notify();
                            }
                        }
                    }
                },
            )
            .detach();
            self.exchange_select = Some(select);
        }

        if self.symbol_select.is_none() {
            let delegate = SearchableVec::new(Vec::<SymbolSelectItem>::new());
            let select = cx.new(|cx| SelectState::new(delegate, None, window, cx).searchable(true));
            cx.subscribe(
                &select,
                |this, _select, event: &SelectEvent<SearchableVec<SymbolSelectItem>>, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        if let Some(editor) = &mut this.editor {
                            editor.symbol = value.to_string();
                            cx.notify();
                        }
                    }
                },
            )
            .detach();
            self.symbol_select = Some(select);
        }
    }

    fn sync_symbol_items(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let exchange = editor.exchange.clone();
        let symbol = editor.symbol.clone();
        let loaded = GatewayRegistry::global().symbols_loaded(&exchange);

        if self.symbol_select_exchange == exchange && (self.symbol_select_loaded || !loaded) {
            return;
        }

        let items: Vec<SymbolSelectItem> = GatewayRegistry::global()
            .symbol_infos(&exchange)
            .into_iter()
            .map(|info| SymbolSelectItem::new(info.symbol))
            .collect();

        if let Some(select) = &self.symbol_select {
            select.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(items), window, cx);
                if !symbol.is_empty() {
                    let value = SharedString::from(symbol.clone());
                    state.set_selected_value(&value, window, cx);
                }
            });
        }

        self.symbol_select_exchange = exchange;
        self.symbol_select_loaded = loaded;
    }

    fn open_editor_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_widgets(window, cx);
        let exchange = "binance".to_string();
        self.editor = Some(AlertEditor {
            id: None,
            exchange: exchange.clone(),
            symbol: String::new(),
            condition: "crossing".to_string(),
            price: String::new(),
            trigger_mode: "once".to_string(),
            play_sound: true,
            enabled: true,
        });
        self.symbol_select_exchange.clear();
        self.pending_delete = None;
        self.error = None;

        if let Some(select) = &self.exchange_select {
            let value = SharedString::from(exchange);
            select.update(cx, |state, cx| {
                state.set_selected_value(&value, window, cx);
            });
        }
        if let Some(input) = &self.price_input {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.sync_symbol_items(window, cx);
        cx.notify();
    }

    fn open_editor_edit(&mut self, alert: Alert, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_widgets(window, cx);
        let price = price_input_value(alert.price);
        self.editor = Some(AlertEditor {
            id: Some(alert.id.clone()),
            exchange: alert.exchange.clone(),
            symbol: alert.symbol.clone(),
            condition: alert.display_condition().to_string(),
            price: price.clone(),
            trigger_mode: alert
                .trigger_mode
                .clone()
                .unwrap_or_else(|| "once".to_string()),
            play_sound: alert.play_sound.unwrap_or(true),
            enabled: alert.enabled,
        });
        self.symbol_select_exchange.clear();
        self.pending_delete = None;
        self.error = None;

        if let Some(select) = &self.exchange_select {
            let value = SharedString::from(alert.exchange.clone());
            select.update(cx, |state, cx| {
                state.set_selected_value(&value, window, cx);
            });
        }
        if let Some(input) = &self.price_input {
            input.update(cx, |state, cx| state.set_value(&price, window, cx));
        }
        self.sync_symbol_items(window, cx);
        cx.notify();
    }

    fn close_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.pending_delete = None;
        self.error = None;
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };

        // The editor keeps a copy of the price for rendering, but the live
        // value lives in the InputState. Read it at save time so typing a price
        // without triggering another state update still submits correctly.
        let price_text = self
            .price_input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_else(|| editor.price.clone());
        let Some(price) = parse_price_input(&price_text) else {
            self.error = Some("Enter a target price greater than zero.".to_string());
            cx.notify();
            return;
        };

        if editor.symbol.trim().is_empty() {
            self.error = Some("Choose a symbol for this alert.".to_string());
            cx.notify();
            return;
        }

        // Editing must preserve notification channels (email, webhook, toast)
        // that the web client may have configured. Start from the stored alert
        // and only overwrite the fields this compact editor exposes.
        let existing = editor
            .id
            .as_ref()
            .and_then(|id| self.alerts.iter().find(|alert| &alert.id == id))
            .cloned();

        let mut payload = match &existing {
            Some(alert) => payload_from_alert(alert),
            None => AlertPayload::new(
                editor.exchange.clone(),
                editor.symbol.clone(),
                editor.condition.clone(),
                price,
            ),
        };
        payload.exchange = editor.exchange.clone();
        payload.symbol = editor.symbol.clone();
        payload.condition = editor.condition.clone();
        payload.original_condition = editor.condition.clone();
        payload.price = price;
        payload.enabled = editor.enabled;
        payload.trigger_mode = editor.trigger_mode.clone();
        payload.play_sound = editor.play_sound;
        payload.sound_repeat = if editor.trigger_mode == "every_time" {
            "Every time".to_string()
        } else {
            "Once".to_string()
        };

        self.submitting = true;
        self.error = None;
        cx.notify();

        let client = self.api_client.clone();
        let rt = tokio_runtime();
        let editing_id = editor.id.clone();
        cx.spawn(async move |this, cx| {
            let result = rt
                .spawn_blocking(move || match editing_id.as_deref() {
                    Some(id) => client.update_alert(id, &payload).map(|_| ()),
                    None => client.create_alert(&payload).map(|_| ()),
                })
                .await;

            let _ = this.update(cx, |panel, cx| {
                panel.submitting = false;
                if panel.api_client.get_session().is_none() {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(())) => {
                        panel.editor = None;
                        panel.error = None;
                        panel.refresh(cx);
                    }
                    Ok(Err(error)) => {
                        panel.error = Some(friendly_error(&error));
                    }
                    Err(error) => {
                        panel.error = Some(format!("Save failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_alert(&mut self, alert: Alert, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }

        let id = alert.id.clone();
        let enabled = !alert.enabled;
        if let Some(existing) = self.alerts.iter_mut().find(|item| item.id == id) {
            existing.enabled = enabled;
        }
        publish_alerts(&self.alerts);

        let mut payload = payload_from_alert(&alert);
        payload.enabled = enabled;

        self.submitting = true;
        cx.notify();

        let client = self.api_client.clone();
        let rt = tokio_runtime();
        cx.spawn(async move |this, cx| {
            let result = rt
                .spawn_blocking(move || client.update_alert(&id, &payload))
                .await;

            let _ = this.update(cx, |panel, cx| {
                panel.submitting = false;
                if panel.api_client.get_session().is_none() {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(updated)) => {
                        if let Some(existing) =
                            panel.alerts.iter_mut().find(|item| item.id == updated.id)
                        {
                            *existing = updated;
                        }
                        publish_alerts(&panel.alerts);
                        panel.error = None;
                    }
                    Ok(Err(error)) => {
                        panel.error = Some(friendly_error(&error));
                        panel.refresh(cx);
                    }
                    Err(error) => {
                        panel.error = Some(format!("Update failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn request_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.pending_delete = Some(id.to_string());
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        self.pending_delete = None;
        cx.notify();
    }

    fn confirm_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }

        let id = id.to_string();
        self.submitting = true;
        self.pending_delete = None;
        cx.notify();

        let client = self.api_client.clone();
        let rt = tokio_runtime();
        let delete_id = id.clone();
        cx.spawn(async move |this, cx| {
            let result = rt
                .spawn_blocking(move || client.delete_alert(&delete_id))
                .await;

            let _ = this.update(cx, |panel, cx| {
                panel.submitting = false;
                if panel.api_client.get_session().is_none() {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(())) => {
                        panel.alerts.retain(|alert| alert.id != id);
                        publish_alerts(&panel.alerts);
                        if panel
                            .editor
                            .as_ref()
                            .and_then(|editor| editor.id.as_deref())
                            == Some(id.as_str())
                        {
                            panel.editor = None;
                        }
                        panel.error = None;
                    }
                    Ok(Err(error)) => {
                        panel.error = Some(friendly_error(&error));
                    }
                    Err(error) => {
                        panel.error = Some(format!("Delete failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_logs(&mut self, cx: &mut Context<Self>) {
        if self.logs_loading {
            return;
        }

        self.logs_loading = true;
        cx.notify();

        let client = self.api_client.clone();
        let rt = tokio_runtime();
        cx.spawn(async move |this, cx| {
            let result = rt.spawn_blocking(move || client.clear_alert_logs()).await;

            let _ = this.update(cx, |panel, cx| {
                panel.logs_loading = false;
                if panel.api_client.get_session().is_none() {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Ok(())) => {
                        panel.logs.clear();
                        panel.last_seen_log_id = None;
                        panel.error = None;
                    }
                    Ok(Err(error)) => {
                        panel.error = Some(friendly_error(&error));
                    }
                    Err(error) => {
                        panel.error = Some(format!("Clear failed: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Flush queued trigger notifications. Called by the parent view every
    /// frame so alerts still surface after the sidebar is closed.
    pub fn drain_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_notifications.is_empty() {
            return;
        }

        let take = self.pending_notifications.len().min(3);
        let notifications: Vec<AlertLog> = self.pending_notifications.drain(0..take).collect();
        for log in notifications {
            let style = condition_style(log.display_condition(), cx);
            let message = format!("{} {} {}", log.symbol, style.glyph, format_price(log.price));
            let notification = match log.display_condition() {
                "crossing_down" | "less_than" | "below" => Notification::warning(message),
                _ => Notification::success(message),
            };
            window.defer(cx, move |window, cx| {
                window.push_notification(notification.title("Price alert triggered"), cx);
            });
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let signed_in = self.api_client.get_session().is_some();
        let live = self.alerts.iter().filter(|alert| alert.is_active()).count();

        h_flex()
            .w_full()
            .h(px(32.0))
            .px(px(12.0))
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(colors.border.opacity(0.5))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.muted_foreground)
                            .child("SIGNAL DESK"),
                    )
                    .when(signed_in && live > 0, |this| {
                        this.child(
                            h_flex()
                                .items_center()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .w(px(5.0))
                                        .h(px(5.0))
                                        .rounded_full()
                                        .bg(colors.bullish),
                                )
                                .child(
                                    div()
                                        .text_size(px(9.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(colors.bullish)
                                        .child(live.to_string()),
                                ),
                        )
                    }),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        Button::new("alerts-new")
                            .ghost()
                            .xsmall()
                            .w(px(22.0))
                            .h(px(22.0))
                            .p(px(0.0))
                            .icon(Icon::default().path("icons/plus.svg").size_4())
                            .tooltip("New alert")
                            .disabled(!signed_in || self.submitting)
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.open_editor_create(window, cx);
                            })),
                    )
                    .child(
                        Button::new("alerts-refresh")
                            .ghost()
                            .xsmall()
                            .w(px(22.0))
                            .h(px(22.0))
                            .p(px(0.0))
                            .icon(Icon::default().path("icons/refresh-cw.svg").size_4())
                            .tooltip("Refresh alerts")
                            .disabled(!signed_in || self.loading || self.submitting)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.refresh(cx);
                            })),
                    ),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        h_flex()
            .w_full()
            .h(px(30.0))
            .px(px(12.0))
            .gap(px(16.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border.opacity(0.5))
            .child(self.render_tab_button(
                "alerts-tab-alerts",
                "Alerts",
                self.alerts.len(),
                AlertsTab::Alerts,
                cx,
            ))
            .child(self.render_tab_button(
                "alerts-tab-history",
                "History",
                self.logs.len(),
                AlertsTab::History,
                cx,
            ))
    }

    fn render_tab_button(
        &self,
        id: &'static str,
        label: &'static str,
        count: usize,
        tab: AlertsTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let active = self.active_tab == tab;
        let foreground = if active {
            colors.foreground
        } else {
            colors.muted_foreground
        };
        let count_color = if active {
            colors.primary
        } else {
            colors.muted_foreground.opacity(0.7)
        };

        div()
            .id(id)
            .h_full()
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .border_b_2()
            .border_color(if active {
                colors.primary
            } else {
                gpui::transparent_black()
            })
            .text_size(px(10.0))
            .font_weight(if active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(foreground)
            .child(label)
            .when(count > 0, |this| {
                this.child(
                    div()
                        .text_size(px(9.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(count_color)
                        .child(count.to_string()),
                )
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.active_tab = tab;
                this.editor = None;
                this.pending_delete = None;
                cx.notify();
            }))
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let active = self.alerts.iter().filter(|alert| alert.is_active()).count();
        let paused = self.alerts.len().saturating_sub(active);

        h_flex()
            .w_full()
            .h(px(26.0))
            .px(px(12.0))
            .gap(px(10.0))
            .items_center()
            .border_b_1()
            .border_color(colors.border.opacity(0.4))
            .child(render_stat("LIVE", active, colors.bullish, colors))
            .child(render_stat(
                "PAUSED",
                paused,
                colors.muted_foreground,
                colors,
            ))
            .child(render_stat("FIRED", self.logs.len(), colors.info, colors))
    }

    fn render_content(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.api_client.get_session().is_none() {
            return self.render_signed_out(cx).into_any_element();
        }

        if self.loading && self.alerts.is_empty() && self.logs.is_empty() {
            return self.render_loading(cx).into_any_element();
        }

        if self.alerts.is_empty() {
            if let Some(error) = &self.error {
                return self.render_error(error, cx).into_any_element();
            }
        }

        let body = match self.active_tab {
            AlertsTab::Alerts => self.render_alerts_list(cx).into_any_element(),
            AlertsTab::History => self.render_history_list(cx).into_any_element(),
        };

        if let Some(error) = &self.error {
            return v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_error_banner(error, cx))
                .child(body)
                .into_any_element();
        }

        body
    }

    fn render_error_banner(&self, error: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        h_flex()
            .mx(px(10.0))
            .my(px(6.0))
            .p(px(6.0))
            .gap(px(5.0))
            .items_center()
            .rounded(px(4.0))
            .border_1()
            .border_color(colors.danger.opacity(0.3))
            .bg(colors.danger.opacity(0.1))
            .child(
                Icon::default()
                    .path("icons/triangle-alert.svg")
                    .size_3()
                    .text_color(colors.danger),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(9.0))
                    .text_color(colors.danger)
                    .child(error.to_string()),
            )
    }

    fn render_alerts_list(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.alerts.is_empty() {
            return self
                .render_empty(
                    "icons/bell.svg",
                    "No alerts yet",
                    "Set a price target and get pinged the moment it hits.",
                    Some(("Create first alert", "alerts-empty-create")),
                    cx,
                )
                .into_any_element();
        }

        let mut list = v_flex()
            .id("alerts-list-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb(px(4.0));

        for alert in &self.alerts {
            list = list.child(self.render_alert_row(alert, cx));
        }

        list.into_any_element()
    }

    fn render_alert_row(&self, alert: &Alert, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let style = condition_style(alert.display_condition(), cx);
        let (base, quote) = split_symbol(&alert.symbol);
        let price = format_price(alert.price);
        let expired = alert.is_expired();
        let active = alert.is_active();
        let pending = self.pending_delete.as_deref() == Some(alert.id.as_str());
        let row_id = SharedString::from(format!("alert-row-{}", alert.id));
        let status_color = if expired {
            colors.warning
        } else if active {
            colors.bullish
        } else {
            colors.muted_foreground
        };
        let status_label = if expired {
            "EXPIRED"
        } else if active {
            "LIVE"
        } else {
            "PAUSED"
        };

        let actions = if pending {
            h_flex()
                .gap(px(2.0))
                .items_center()
                .child(
                    div()
                        .text_size(px(9.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.danger)
                        .child("DELETE?"),
                )
                .child(icon_button(
                    SharedString::from(format!("confirm-delete-{}", alert.id)),
                    "icons/check.svg",
                    "Confirm delete",
                    colors.danger,
                    {
                        let id = alert.id.clone();
                        cx.listener(move |this, _event, _window, cx| {
                            this.confirm_delete(&id, cx);
                        })
                    },
                ))
                .child(icon_button(
                    SharedString::from(format!("cancel-delete-{}", alert.id)),
                    "icons/close.svg",
                    "Cancel",
                    colors.muted_foreground,
                    cx.listener(|this, _event, _window, cx| {
                        this.cancel_delete(cx);
                    }),
                ))
        } else {
            let toggle_icon = if alert.enabled {
                "icons/pause.svg"
            } else {
                "icons/play.svg"
            };
            h_flex()
                .gap(px(2.0))
                .items_center()
                .opacity(0.55)
                .group_hover(row_id.clone(), |style| style.opacity(1.0))
                .child(icon_button(
                    SharedString::from(format!("toggle-{}", alert.id)),
                    toggle_icon,
                    if alert.enabled {
                        "Pause alert"
                    } else {
                        "Resume alert"
                    },
                    colors.muted_foreground,
                    {
                        let alert = alert.clone();
                        cx.listener(move |this, _event, _window, cx| {
                            this.toggle_alert(alert.clone(), cx);
                        })
                    },
                ))
                .child(icon_button(
                    SharedString::from(format!("edit-{}", alert.id)),
                    "icons/pencil.svg",
                    "Edit alert",
                    colors.muted_foreground,
                    {
                        let alert = alert.clone();
                        cx.listener(move |this, _event, window, cx| {
                            this.open_editor_edit(alert.clone(), window, cx);
                        })
                    },
                ))
                .child(icon_button(
                    SharedString::from(format!("delete-{}", alert.id)),
                    "icons/delete.svg",
                    "Delete alert",
                    colors.muted_foreground,
                    {
                        let id = alert.id.clone();
                        cx.listener(move |this, _event, _window, cx| {
                            this.request_delete(&id, cx);
                        })
                    },
                ))
        };

        h_flex()
            .id(row_id.clone())
            .group(row_id)
            .w_full()
            .h(px(46.0))
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(8.0))
            .border_b_1()
            .border_color(colors.border.opacity(0.35))
            .hover(|style| style.bg(colors.list_hover))
            .child(div().w(px(2.0)).h_full().bg(if active {
                style.color
            } else {
                style.color.opacity(0.35)
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.0))
                    .child(
                        h_flex()
                            .w_full()
                            .items_baseline()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .max_w(px(96.0))
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .child(base),
                            )
                            .when(!quote.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(9.0))
                                        .text_color(colors.muted_foreground)
                                        .child(quote),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(style.color)
                                    .child(style.glyph),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .max_w(px(92.0))
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_family(mono_font.clone())
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(style.color)
                                    .child(price),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .max_w(px(64.0))
                                    .truncate()
                                    .text_size(px(9.0))
                                    .text_color(colors.muted_foreground.opacity(0.8))
                                    .child(alert.exchange.to_uppercase()),
                            )
                            .child(render_status_pill(
                                status_label,
                                status_color,
                                active && !expired,
                            ))
                            .when(alert.is_repeating(), |this| {
                                this.child(
                                    div().text_size(px(9.0)).text_color(colors.info).child("↻"),
                                )
                            })
                            .child(div().flex_1())
                            .child(actions),
                    ),
            )
    }

    fn render_history_list(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors;

        let header = h_flex()
            .w_full()
            .h(px(26.0))
            .px(px(12.0))
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(colors.border.opacity(0.4))
            .child(
                div()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child(format!("{} TRIGGERS", self.logs.len())),
            )
            .when(!self.logs.is_empty(), |this| {
                this.child(
                    Button::new("alerts-clear-history")
                        .ghost()
                        .xsmall()
                        .label("Clear")
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.clear_logs(cx);
                        })),
                )
            });

        let body = if self.logs.is_empty() {
            self.render_empty(
                "icons/history.svg",
                "No triggers yet",
                "When a target fires, it lands here with the exact price.",
                None,
                cx,
            )
            .into_any_element()
        } else {
            let mut list = v_flex()
                .id("alerts-history-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .pb(px(4.0));

            for log in &self.logs {
                list = list.child(self.render_history_row(log, cx));
            }
            list.into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_history_row(&self, log: &AlertLog, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let style = condition_style(log.display_condition(), cx);
        let (base, quote) = split_symbol(&log.symbol);

        h_flex()
            .id(SharedString::from(format!("history-row-{}", log.id)))
            .w_full()
            .items_center()
            .gap(px(8.0))
            .px(px(10.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(colors.border.opacity(0.25))
            .hover(|style| style.bg(colors.list_hover))
            .child(
                div()
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(style.color.opacity(0.12))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(style.color)
                            .child(style.glyph),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.0))
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .max_w(px(90.0))
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .child(base),
                            )
                            .when(!quote.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_size(px(9.0))
                                        .text_color(colors.muted_foreground)
                                        .child(quote),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(style.color)
                                    .child(style.glyph),
                            )
                            .child(
                                div()
                                    .max_w(px(86.0))
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_family(mono_font)
                                    .text_color(style.color)
                                    .child(format_price(log.price)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .max_w(px(72.0))
                                    .truncate()
                                    .text_size(px(9.0))
                                    .text_color(colors.muted_foreground.opacity(0.8))
                                    .child(log.exchange.to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(colors.muted_foreground.opacity(0.5))
                                    .child("·"),
                            )
                            .child(
                                div()
                                    .text_size(px(9.0))
                                    .text_color(colors.muted_foreground.opacity(0.5))
                                    .child(format_log_date(&log.triggered_at)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(colors.muted_foreground.opacity(0.6))
                    .child(relative_time(&log.triggered_at)),
            )
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors;
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };
        let editing = editor.id.is_some();
        let pending_delete = editor
            .id
            .as_deref()
            .is_some_and(|id| self.pending_delete.as_deref() == Some(id));
        let mark = TradeStore::global()
            .lock()
            .ok()
            .and_then(|store| store.get_mark_price(&editor.symbol));
        let exchange_select = self.exchange_select.clone();
        let symbol_select = self.symbol_select.clone();
        let price_input = self.price_input.clone();

        let condition_chips = CONDITIONS.iter().copied().map(|(value, label, glyph)| {
            let active = editor.condition == value;
            let style = condition_style(value, cx);
            let value = value.to_string();
            div()
                .id(SharedString::from(format!("alert-condition-{value}")))
                .w(px(108.0))
                .h(px(26.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(if active {
                    style.color.opacity(0.55)
                } else {
                    colors.border.opacity(0.6)
                })
                .bg(if active {
                    style.color.opacity(0.1)
                } else {
                    gpui::transparent_black()
                })
                .cursor_pointer()
                .hover(|this| this.bg(colors.list_hover))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(if active {
                            style.color
                        } else {
                            colors.muted_foreground
                        })
                        .child(glyph),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if active {
                            style.color
                        } else {
                            colors.muted_foreground
                        })
                        .child(label),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    if let Some(editor) = &mut this.editor {
                        editor.condition = value.clone();
                    }
                    cx.notify();
                }))
        });

        let trigger_once = editor.trigger_mode != "every_time";
        let trigger_repeat = !trigger_once;

        let header = h_flex()
            .w_full()
            .h(px(32.0))
            .px(px(12.0))
            .items_center()
            .gap(px(6.0))
            .border_b_1()
            .border_color(colors.border.opacity(0.5))
            .child(icon_button(
                "alerts-editor-back",
                "icons/chevron-left.svg",
                "Back to alerts",
                colors.muted_foreground,
                cx.listener(|this, _event, _window, cx| {
                    this.close_editor(cx);
                }),
            ))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child(if editing { "EDIT ALERT" } else { "NEW ALERT" }),
            )
            .child(div().flex_1())
            .when(editing, |this| {
                let id = editor.id.clone().unwrap_or_default();
                this.child(icon_button(
                    "alerts-editor-delete",
                    "icons/delete.svg",
                    "Delete alert",
                    colors.danger,
                    cx.listener(move |this, _event, _window, cx| {
                        this.request_delete(&id, cx);
                    }),
                ))
            });

        let condition_field = v_flex()
            .w_full()
            .gap(px(4.0))
            .child(field_label("CONDITION", colors.muted_foreground))
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(4.0))
                    .children(condition_chips),
            );

        let mark_hint = mark.map(|mark| {
            let value = price_input_value(mark);
            h_flex()
                .w_full()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(colors.muted_foreground.opacity(0.8))
                        .child(format!("MARK {}", format_price(mark))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("alerts-use-mark")
                        .text_size(px(9.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.info)
                        .cursor_pointer()
                        .child("USE")
                        .on_click(cx.listener(move |this, _event, window, cx| {
                            if let Some(input) = &this.price_input {
                                input.update(cx, |state, cx| {
                                    state.set_value(&value, window, cx);
                                });
                            }
                            if let Some(editor) = &mut this.editor {
                                editor.price = value.clone();
                            }
                            cx.notify();
                        })),
                )
        });

        v_flex()
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                v_flex()
                    .id("alerts-editor-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(12.0))
                    .py(px(10.0))
                    .gap(px(12.0))
                    .when(pending_delete, |this| {
                        let id = editor.id.clone().unwrap_or_default();
                        this.child(
                            h_flex()
                                .w_full()
                                .p(px(6.0))
                                .gap(px(6.0))
                                .items_center()
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(colors.danger.opacity(0.3))
                                .bg(colors.danger.opacity(0.1))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(9.0))
                                        .text_color(colors.danger)
                                        .child("Delete this alert?"),
                                )
                                .child(
                                    Button::new("alerts-editor-confirm-delete")
                                        .danger()
                                        .xsmall()
                                        .label("Delete")
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            this.confirm_delete(&id, cx);
                                        })),
                                )
                                .child(
                                    Button::new("alerts-editor-keep")
                                        .ghost()
                                        .xsmall()
                                        .label("Keep")
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            this.cancel_delete(cx);
                                        })),
                                ),
                        )
                    })
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(4.0))
                            .child(field_label("EXCHANGE", colors.muted_foreground))
                            .when_some(exchange_select, |this, select| {
                                this.child(Select::new(&select).small().w_full())
                            }),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(4.0))
                            .child(field_label("SYMBOL", colors.muted_foreground))
                            .when_some(symbol_select, |this, select| {
                                this.child(Select::new(&select).small().w_full())
                            }),
                    )
                    .child(condition_field)
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(4.0))
                            .child(field_label("TARGET PRICE", colors.muted_foreground))
                            .when_some(price_input, |this, input| {
                                this.child(Input::new(&input).small().w_full())
                            })
                            .when_some(mark_hint, |this, hint| this.child(hint)),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap(px(4.0))
                            .child(field_label("TRIGGER", colors.muted_foreground))
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap(px(4.0))
                                    .child(render_mode_chip(
                                        "alerts-trigger-once",
                                        "Once",
                                        trigger_once,
                                        colors,
                                        cx.listener(|this, _event, _window, cx| {
                                            if let Some(editor) = &mut this.editor {
                                                editor.trigger_mode = "once".to_string();
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(render_mode_chip(
                                        "alerts-trigger-repeat",
                                        "Repeat",
                                        trigger_repeat,
                                        colors,
                                        cx.listener(|this, _event, _window, cx| {
                                            if let Some(editor) = &mut this.editor {
                                                editor.trigger_mode = "every_time".to_string();
                                            }
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap(px(1.0))
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.foreground)
                                            .child("Play sound"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.0))
                                            .text_color(colors.muted_foreground)
                                            .child("Chime when this target fires"),
                                    ),
                            )
                            .child(
                                Switch::new("alerts-play-sound")
                                    .checked(editor.play_sound)
                                    .small()
                                    .on_click(cx.listener(|this, checked, _window, cx| {
                                        if let Some(editor) = &mut this.editor {
                                            editor.play_sound = *checked;
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .w_full()
                                .p(px(6.0))
                                .rounded(px(4.0))
                                .bg(colors.danger.opacity(0.1))
                                .border_1()
                                .border_color(colors.danger.opacity(0.3))
                                .text_size(px(9.0))
                                .text_color(colors.danger)
                                .child(error),
                        )
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .px(px(12.0))
                    .py(px(8.0))
                    .gap(px(6.0))
                    .border_t_1()
                    .border_color(colors.border.opacity(0.4))
                    .child(
                        Button::new("alerts-editor-cancel")
                            .outline()
                            .small()
                            .flex_1()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.close_editor(cx);
                            })),
                    )
                    .child(
                        Button::new("alerts-editor-save")
                            .primary()
                            .small()
                            .flex_1()
                            .label(if editing { "Update" } else { "Create" })
                            .disabled(self.submitting)
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.save_editor(cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_signed_out(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .px(px(20.0))
            .text_center()
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(colors.border.opacity(0.6))
                    .bg(colors.sidebar_accent)
                    .child(
                        Icon::default()
                            .path("icons/bell.svg")
                            .size_5()
                            .text_color(colors.muted_foreground),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child("Alerts live in your account"),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.muted_foreground)
                    .child("Sign in to sync price targets with CoinWatch."),
            )
            .child(
                Button::new("alerts-sign-in")
                    .primary()
                    .small()
                    .label("Sign in")
                    .on_click(cx.listener(|_this, _event, _window, cx| {
                        cx.emit(AlertsPanelEvent::RequestLogin);
                    })),
            )
    }

    fn render_loading(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        v_flex()
            .flex_1()
            .child(
                h_flex()
                    .w_full()
                    .h(px(30.0))
                    .px(px(12.0))
                    .items_center()
                    .gap(px(6.0))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.4))
                    .child(
                        gpui_component::spinner::Spinner::new()
                            .small()
                            .color(colors.muted_foreground),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.muted_foreground)
                            .child("SYNCING ALERTS"),
                    ),
            )
            .children((0..3).map(|_| {
                h_flex()
                    .w_full()
                    .h(px(46.0))
                    .px(px(10.0))
                    .items_center()
                    .gap(px(8.0))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.25))
                    .child(
                        div()
                            .w(px(2.0))
                            .h_full()
                            .bg(colors.muted_foreground.opacity(0.2)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .h(px(8.0))
                                    .w(px(80.0))
                                    .rounded(px(3.0))
                                    .bg(colors.muted_foreground.opacity(0.2)),
                            )
                            .child(
                                div()
                                    .h(px(6.0))
                                    .w(px(130.0))
                                    .rounded(px(3.0))
                                    .bg(colors.muted_foreground.opacity(0.12)),
                            ),
                    )
            }))
    }

    fn render_error(&self, error: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(20.0))
            .text_center()
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(colors.warning.opacity(0.3))
                    .bg(colors.warning.opacity(0.1))
                    .child(
                        Icon::default()
                            .path("icons/triangle-alert.svg")
                            .size_5()
                            .text_color(colors.warning),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child("Could not sync alerts"),
            )
            .child(
                div()
                    .text_size(px(9.0))
                    .line_height(px(14.0))
                    .text_color(colors.muted_foreground)
                    .child(error.to_string()),
            )
            .child(
                Button::new("alerts-retry")
                    .outline()
                    .xsmall()
                    .label("Retry")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh(cx);
                    })),
            )
    }

    fn render_empty(
        &self,
        icon: &'static str,
        title: &'static str,
        body: &'static str,
        action: Option<(&'static str, &'static str)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(20.0))
            .text_center()
            .child(
                div()
                    .w(px(44.0))
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(10.0))
                    .border_1()
                    .border_dashed()
                    .border_color(colors.border.opacity(0.8))
                    .bg(colors.sidebar_accent.opacity(0.25))
                    .child(
                        Icon::default()
                            .path(icon)
                            .size_5()
                            .text_color(colors.muted_foreground.opacity(0.7)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.muted_foreground)
                    .child(body),
            )
            .when_some(action, |this, (label, id)| {
                this.child(
                    Button::new(id)
                        .outline()
                        .xsmall()
                        .label(label)
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.open_editor_create(window, cx);
                        })),
                )
            })
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let signed_in = self.api_client.get_session().is_some();
        let (label, color) = if !signed_in {
            ("SIGNED OUT", colors.muted_foreground)
        } else if self.loading {
            ("SYNCING", colors.warning)
        } else if self.error.is_some() {
            ("SYNC ERROR", colors.danger)
        } else {
            ("LIVE SYNC", colors.bullish)
        };

        h_flex()
            .w_full()
            .h(px(26.0))
            .px(px(10.0))
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(colors.border.opacity(0.4))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        Icon::default()
                            .path("icons/bell.svg")
                            .xsmall()
                            .text_color(colors.muted_foreground),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(colors.muted_foreground)
                            .child(format!("{} alerts", self.alerts.len())),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(color))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color)
                            .child(label),
                    ),
            )
    }
}

impl Render for AlertsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_symbol_items(window, cx);
        self.drain_notifications(window, cx);

        let colors = cx.theme().colors;
        let signed_in = self.api_client.get_session().is_some();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(colors.sidebar)
            .text_color(colors.foreground)
            .child(self.render_toolbar(cx))
            .when(signed_in, |this| this.child(self.render_tabs(cx)))
            .when(signed_in, |this| this.child(self.render_summary(cx)))
            .child(if self.editor.is_some() {
                self.render_editor(cx)
            } else {
                self.render_content(cx)
            })
            .child(self.render_footer(cx))
    }
}

fn icon_button(
    id: impl Into<ElementId>,
    icon: &'static str,
    tooltip: &'static str,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .p(px(0.0))
        .w(px(20.0))
        .h(px(20.0))
        .icon(Icon::default().path(icon).size_3().text_color(color))
        .tooltip(tooltip)
        .on_click(on_click)
}

/// Mirror the sidebar's alert snapshot into the process-wide store so chart
/// panes can paint the same levels without holding an `AlertsPanel` entity.
fn publish_alerts(alerts: &[Alert]) {
    if let Ok(mut store) = AlertStore::global().lock() {
        store.set_alerts(alerts.to_vec());
    }
}

fn clear_published_alerts() {
    if let Ok(mut store) = AlertStore::global().lock() {
        store.clear();
    }
}

fn field_label(label: &'static str, color: Hsla) -> impl IntoElement {
    div()
        .text_size(px(9.0))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(label)
}

fn render_stat(
    label: &'static str,
    value: usize,
    color: Hsla,
    colors: gpui_component::theme::ThemeColor,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap(px(4.0))
        .child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(color))
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color)
                .child(value.to_string()),
        )
        .child(
            div()
                .text_size(px(8.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.muted_foreground)
                .child(label),
        )
}

fn render_status_pill(label: &'static str, color: Hsla, live: bool) -> impl IntoElement {
    h_flex()
        .gap(px(3.0))
        .items_center()
        .when(live, |this| {
            this.child(div().w(px(4.0)).h(px(4.0)).rounded_full().bg(color))
        })
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(label),
        )
}

fn render_mode_chip(
    id: &'static str,
    label: &'static str,
    active: bool,
    colors: gpui_component::theme::ThemeColor,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .h(px(26.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .border_1()
        .border_color(if active {
            colors.primary.opacity(0.55)
        } else {
            colors.border.opacity(0.6)
        })
        .bg(if active {
            colors.primary.opacity(0.1)
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .hover(|this| this.bg(colors.list_hover))
        .text_size(px(9.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active {
            colors.primary
        } else {
            colors.muted_foreground
        })
        .child(label)
        .on_click(on_click)
}

#[derive(Clone, Copy)]
struct ConditionStyle {
    glyph: &'static str,
    color: Hsla,
}

fn condition_style(condition: &str, cx: &App) -> ConditionStyle {
    let colors = cx.theme().colors;
    match condition {
        "crossing_up" | "greater_than" | "above" => ConditionStyle {
            glyph: "↗",
            color: colors.bullish,
        },
        "crossing_down" | "less_than" | "below" => ConditionStyle {
            glyph: "↘",
            color: colors.bearish,
        },
        _ => ConditionStyle {
            glyph: "⇄",
            color: colors.info,
        },
    }
}

fn payload_from_alert(alert: &Alert) -> AlertPayload {
    AlertPayload {
        exchange: alert.exchange.clone(),
        symbol: alert.symbol.clone(),
        condition: alert.condition.clone(),
        original_condition: alert.display_condition().to_string(),
        price: alert.price,
        enabled: alert.enabled,
        trigger_mode: alert
            .trigger_mode
            .clone()
            .unwrap_or_else(|| "once".to_string()),
        expiration: alert.expiration.clone(),
        message: alert.message.clone(),
        notify_in_app: alert.notify_in_app.unwrap_or(true),
        show_toast: alert.show_toast.unwrap_or(true),
        send_email: alert.send_email.unwrap_or(false),
        webhook_enabled: alert.webhook_enabled.unwrap_or(false),
        webhook_url: alert.webhook_url.clone(),
        play_sound: alert.play_sound.unwrap_or(true),
        sound_style: alert
            .sound_style
            .clone()
            .unwrap_or_else(|| "Thin".to_string()),
        sound_repeat: alert
            .sound_repeat
            .clone()
            .unwrap_or_else(|| "Once".to_string()),
    }
}

fn friendly_error(error: &CoinWatchError) -> String {
    match error {
        CoinWatchError::Auth(_) => "Session expired. Sign in again.".to_string(),
        CoinWatchError::Api { status, .. } if *status == 401 => {
            "Session expired. Sign in again.".to_string()
        }
        _ => error.to_string(),
    }
}

fn split_symbol(symbol: &str) -> (String, String) {
    if let Some((base, quote)) = symbol.split_once('/') {
        return (base.to_string(), quote.to_string());
    }

    let upper = symbol.to_uppercase();
    for quote in ["USDT", "USDC", "BUSD", "USD", "BTC", "ETH", "BNB"] {
        if upper.ends_with(quote) && upper.len() > quote.len() {
            let base = upper[..upper.len() - quote.len()].to_string();
            return (base, quote.to_string());
        }
    }

    (upper, String::new())
}

fn parse_price_input(text: &str) -> Option<f64> {
    let cleaned: String = text
        .chars()
        .filter(|character| !character.is_whitespace() && *character != ',' && *character != '$')
        .collect();
    cleaned
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn format_price(price: f64) -> String {
    if !price.is_finite() {
        return "—".to_string();
    }

    let decimals = if price >= 1_000_000.0 {
        0
    } else if price >= 1_000.0 {
        2
    } else if price >= 1.0 {
        4
    } else {
        8
    };

    let mut text = format!("{:.*}", decimals, price);
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }

    group_thousands(&text)
}

fn price_input_value(price: f64) -> String {
    let mut text = format!("{:.8}", price);
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn group_thousands(raw: &str) -> String {
    let (integer, fraction) = match raw.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (raw, None),
    };

    let negative = integer.starts_with('-');
    let digits = integer.trim_start_matches('-');
    let mut grouped = String::new();
    for (index, character) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(character);
    }

    let mut result: String = grouped.chars().rev().collect();
    if negative {
        result.insert(0, '-');
    }
    if let Some(fraction) = fraction {
        result.push('.');
        result.push_str(fraction);
    }
    result
}

fn relative_time(iso: &str) -> String {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return "—".to_string();
    };
    let seconds = chrono::Utc::now()
        .signed_duration_since(when.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);

    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn format_log_date(iso: &str) -> String {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return "—".to_string();
    };
    when.with_timezone(&chrono::Local)
        .format("%b %d · %H:%M")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_grouped_and_trimmed() {
        assert_eq!(format_price(65_000.5), "65,000.5");
        assert_eq!(format_price(3_500.0), "3,500");
        assert_eq!(format_price(0.00012345), "0.00012345");
    }

    #[test]
    fn symbols_split_with_and_without_slash() {
        assert_eq!(split_symbol("BTC/USDT"), ("BTC".into(), "USDT".into()));
        assert_eq!(split_symbol("ETHUSDT"), ("ETH".into(), "USDT".into()));
    }

    #[test]
    fn relative_time_handles_bad_input() {
        assert_eq!(relative_time("not-a-date"), "—");
    }

    #[test]
    fn price_input_accepts_common_formats() {
        assert_eq!(parse_price_input("65000"), Some(65_000.0));
        assert_eq!(parse_price_input("65,000.50"), Some(65_000.5));
        assert_eq!(parse_price_input("$ 3 500"), Some(3_500.0));
        assert_eq!(parse_price_input("0"), None);
        assert_eq!(parse_price_input(""), None);
        assert_eq!(parse_price_input("not a price"), None);
    }
}
