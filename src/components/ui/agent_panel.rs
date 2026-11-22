//! AI Agent module: analysis of the active dashboards plus agentic trade execution.
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::{h_flex, v_flex};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::Sizable;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::text::TextView;
use serde_json::Value;

use crate::components::ui::agent_chart::AgentChart;
use crate::data::agent::{ChartRequest, MessageSegment, TradeAction, TradeActionResult};
use crate::data::pane_control::UiActionResult;

/// Trim a model id so it fits the selector button. The full id stays available
/// in the tooltip and the dropdown. `Button::label` is rendered with
/// `flex_none`, so layout-level truncation cannot clamp it; the string itself
/// has to be short.
fn short_model_label(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_string();
    }
    let mut shortened: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

#[derive(Clone)]
struct ChatMessage {
    prompt: String,
    response: String,
    failed: bool,
    typing: bool,
    timestamp: Option<String>,
    /// Receipts for the trade directives in `response`, produced exactly once
    /// when the reply arrives and persisted so renders/reloads never re-execute.
    trade_receipts: Vec<TradeActionResult>,
    /// Receipts for the pane-grid (`ui`) directives in `response`, produced once
    /// on arrival and persisted so reloading never re-applies a layout change.
    ui_receipts: Vec<UiActionResult>,
    /// Receipts for workspace (`terminal`) directives, produced once on arrival.
    terminal_receipts: Vec<UiActionResult>,
    /// Receipts for workspace file (`file`) directives, produced once on arrival.
    file_receipts: Vec<UiActionResult>,
}

pub struct AgentPanel { 
    conversation_id: Option<String>,
    conversation_title: String,
    prompt: String,
    response: String,
    history: Vec<ChatMessage>,
    chat_scroll: ScrollHandle,
    context: Value,
    input: Option<Entity<InputState>>,
    models: Vec<String>,
    selected_model: String,
    models_loading: bool,
    discovery_attempted: bool,
    provider: (String, String),
    loading: bool,
    error: Option<String>,
    /// Transient status shown in the "Thinking…" bubble (for example while the
    /// Agent reads workspace files).
    status: Option<String>,
    /// Invalidation counter for in-flight provider requests. Bumped by send and
    /// stop so late replies are discarded.
    request_epoch: u64,
    /// Abort handle for the provider request currently in flight.
    in_flight: Option<tokio::task::AbortHandle>,
    /// Full reply text while the typing animation runs, so Stop can reveal it.
    typing_target: Option<String>,
    width: f32,
    height: f32,
    dashboard_count: usize,
    module_count: usize,
    context_bytes: usize,
    /// Inline chart entities keyed by message + segment + market identity.
    chart_views: std::collections::HashMap<String, Entity<AgentChart>>,
    /// Id of the dashboard this pane lives in. Used as the default target for
    /// `ui` directives that omit their `dashboard` selector.
    dashboard_id: Option<String>,
    /// Pane-grid id of this Agent module. Layout actions use it to protect the
    /// pane (and its conversation) from being replaced or closed.
    pane_id: Option<String>,
    /// Whether the floating transcript is expanded. Only meaningful for the
    /// workspace overlay instance; pane instances ignore it.
    overlay_bubbles_visible: bool,
    /// Whether the floating composer is shown. Hiding it leaves the compact
    /// "Ask FinTer…" pill in the workspace.
    overlay_composer_visible: bool,
}

impl AgentPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let mut last_credit_rev = u64::MAX;
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                let credit_rev = crate::data::credits::CreditStore::revision();
                if credit_rev != last_credit_rev {
                    last_credit_rev = credit_rev;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        }).detach();

        // When dashboard loads, Agent module starts with an empty new conversation as default.
        Self {
            conversation_id: None,
            conversation_title: String::new(),
            prompt: String::new(),
            response: String::new(),
            history: Vec::new(),
            chat_scroll: ScrollHandle::new(),
            context: Value::Null,
            input: None,
            models: Vec::new(),
            selected_model: crate::data::agent::provider_config()
                .model
                .filter(|model| !model.trim().is_empty())
                .unwrap_or_else(|| "Select model".into()),
            models_loading: false,
            discovery_attempted: false,
            // Seed from the resolved config so the first render does not look
            // like a provider change and needlessly clear the model list.
            provider: {
                let config = crate::data::agent::provider_config();
                (config.base_url, config.api_key)
            },
            loading: false,
            error: None,
            status: None,
            request_epoch: 0,
            in_flight: None,
            typing_target: None,
            width: 300.0,
            height: 200.0,
            dashboard_count: 0,
            module_count: 0,
            context_bytes: 0,
            chart_views: std::collections::HashMap::new(),
            dashboard_id: None,
            pane_id: None,
            overlay_bubbles_visible: true,
            overlay_composer_visible: true,
        }
    }

    /// Records which dashboard this Agent pane belongs to, so layout directives
    /// default to the dashboard the user is actually looking at.
    pub fn set_dashboard_id(&mut self, dashboard_id: Option<String>) {
        self.dashboard_id = dashboard_id;
    }

    pub fn set_pane_id(&mut self, pane_id: Option<String>) {
        self.pane_id = pane_id;
    }

    /// Whether the floating transcript should be painted by the workspace.
    pub fn overlay_bubbles_visible(&self) -> bool {
        self.overlay_bubbles_visible
    }

    /// Whether the floating composer should be painted by the workspace.
    pub fn overlay_composer_visible(&self) -> bool {
        self.overlay_composer_visible
    }

    pub fn set_overlay_bubbles_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.overlay_bubbles_visible != visible {
            self.overlay_bubbles_visible = visible;
            cx.notify();
        }
    }

    pub fn set_overlay_composer_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.overlay_composer_visible != visible {
            self.overlay_composer_visible = visible;
            cx.notify();
        }
    }

    pub fn current_conversation_id(&self) -> Option<String> {
        self.conversation_id.clone()
    }

    pub fn start_new_conversation(&mut self, cx: &mut Context<Self>) {
        if self.loading { return; }
        self.conversation_id = None;
        self.conversation_title.clear();
        self.history.clear();
        self.response.clear();
        self.error = None;
        self.chart_views.clear();
        self.chat_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub fn load_conversation(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.loading { return; }
        if let Some(conv) = crate::data::agent::load_conversation(id) {
            self.conversation_id = Some(conv.id);
            self.conversation_title = conv.title;
            // Saved conversations are display-only: reuse the persisted receipts
            // and never re-execute historical trade directives.
            self.history = conv.messages.into_iter().map(|m| ChatMessage {
                prompt: m.prompt,
                response: m.response,
                failed: m.failed,
                typing: false,
                timestamp: m.timestamp,
                trade_receipts: m.trade_receipts,
                ui_receipts: m.ui_receipts,
                terminal_receipts: m.terminal_receipts,
                file_receipts: m.file_receipts,
            }).collect();
            self.response.clear();
            self.error = None;
            self.chart_views.clear();
            self.chat_scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn persist_active_conversation(&self) {
        let Some(id) = &self.conversation_id else { return; };
        let now = chrono::Local::now();
        let title = if self.conversation_title.trim().is_empty() {
            self.history.first().map(|m| {
                m.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or(&m.prompt).chars().take(40).collect()
            }).unwrap_or_else(|| "Conversation".to_string())
        } else {
            self.conversation_title.clone()
        };
        let record = crate::data::agent::ConversationRecord {
            id: id.clone(),
            title,
            updated_at: now.format("%b %d, %H:%M").to_string(),
            updated_at_ts: now.timestamp_millis(),
            messages: self.history.iter().map(|m| crate::data::agent::ChatMessageRecord {
                prompt: m.prompt.clone(),
                response: m.response.clone(),
                failed: m.failed,
                timestamp: m.timestamp.clone(),
                trade_receipts: m.trade_receipts.clone(),
                ui_receipts: m.ui_receipts.clone(),
                terminal_receipts: m.terminal_receipts.clone(),
                file_receipts: m.file_receipts.clone(),
            }).collect(),
        };
        if let Err(e) = crate::data::agent::save_conversation(&record) {
            tracing::error!("Failed to save conversation: {e}");
        }
    }
    pub fn set_size(&mut self, width: f32, height: f32) { self.width = width; self.height = height; }
    pub fn has_size(&self, width: f32, height: f32) -> bool { (self.width-width).abs()<0.5 && (self.height-height).abs()<0.5 }
    pub fn set_context(&mut self, context: Value) {
        self.dashboard_count = context.get("dashboards").and_then(Value::as_array).map_or(0, Vec::len);
        self.module_count = context.get("dashboards").and_then(Value::as_array).map_or(0, |dashboards| dashboards.iter().map(|d| d.get("market_data").map_or(0, |data| data.as_object().map_or(0, |obj| obj.values().map(|v| v.as_array().map_or(0, Vec::len)).sum()))).sum());
        self.context_bytes = serde_json::to_vec(&context).map_or(0, |v| v.len());
        self.context = context;
    }

    fn chart_key(message_index: usize, segment_index: usize, request: &ChartRequest) -> String {
        format!("{message_index}:{segment_index}:{}", request.cache_key())
    }

    /// Renders the receipt for an applied (or rejected) pane-grid directive.
    fn render_ui_receipt(
        index: usize,
        result: &UiActionResult,
        colors: &gpui_component::theme::ThemeColor,
        mono_font: &SharedString,
    ) -> AnyElement {
        let accent = if result.ok { colors.bullish } else { colors.bearish };
        let status = if result.ok { "APPLIED" } else { "FAILED" };

        let mut header = h_flex()
            .w_full()
            .gap(px(6.0))
            .items_center()
            .flex_wrap()
            .child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .bg(accent.opacity(0.18))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(accent)
                    .child(status),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(result.action.clone()),
            );
        if let Some(dashboard) = result.dashboard.as_ref() {
            header = header.child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(format!("on {dashboard}")),
            );
        }
        if let Some(pane) = result.pane.as_ref() {
            header = header.child(
                div()
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(format!("pane {pane}")),
            );
        }

        v_flex()
            .id(ElementId::Name(format!("agent-ui-{index}").into()))
            .w_full()
            .p(px(8.0))
            .rounded(px(6.0))
            .bg(colors.muted.opacity(0.25))
            .border_1()
            .border_color(accent.opacity(0.35))
            .gap(px(4.0))
            .child(header)
            .child(
                div()
                    .text_xs()
                    .text_color(if result.ok { colors.muted_foreground } else { colors.bearish })
                    .child(result.message.clone()),
            )
            .into_any_element()
    }

    /// Renders the receipt for an executed (or rejected) trade directive.
    fn render_trade_receipt(
        index: usize,
        action: &TradeAction,
        result: Option<&TradeActionResult>,
        colors: &gpui_component::theme::ThemeColor,
        mono_font: &SharedString,
    ) -> AnyElement {
        let ok = result.map(|r| r.ok).unwrap_or(true);
        let accent = if ok { colors.bullish } else { colors.bearish };
        let status = result
            .and_then(|r| r.status.clone())
            .unwrap_or_else(|| if ok { "PENDING".to_string() } else { "REJECTED".to_string() });
        let is_buy = result
            .and_then(|r| r.side.as_deref())
            .map(|side| side == "buy")
            .unwrap_or(false);
        let side_color = if is_buy { colors.bullish } else { colors.bearish };
        let live = result
            .and_then(|r| r.mode.as_deref())
            .map(|mode| mode == "live")
            .unwrap_or_else(|| action.is_live());

        let badge = |label: String, fg: Hsla, bg: Hsla| {
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(3.0))
                .bg(bg)
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label)
        };
        let field = |label: &'static str, value: String| {
            h_flex()
                .gap(px(4.0))
                .items_baseline()
                .child(div().text_xs().text_color(colors.muted_foreground).child(label))
                .child(
                    div()
                        .text_xs()
                        .font_family(mono_font.clone())
                        .text_color(colors.foreground)
                        .child(value),
                )
        };
        let fmt_price = |value: f64| {
            if value >= 1000.0 {
                format!("{value:.2}")
            } else if value >= 1.0 {
                format!("{value:.4}")
            } else {
                format!("{value:.6}")
            }
        };
        let fmt_size = |value: f64| {
            if value >= 1000.0 {
                format!("{value:.1}")
            } else if value >= 1.0 {
                format!("{value:.4}")
            } else {
                format!("{value:.6}")
            }
        };

        let mut header = h_flex().w_full().gap(px(5.0)).items_center().flex_wrap();
        header = header
            .child(badge(
                if live { "LIVE".to_string() } else { "PAPER".to_string() },
                if live { colors.danger } else { colors.primary },
                if live { colors.danger.opacity(0.18) } else { colors.primary.opacity(0.15) },
            ))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child(action.kind_label()),
            );
        if let Some(result) = result {
            if let Some(side) = result.side.as_deref() {
                header = header.child(badge(
                    side.to_ascii_uppercase(),
                    side_color,
                    side_color.opacity(0.18),
                ));
            }
            if let Some(symbol) = result.symbol.as_deref() {
                header = header.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.foreground)
                        .child(symbol.to_string()),
                );
            }
        }
        header = header.child(badge(status.clone(), accent, accent.opacity(0.18)));
        if let Some(order_id) = result.and_then(|r| r.order_id.as_deref()) {
            header = header.child(
                div()
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(format!("#{order_id}")),
            );
        }

        let mut body = v_flex().w_full().gap(px(3.0));
        match result {
            Some(result) if result.ok => {
                let mut row = h_flex().w_full().gap(px(12.0)).flex_wrap();
                if let Some(price) = result.price {
                    row = row.child(field("Fill", format!("${}", fmt_price(price))));
                }
                if let Some(quantity) = result.quantity {
                    row = row.child(field("Qty", fmt_size(quantity)));
                }
                if let Some(notional) = result.notional {
                    row = row.child(field("Notional", format!("${notional:.2}")));
                }
                if let Some(leverage) = result.leverage.filter(|l| *l > 1) {
                    row = row.child(field("Lev", format!("{leverage}x")));
                }
                if let Some(pnl) = result.pnl {
                    row = row.child(
                        h_flex()
                            .gap(px(4.0))
                            .items_baseline()
                            .child(div().text_xs().text_color(colors.muted_foreground).child("PnL"))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .text_color(if pnl >= 0.0 { colors.bullish } else { colors.bearish })
                                    .child(format!("{}{:.2}", if pnl >= 0.0 { "+" } else { "" }, pnl)),
                            ),
                    );
                }
                body = body.child(row);
            }
            Some(result) => {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(colors.bearish)
                        .child(result.message.clone()),
                );
            }
            None => {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child("Executing…"),
                );
            }
        }

        v_flex()
            .id(ElementId::Name(format!("agent-trade-{index}").into()))
            .w_full()
            .p(px(8.0))
            .rounded(px(6.0))
            .bg(colors.muted.opacity(0.25))
            .border_1()
            .border_color(accent.opacity(0.35))
            .gap(px(4.0))
            .child(header)
            .child(body)
            .into_any_element()
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() || self.loading {
            return;
        }
        let model = self.selected_model.clone();
        if self.selected_model == "Select model" {
            self.error = Some(
                "Select a model first; use Refresh models to discover provider models.".into(),
            );
            cx.notify();
            return;
        }
        let fallbacks = crate::data::agent::fallback_models(&self.models, &model);
        if self.conversation_id.is_none() {
            let id = uuid::Uuid::new_v4().to_string();
            self.conversation_id = Some(id);
            self.conversation_title = prompt
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(&prompt)
                .chars()
                .take(40)
                .collect();
        }
        let mut context = self.context.clone();
        self.loading = true;
        self.error = None;
        self.status = None;
        self.typing_target = None;
        self.in_flight = None;
        self.request_epoch = self.request_epoch.wrapping_add(1);
        let epoch = self.request_epoch;
        // Tell the model which surface is asking. The shared snapshot is built
        // by MainView, so caller identity is attached here, per conversation.
        if let Some(root) = context.as_object_mut() {
            root.insert(
                "caller".to_string(),
                serde_json::json!({
                    "dashboard": self.dashboard_id.clone(),
                    "pane": self.pane_id.clone(),
                    "surface": if self.dashboard_id.is_none() { "workspace_overlay" } else { "pane" },
                }),
            );
        }
        // Captured before the async task: layout and workspace directives need
        // to know which dashboard/pane is asking, even if the panel moves.
        let caller_dashboard = self.dashboard_id.clone();
        let caller_pane = self.pane_id.clone();
        // Clear the visible editor immediately on every accepted submission,
        // rather than waiting for the asynchronous response to arrive.
        if let Some(input) = self.input.as_ref() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.prompt.clear();
        // A reply that is not visible is not useful. Re-open the floating
        // transcript as soon as a message is accepted from the overlay.
        self.overlay_bubbles_visible = true;

        // Create the conversation pair immediately so the question and a
        // "Thinking…" placeholder bubble appear instantly without waiting for
        // network turnaround.
        let message_index = self.history.len();
        self.history.push(ChatMessage {
            prompt: prompt.clone(),
            response: String::new(),
            failed: false,
            typing: true,
            timestamp: Some(chrono::Local::now().format("%b %d, %H:%M").to_string()),
            trade_receipts: Vec::new(),
            ui_receipts: Vec::new(),
            terminal_receipts: Vec::new(),
            file_receipts: Vec::new(),
        });
        self.response.clear();
        self.persist_active_conversation();
        self.chat_scroll.scroll_to_bottom();
        cx.notify();

        cx.spawn(async move |this, cx| {
            const MAX_READ_ROUNDS: usize = 3;
            const MAX_READ_CONTEXT_BYTES: usize = 160_000;
            let mut working_model = model.clone();
            let mut round_prompt = prompt.clone();
            let mut read_results = String::new();
            let mut reads_exhausted = false;
            let mut read_rounds = 0usize;
            let mut file_receipts: Vec<UiActionResult> = Vec::new();
            let mut final_text: Option<String> = None;
            let mut final_error: Option<String> = None;

            'rounds: loop {
                // Stop may have been pressed during a previous round's file
                // handling; do not start another provider request.
                if this
                    .update(cx, |panel, _| panel.request_epoch != epoch)
                    .unwrap_or(true)
                {
                    return;
                }
                // One provider request, with the same credit fallback the
                // single-shot path used. Each attempt registers an abort handle
                // so the Stop button can cancel the network call immediately.
                let mut result = {
                    let handle = crate::data::client::tokio_runtime().spawn({
                        let prompt = round_prompt.clone();
                        let context = context.clone();
                        let model = working_model.clone();
                        async move { crate::data::agent::analyze(&prompt, context, &model).await }
                    });
                    let abort = handle.abort_handle();
                    let _ = this.update(cx, |panel, _| {
                        if panel.request_epoch == epoch {
                            panel.in_flight = Some(abort);
                        }
                    });
                    handle.await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
                };

                if let Err(error) = &result
                    && crate::data::agent::is_credit_error_message(&error.to_string())
                {
                    for candidate in &fallbacks {
                        if candidate == &working_model {
                            continue;
                        }
                        if this
                            .update(cx, |panel, _| panel.request_epoch != epoch)
                            .unwrap_or(true)
                        {
                            return;
                        }
                        let handle = crate::data::client::tokio_runtime().spawn({
                            let prompt = round_prompt.clone();
                            let context = context.clone();
                            let candidate = candidate.clone();
                            async move {
                                crate::data::agent::analyze(&prompt, context, &candidate).await
                            }
                        });
                        let abort = handle.abort_handle();
                        let _ = this.update(cx, |panel, _| {
                            if panel.request_epoch == epoch {
                                panel.in_flight = Some(abort);
                            }
                        });
                        let attempt = handle.await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
                        if this
                            .update(cx, |panel, _| panel.request_epoch != epoch)
                            .unwrap_or(true)
                        {
                            return;
                        }
                        if let Ok(text) = attempt {
                            result = Ok(text);
                            working_model = candidate.clone();
                            break;
                        }
                    }
                }

                // Stop may have been pressed while the request was in flight.
                if this
                    .update(cx, |panel, _| panel.request_epoch != epoch)
                    .unwrap_or(true)
                {
                    return;
                }

                if working_model != model {
                    let switched = working_model.clone();
                    let _ = this.update(cx, |panel, cx| {
                        panel.selected_model = switched.clone();
                        let _ = crate::data::agent::save_selected_model(&switched);
                        panel.error = None;
                        cx.notify();
                    });
                }

                match result {
                    Ok(text) => {
                        let allow_reads = read_rounds < MAX_READ_ROUNDS && !reads_exhausted;
                        let mut execution = cx
                            .update(|cx| {
                                crate::components::ui::file_control::execute_file_actions_in_reply(
                                    &text, allow_reads, cx,
                                )
                            })
                            .unwrap_or_default();
                        file_receipts.append(&mut execution.receipts);
                        // Persist applied file changes immediately: if the user
                        // stops during a later read round, the receipts remain.
                        let receipts_snapshot = file_receipts.clone();
                        let _ = this.update(cx, |panel, _| {
                            if let Some(message) = panel.history.get_mut(message_index) {
                                message.file_receipts = receipts_snapshot;
                            }
                            panel.persist_active_conversation();
                        });

                        if execution.read_requested && allow_reads && !execution.reads.is_empty() {
                            read_rounds += 1;
                            read_results.push_str(&execution.read_context());
                            if read_results.len() > MAX_READ_CONTEXT_BYTES {
                                let mut end = MAX_READ_CONTEXT_BYTES;
                                while !read_results.is_char_boundary(end) {
                                    end -= 1;
                                }
                                read_results.truncate(end);
                                read_results.push_str(
                                    "\n…[read results truncated; no further reads will be returned]\n",
                                );
                                reads_exhausted = true;
                            }
                            round_prompt = format!(
                                "{prompt}\n\n{read_results}\n\nRead round {read_rounds} of {MAX_READ_ROUNDS}. Continue: emit another `file` read block if you need more files, or apply writes and answer. Do not repeat files you already received.",
                            );
                            let status = format!("Reading files… ({read_rounds}/{MAX_READ_ROUNDS})");
                            let _ = this.update(cx, |panel, cx| {
                                panel.status = Some(status);
                                cx.notify();
                            });
                            continue 'rounds;
                        }

                        final_text = Some(text);
                        break 'rounds;
                    }
                    Err(error) => {
                        final_error = Some(error.to_string());
                        break 'rounds;
                    }
                }
            }

            let _ = this.update(cx, |panel, _| {
                panel.status = None;
                panel.in_flight = None;
            });

            if this
                .update(cx, |panel, _| panel.request_epoch != epoch)
                .unwrap_or(true)
            {
                return;
            }

            match (final_text, final_error) {
                (Some(mut text), _) => {
                    if this
                        .update(cx, |panel, _| panel.request_epoch != epoch)
                        .unwrap_or(true)
                    {
                        return;
                    }
                    if working_model != model {
                        text.push_str(&format!(
                            "\n\n_Model switched to `{working_model}` after the previous model reported exhausted credits._"
                        ));
                    }
                    // Execute the reply's trade directives exactly once, here, as
                    // the response arrives. The receipts are stored on the message
                    // and persisted, so renders and reloads only display them.
                    let trade_receipts = crate::data::agent::execute_trades_in_reply(&text);
                    // Layout and workspace directives mutate whole entities, so
                    // they run in app scope first. Executing them inside the
                    // panel's own update would re-enter `set_agent_context` on
                    // this very entity when a command refreshes the snapshot.
                    let (ui_receipts, terminal_receipts) = {
                        let text = text.clone();
                        let caller_dashboard = caller_dashboard.clone();
                        let caller_pane = caller_pane.clone();
                        cx.update(|cx| {
                            let terminal_receipts =
                                crate::components::ui::terminal_control::execute_terminal_actions_in_reply(
                                    &text,
                                    caller_dashboard.as_deref(),
                                    caller_pane.as_deref(),
                                    cx,
                                );
                            let ui_receipts =
                                crate::components::ui::pane_control::execute_ui_actions_in_reply(
                                    &text,
                                    caller_dashboard.as_deref(),
                                    caller_pane.as_deref(),
                                    cx,
                                );
                            (ui_receipts, terminal_receipts)
                        })
                        .unwrap_or_default()
                    };
                    let typing_text = text.clone();
                    let started = this.update(cx, |panel, _cx| {
                        if panel.request_epoch != epoch {
                            return false;
                        }
                        // Persist the full answer before animating it, so closing
                        // the pane during the typing effect does not discard the
                        // reply. Stop reveals `typing_target` if interrupted.
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response = text.clone();
                            message.trade_receipts = trade_receipts;
                            message.ui_receipts = ui_receipts;
                            message.terminal_receipts = terminal_receipts;
                            message.file_receipts = file_receipts;
                        }
                        panel.typing_target = Some(typing_text);
                        panel.persist_active_conversation();
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response.clear();
                        }
                        true
                    });
                    if !matches!(started, Ok(true)) {
                        return;
                    }
                    for character in text.chars() {
                        let updated = this.update(cx, |panel, cx| {
                            if panel.request_epoch != epoch {
                                return false;
                            }
                            if let Some(message) = panel.history.get_mut(message_index) {
                                message.response.push(character);
                                panel.response.push(character);
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(updated, Ok(true)) {
                            return;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(10))
                            .await;
                    }

                    let _ = this.update(cx, |panel, cx| {
                        if panel.request_epoch != epoch {
                            return;
                        }
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.typing = false;
                        }
                        panel.typing_target = None;
                        panel.loading = false;
                        panel.in_flight = None;
                        panel.persist_active_conversation();
                        cx.notify();
                    });
                }
                (None, Some(error)) => {
                    if this
                        .update(cx, |panel, _| panel.request_epoch != epoch)
                        .unwrap_or(true)
                    {
                        return;
                    }
                    let updated = this.update(cx, |panel, cx| {
                        if panel.request_epoch != epoch {
                            return false;
                        }
                        panel.loading = false;
                        panel.in_flight = None;
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response = format!("Error: {error}");
                            message.failed = true;
                            message.typing = false;
                        }
                        panel.error = Some(error);
                        panel.persist_active_conversation();
                        cx.notify();
                        true
                    });
                    if !matches!(updated, Ok(true)) {
                        return;
                    }
                }
                (None, None) => {}
            }
        })
        .detach();
    }

    /// Cancel an in-flight reply: abort the provider request and invalidate the
    /// read/write loop so no further rounds or directives run.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.loading {
            return;
        }
        self.request_epoch = self.request_epoch.wrapping_add(1);
        if let Some(handle) = self.in_flight.take() {
            handle.abort();
        }
        self.loading = false;
        self.status = None;

        if let Some(message) = self.history.last_mut()
            && message.typing
        {
            message.typing = false;
            if let Some(text) = self.typing_target.take() {
                // The reply already arrived and was applied; Stop only skips
                // the typing animation, so show the complete answer.
                message.response = text.clone();
                self.response = text;
            } else if message.response.trim().is_empty() {
                message.response = "Stopped.".to_string();
            }
        }
        self.typing_target = None;
        self.persist_active_conversation();
        cx.notify();
    }

    fn refresh_models(&mut self, cx: &mut Context<Self>) {
        if self.models_loading { return; }
        self.discovery_attempted = true; self.models_loading = true; self.error = None; cx.notify();
        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime().spawn(crate::data::agent::discover_models()).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
            let _ = this.update(cx, |panel, cx| {
                panel.models_loading = false;
                match result {
                    Ok(models) => {
                        // Adopt the first discovered model when the user has no
                        // saved choice, so a fresh install can send immediately.
                        if panel.selected_model == "Select model" {
                            if let Some(first) = models.first().cloned() {
                                panel.selected_model = first.clone();
                                let _ = crate::data::agent::save_selected_model(&first);
                            }
                        }
                        panel.models = models;
                    }
                    Err(e) => panel.error = Some(e.to_string()),
                }
                cx.notify();
            });
        }).detach();
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_some() { return; }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask about markets, arrange dashboards, or control FinTer…"));
        cx.subscribe_in(&input, window, |panel, _input, event: &InputEvent, window, cx| {
            match event {
                InputEvent::Change => {
                    if let Some(inp) = panel.input.as_ref() {
                        panel.prompt = inp.read(cx).value().to_string();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => panel.send(window, cx),
                _ => {}
            }
        }).detach();
        self.input=Some(input);
    }

    /// Detect provider changes and kick off model discovery exactly once.
    ///
    /// Shared by the pane view and the floating composer so a fresh install can
    /// select a model from the workspace overlay without opening a pane first.
    fn sync_provider(&mut self, cx: &mut Context<Self>) {
        let resolved = crate::data::agent::provider_config();
        let current_provider = (resolved.base_url.clone(), resolved.api_key.clone());
        if self.provider != current_provider {
            self.provider = current_provider;
            self.discovery_attempted = false;
            self.models.clear();
            // Keep a model the user already chose; only reset when the new
            // provider has no saved selection.
            if resolved.model.as_deref().map_or(true, |m| m.trim().is_empty()) {
                self.selected_model = "Select model".into();
            }
        }
        if !self.discovery_attempted && !self.models_loading {
            self.refresh_models(cx);
        }
    }

    /// Parse every completed reply once so charts and receipts can be rendered
    /// consistently by the pane view and the workspace overlay.
    fn parsed_messages(&self) -> Vec<Vec<MessageSegment>> {
        self.history
            .iter()
            .map(|message| {
                if message.typing {
                    Vec::new()
                } else {
                    crate::data::agent::parse_message_segments_with_results(
                        &message.response,
                        &message.trade_receipts,
                    )
                }
            })
            .collect()
    }

    /// Materialise (and cache) the inline chart entities for parsed replies.
    fn materialize_charts(
        &mut self,
        parsed_messages: &[Vec<MessageSegment>],
        cx: &mut Context<Self>,
    ) -> std::collections::HashMap<String, Entity<AgentChart>> {
        let mut chart_entities = std::collections::HashMap::new();
        for (message_index, segments) in parsed_messages.iter().enumerate() {
            for (segment_index, segment) in segments.iter().enumerate() {
                let MessageSegment::Chart(request) = segment else {
                    continue;
                };
                let key = Self::chart_key(message_index, segment_index, request);
                let entity = self
                    .chart_views
                    .entry(key.clone())
                    .or_insert_with(|| cx.new(|cx| AgentChart::new(request.clone(), cx)))
                    .clone();
                chart_entities.insert(key, entity);
            }
        }
        // Drop entities for messages that no longer exist (new/loaded conversation).
        self.chart_views
            .retain(|key, _| chart_entities.contains_key(key));
        chart_entities
    }

    /// The partially streamed reply, with the trailing caret and directive
    /// fences withheld.
    fn render_typing_text(
        visible: &str,
        colors: &gpui_component::theme::ThemeColor,
        status: Option<&str>,
    ) -> AnyElement {
        if visible.trim().is_empty() {
            return div()
                .text_color(colors.muted_foreground)
                .text_xs()
                .child(status.unwrap_or("Thinking…").to_string())
                .into_any_element();
        }
        let lines: Vec<&str> = visible.split('\n').collect();
        let mut content = v_flex()
            .w_full()
            .gap(px(2.0))
            .text_sm()
            .text_color(colors.foreground);
        let count = lines.len();
        for (i, line) in lines.into_iter().enumerate() {
            if i + 1 == count {
                content = content.child(div().child(format!("{line}▍")));
            } else if line.is_empty() {
                content = content.child(div().h(px(8.0)));
            } else {
                content = content.child(div().child(line.to_string()));
            }
        }
        content.into_any_element()
    }

    /// Renders the markdown, inline charts, trade receipts and directive
    /// receipts for one completed reply.
    fn render_message_content(
        prefix: &str,
        base: usize,
        idx: usize,
        msg: &ChatMessage,
        parsed: &[MessageSegment],
        chart_entities: &std::collections::HashMap<String, Entity<AgentChart>>,
        colors: &gpui_component::theme::ThemeColor,
        mono_font: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut content = v_flex()
            .w_full()
            .gap(px(6.0))
            .text_sm()
            .text_color(colors.foreground);
        for (segment_index, segment) in parsed.iter().enumerate() {
            match segment {
                MessageSegment::Markdown(text) if !text.trim().is_empty() => {
                    content = content.child(
                        TextView::markdown(
                            SharedString::from(format!(
                                "{prefix}-msg-{idx}-{segment_index}"
                            )),
                            text.clone(),
                            window,
                            cx,
                        )
                        .selectable(true)
                        .scrollable(false),
                    );
                }
                MessageSegment::Chart(request) => {
                    let key = Self::chart_key(idx, segment_index, request);
                    if let Some(entity) = chart_entities.get(&key) {
                        content = content.child(entity.clone());
                    }
                }
                MessageSegment::Trade(action, result) => {
                    content = content.child(Self::render_trade_receipt(
                        base + idx * 1000 + segment_index,
                        action,
                        result.as_ref(),
                        colors,
                        mono_font,
                    ));
                }
                _ => {}
            }
        }
        for (receipt_index, receipt) in msg.ui_receipts.iter().enumerate() {
            content = content.child(Self::render_ui_receipt(
                base + idx * 1000 + receipt_index,
                receipt,
                colors,
                mono_font,
            ));
        }
        for (receipt_index, receipt) in msg.terminal_receipts.iter().enumerate() {
            content = content.child(Self::render_ui_receipt(
                base + idx * 1000 + 500 + receipt_index,
                receipt,
                colors,
                mono_font,
            ));
        }
        for (receipt_index, receipt) in msg.file_receipts.iter().enumerate() {
            content = content.child(Self::render_ui_receipt(
                base + idx * 1000 + 900 + receipt_index,
                receipt,
                colors,
                mono_font,
            ));
        }
        content.into_any_element()
    }

    /// The floating transcript pinned to the top-right of the workspace.
    ///
    /// The returned element is absolutely positioned by this method so the
    /// workspace container only has to append it. The transcript is a solid
    /// instrument panel: translucency over live charts made text hard to read.
    pub fn render_overlay_bubbles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_input(window, cx);
        if !self.overlay_bubbles_visible || self.history.is_empty() {
            return div().absolute().into_any_element();
        }

        // Keep the newest reply in view unless the user has scrolled away.
        // This mirrors the pane view; without it the overlay keeps rendering
        // below the fold while a reply is streaming.
        let distance_from_bottom =
            self.chat_scroll.max_offset().height + self.chat_scroll.offset().y;
        if distance_from_bottom <= px(2.0) {
            self.chat_scroll.scroll_to_bottom();
        }

        let colors = cx.theme().colors.clone();
        let mono_font = cx.theme().mono_font_family.clone();
        let parsed_messages = self.parsed_messages();
        let chart_entities = self.materialize_charts(&parsed_messages, cx);

        let title = if self.conversation_title.trim().is_empty() {
            "Agent".to_string()
        } else {
            self.conversation_title.clone()
        };

        let header = h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(7.0))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child(title),
            )
            .child(
                h_flex()
                    .gap(px(2.0))
                    .child(
                        Button::new("agent-overlay-new")
                            .label("New")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_new_conversation(cx);
                            })),
                    )
                    .child(
                        Button::new("agent-overlay-hide")
                            .label("—")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_overlay_bubbles_visible(false, cx);
                            })),
                    ),
            );

        // Cap the transcript relative to the window so it never becomes a wall
        // of panel on a short workspace.
        let mut message_max_h = window.viewport_size().height * 0.55;
        if message_max_h < px(220.0) {
            message_max_h = px(220.0);
        }
        if message_max_h > px(560.0) {
            message_max_h = px(560.0);
        }

        let mut messages = v_flex()
            .id("agent-overlay-bubbles")
            .w_full()
            .min_h_0()
            .max_h(message_max_h)
            .overflow_y_scroll()
            .track_scroll(&self.chat_scroll)
            .gap(px(8.0))
            .p(px(10.0));

        for (idx, msg) in self.history.iter().enumerate() {
            messages = messages.child(
                div().w_full().flex().justify_end().child(
                    div()
                        .max_w(px(320.0))
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(colors.secondary)
                        .border_1()
                        .border_color(colors.primary.opacity(0.35))
                        .text_color(colors.foreground)
                        .text_sm()
                        .child(msg.prompt.clone()),
                ),
            );

            let (agent_bg, agent_border) = if msg.failed {
                (colors.popover, colors.danger)
            } else {
                (colors.muted, colors.border)
            };
            messages = messages.child(
                div().w_full().flex().justify_start().child(
                    div()
                        .w_full()
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(agent_bg)
                        .border_1()
                        .border_color(agent_border)
                        .child(if msg.typing {
                            Self::render_typing_text(
                                &crate::data::agent::strip_chart_directives(&msg.response),
                                &colors,
                                self.status.as_deref(),
                            )
                        } else {
                            Self::render_message_content(
                                "agent-overlay",
                                1_000_000,
                                idx,
                                msg,
                                &parsed_messages[idx],
                                &chart_entities,
                                &colors,
                                &mono_font,
                                window,
                                cx,
                            )
                        }),
                ),
            );
        }

        let panel = v_flex()
            .w(px(420.0))
            .max_h(message_max_h + px(52.0))
            .rounded(px(12.0))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .overflow_hidden()
            .child(header)
            .child(messages);

        div()
            .absolute()
            .top(px(12.0))
            .right(px(16.0))
            .child(panel)
            .into_any_element()
    }

    /// The floating composer anchored to the bottom of the workspace.
    ///
    /// The full-width wrapper is deliberately non-interactive: only the card
    /// captures the pointer, so the pane grid remains usable either side of it.
    pub fn render_overlay_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_input(window, cx);
        self.sync_provider(cx);
        if !self.overlay_composer_visible {
            return div().absolute().into_any_element();
        }

        let colors = cx.theme().colors.clone();
        let input = self.input.as_ref().unwrap().clone();
        let models = self.models.clone();
        let selected = self.selected_model.clone();
        let owner = cx.entity().downgrade();

        let model_label = if self.models_loading {
            "Loading…".to_string()
        } else {
            selected.clone()
        };

        let model_button = Button::new("agent-overlay-model")
            .w(px(210.0))
            .overflow_hidden()
            .tooltip(model_label.clone())
            .label(short_model_label(&model_label, 16))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for model in &models {
                    let value = model.clone();
                    let checked = value == selected;
                    let owner = owner.clone();
                    let display = short_model_label(&value, 42);
                    menu = menu.item(
                        PopupMenuItem::new(display)
                            .checked(checked)
                            .on_click(move |_, _, app| {
                                let _ = owner.update(app, |panel, cx| {
                                    panel.selected_model = value.clone();
                                    panel.error = None;
                                    if let Err(error) =
                                        crate::data::agent::save_selected_model(&value)
                                    {
                                        panel.error = Some(format!(
                                            "Could not save the selected model: {error}"
                                        ));
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu.scrollable(true).max_h(px(420.0)).max_w(px(420.0))
            });

        let footer = h_flex()
            .w_full()
            .items_center()
            .gap(px(6.0))
            .child(model_button)
            .child(
                Button::new("agent-overlay-history")
                    .label("Chat")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_overlay_bubbles_visible(true, cx);
                    })),
            )
            .child(div().flex_1())
            .child(if self.loading {
                Button::new("agent-overlay-stop")
                    .label("Stop")
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                    .into_any_element()
            } else {
                Button::new("agent-overlay-send")
                    .label("Send")
                    .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                    .into_any_element()
            })
            .child(
                Button::new("agent-overlay-close")
                    .label("×")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_overlay_composer_visible(false, cx);
                    })),
            );

        let card = v_flex()
            .w(px(640.0))
            .max_w(relative(0.94))
            .gap(px(6.0))
            .p(px(8.0))
            .rounded(px(12.0))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .occlude()
            .child(Input::new(&input).w_full())
            .child(footer);

        let mut stack = v_flex().items_center().gap(px(4.0));
        if let Some(error) = self.error.clone() {
            stack = stack.child(
                div()
                    .w(px(640.0))
                    .max_w(relative(0.94))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .bg(colors.popover)
                    .border_1()
                    .border_color(colors.danger)
                    .text_xs()
                    .text_color(colors.danger)
                    .child(error),
            );
        }
        stack = stack.child(card);

        div()
            .absolute()
            .bottom(px(20.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(stack)
            .into_any_element()
    }
}
impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        // Compare the resolved provider (saved config, then env) rather than the
        // env vars alone: a provider configured through Settings is written to
        // `agent_config.json`, so an env-only check never noticed it change and
        // kept the stale model list.
        self.sync_provider(cx);
        let colors=cx.theme().colors.clone(); let mono_font=cx.theme().mono_font_family.clone(); let input=self.input.as_ref().unwrap().clone();
        let models=self.models.clone(); let selected=self.selected_model.clone();
        let owner = cx.entity().downgrade();
        let compact = self.width < 360.0 || self.height < 260.0;
        let padding = if compact { 6.0 } else { 12.0 };
        let gap = if compact { 4.0 } else { 10.0 };

        // Inspect the last laid-out scroll range before adding new text. Follow only
        // when already at the bottom, so reading older messages is never interrupted.
        let distance_from_bottom = self.chat_scroll.max_offset().height + self.chat_scroll.offset().y;
        if distance_from_bottom <= px(2.0) {
            self.chat_scroll.scroll_to_bottom();
        }

        let parsed_messages = self.parsed_messages();
        let chart_entities = self.materialize_charts(&parsed_messages, cx);

        // Keep each question close to its answer, with more breathing room between exchanges.
        // `overflow_y_scroll` preserves scrolling without drawing a permanent scrollbar.
        let mut chat_view = v_flex().id("agent-response").track_scroll(&self.chat_scroll).w_full().min_w_0().min_h_0().flex_1().overflow_y_scroll().p(px(if compact { 5.0 } else { 8.0 })).gap(px(if compact { 10.0 } else { 16.0 }));
        if self.history.is_empty() {
            chat_view = chat_view.child(
                div().text_color(colors.muted_foreground).text_sm()
                    .child("Ask the Agent to analyse markets, arrange dashboards, or control FinTer, e.g. “Chart BTC/USDT on 1h”.")
            );
        } else {
            for (idx, msg) in self.history.iter().enumerate() {
                chat_view = chat_view.child(
                    v_flex().w_full().gap(px(if compact { 4.0 } else { 6.0 }))
                        .child(
                            div().w_full().flex().justify_end().child(
                                div().max_w(px(720.0)).p(px(8.0)).rounded(px(6.0))
                                    .bg(colors.primary.opacity(0.12))
                                    .border_1().border_color(colors.primary.opacity(0.3))
                                    .text_color(colors.foreground).text_sm()
                                    .child(msg.prompt.clone())
                            )
                        )
                        .child(
                            div().w_full().flex().justify_start().child(
                                div().w_full().p(px(8.0)).rounded(px(6.0))
                                    .bg(if msg.failed { colors.bearish.opacity(0.1) } else { colors.muted.opacity(0.3) })
                                    .border_1().border_color(if msg.failed { colors.bearish.opacity(0.4) } else { colors.border.opacity(0.4) })
                                    .child(
                                        if msg.typing {
                                            Self::render_typing_text(
                                                &crate::data::agent::strip_chart_directives(&msg.response),
                                                &colors,
                                                self.status.as_deref(),
                                            )
                                        } else {
                                            Self::render_message_content(
                                                "agent",
                                                0,
                                                idx,
                                                msg,
                                                &parsed_messages[idx],
                                                &chart_entities,
                                                &colors,
                                                &mono_font,
                                                window,
                                                cx,
                                            )
                                        }
                                    )
                            )
                        )
                );
            }
        }

        let credit_error = crate::data::credits::CreditStore::module_error("AI Agent");
        if let Some(ref ce) = credit_error {
            return v_flex()
                .size_full()
                .bg(colors.background)
                .child(crate::components::ui::credit_alert::render_credits_exhausted_empty(
                    "AI Agent",
                    Some(ce),
                    self.height < 200.0 || self.width < 250.0,
                    cx.theme(),
                ))
                .into_any_element();
        }

        div().size_full().w_full().min_w_0().min_h_0().p(px(padding)).flex().flex_col().gap(px(gap)).overflow_hidden().bg(colors.background)
            .when(!self.conversation_title.is_empty(), |this| {
                this.child(
                    div().w_full().flex().items_center().justify_between().px(px(4.0)).pb(px(2.0))
                        .child(
                            div().text_xs().text_color(colors.muted_foreground).truncate()
                                .child(format!("Conversation: {}", self.conversation_title))
                        )
                        .child(
                            Button::new("agent-new-chat-btn").label("New conversation").compact().ghost().on_click(cx.listener(|this, _, _window, cx| this.start_new_conversation(cx)))
                        )
                )
            })
            .when_some(self.error.clone(), |this, err| {
                this.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(colors.danger.opacity(0.12))
                        .border_1()
                        .border_color(colors.danger.opacity(0.35))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(colors.danger)
                                .child(err),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .cursor_pointer()
                                .text_xs()
                                .text_color(colors.danger.opacity(0.8))
                                .hover(|s| s.text_color(colors.danger))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.error = None;
                                        cx.notify();
                                    }),
                                )
                                .child("✕"),
                        ),
                )
            })
            .child(chat_view)
            .child(div().min_w_0().flex().items_center().gap(px(6.0))
                .child(Input::new(&input).flex_1())
                .child(Button::new("agent-model-select").overflow_hidden().label(if self.models_loading { "Loading…".to_string() } else { short_model_label(&selected, 16) }).tooltip(selected.clone()).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                    for model in &models {
                        let value=model.clone(); let checked=value==selected; let owner=owner.clone();
                        menu=menu.item(PopupMenuItem::new(short_model_label(&value, 42)).checked(checked).on_click(move |_, _, app| {
                            let _ = owner.update(app, |panel, cx| {
                                panel.selected_model = value.clone();
                                panel.error = None;
                                // Persist immediately: the model choice used to
                                // live only in memory, so every restart forced a
                                // manual re-selection before the Agent could talk
                                // to the provider at all.
                                if let Err(error) = crate::data::agent::save_selected_model(&value) {
                                    panel.error = Some(format!("Could not save the selected model: {error}"));
                                }
                                cx.notify();
                            });
                        }));
                    }
                    menu.scrollable(true).max_h(px(420.0)).max_w(px(420.0)) }))
                .child(if self.loading {
                    Button::new("agent-stop").label("Stop").on_click(cx.listener(|this, _, _, cx| this.stop(cx))).into_any_element()
                } else {
                    Button::new("agent-send").label("Send").on_click(cx.listener(|this, _, window, cx| this.send(window, cx))).into_any_element()
                }))
            .into_any_element()
    }
}
