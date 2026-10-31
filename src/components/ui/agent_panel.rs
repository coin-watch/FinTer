//! AI Agent module: read-only analysis of data from the active dashboards.
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::v_flex;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::text::TextView;
use serde_json::Value;

#[derive(Clone)]
struct ChatMessage {
    prompt: String,
    response: String,
    failed: bool,
    typing: bool,
    timestamp: Option<String>,
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
    width: f32,
    height: f32,
    dashboard_count: usize,
    module_count: usize,
    context_bytes: usize,
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
            selected_model: "Select model".into(),
            models_loading: false,
            discovery_attempted: false,
            provider: (std::env::var("FINTER_AI_BASE_URL").unwrap_or_default(), std::env::var("FINTER_AI_API_KEY").unwrap_or_default()),
            loading: false,
            error: None,
            width: 300.0,
            height: 200.0,
            dashboard_count: 0,
            module_count: 0,
            context_bytes: 0,
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
        self.chat_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub fn load_conversation(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.loading { return; }
        if let Some(conv) = crate::data::agent::load_conversation(id) {
            self.conversation_id = Some(conv.id);
            self.conversation_title = conv.title;
            self.history = conv.messages.into_iter().map(|m| ChatMessage {
                prompt: m.prompt,
                response: m.response,
                failed: m.failed,
                typing: false,
                timestamp: m.timestamp,
            }).collect();
            self.response.clear();
            self.error = None;
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
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() || self.loading { return; }
        let model = self.selected_model.clone();
        if self.selected_model == "Select model" { self.error = Some("Select a model first; use Refresh models to discover provider models.".into()); cx.notify(); return; }
        if self.conversation_id.is_none() {
            let id = uuid::Uuid::new_v4().to_string();
            self.conversation_id = Some(id);
            self.conversation_title = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or(&prompt).chars().take(40).collect();
        }
        let context = self.context.clone(); self.loading = true; self.error = None;
        // Clear the visible editor immediately on every accepted submission, rather
        // than waiting for the asynchronous response to arrive.
        if let Some(input) = self.input.as_ref() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.prompt.clear();

        // Create the conversation pair immediately so the question and a "Thinking…"
        // placeholder bubble appear instantly without waiting for network turnaround.
        let message_index = self.history.len();
        self.history.push(ChatMessage {
            prompt: prompt.clone(),
            response: String::new(),
            failed: false,
            typing: true,
            timestamp: Some(chrono::Local::now().format("%b %d, %H:%M").to_string()),
        });
        self.response.clear();
        self.persist_active_conversation();
        self.chat_scroll.scroll_to_bottom();
        cx.notify();

        let _prompt_clone = prompt.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime().spawn(async move { crate::data::agent::analyze(&prompt, context, &model).await }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));

            match result {
                Ok(text) => {
                    // Persist the full answer before animating it, so closing the
                    // pane during the typing effect does not discard the reply.
                    let _ = this.update(cx, |panel, _| {
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response = text.clone();
                        }
                        panel.persist_active_conversation();
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response.clear();
                        }
                    });
                    for character in text.chars() {
                        let updated = this.update(cx, |panel, cx| {
                            if let Some(message) = panel.history.get_mut(message_index) {
                                message.response.push(character);
                                panel.response.push(character);
                            }
                            cx.notify();
                        });
                        if updated.is_err() {
                            return;
                        }
                        cx.background_executor().timer(std::time::Duration::from_millis(10)).await;
                    }

                    let _ = this.update(cx, |panel, cx| {
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.typing = false;
                        }
                        panel.loading = false;
                        panel.persist_active_conversation();
                        cx.notify();
                    });
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    let _ = this.update(cx, |panel, cx| {
                        panel.loading = false;
                        if let Some(message) = panel.history.get_mut(message_index) {
                            message.response = format!("Error: {err_msg}");
                            message.failed = true;
                            message.typing = false;
                        }
                        panel.error = Some(err_msg);
                        panel.persist_active_conversation();
                        cx.notify();
                    });
                }
            }
        }).detach();
    }
    fn refresh_models(&mut self, cx: &mut Context<Self>) {
        if self.models_loading { return; }
        self.discovery_attempted = true; self.models_loading = true; self.error = None; cx.notify();
        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime().spawn(crate::data::agent::discover_models()).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
            let _ = this.update(cx, |panel, cx| {
                panel.models_loading = false;
                match result { Ok(models) => { if panel.selected_model == "Select model" { panel.selected_model = models.first().cloned().unwrap_or_else(|| "Select model".into()); } panel.models=models; }, Err(e) => panel.error=Some(e.to_string()) }
                cx.notify();
            });
        }).detach();
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.is_some() { return; }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask about your dashboards..."));
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
}
impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        let current_provider = (std::env::var("FINTER_AI_BASE_URL").unwrap_or_default(), std::env::var("FINTER_AI_API_KEY").unwrap_or_default());
        if self.provider != current_provider {
            self.provider = current_provider;
            self.discovery_attempted = false;
            self.models.clear();
            self.selected_model = "Select model".into();
        }
        if !self.discovery_attempted && !self.models_loading { self.refresh_models(cx); }
        let colors=cx.theme().colors.clone(); let input=self.input.as_ref().unwrap().clone();
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

        // Keep each question close to its answer, with more breathing room between exchanges.
        // `overflow_y_scroll` preserves scrolling without drawing a permanent scrollbar.
        let mut chat_view = v_flex().id("agent-response").track_scroll(&self.chat_scroll).w_full().min_w_0().min_h_0().flex_1().overflow_y_scroll().p(px(if compact { 5.0 } else { 8.0 })).gap(px(if compact { 10.0 } else { 16.0 }));
        if self.history.is_empty() {
            chat_view = chat_view.child(
                div().text_color(colors.muted_foreground).text_sm()
                    .child("Ask the Agent to analyse the data visible across your open dashboards.")
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
                                            if msg.response.is_empty() {
                                                div().text_color(colors.muted_foreground).text_xs().child("Thinking…").into_any_element()
                                            } else {
                                                let lines: Vec<&str> = msg.response.split('\n').collect();
                                                let mut content = v_flex().w_full().gap(px(2.0)).text_sm().text_color(colors.foreground);
                                                let count = lines.len();
                                                for (i, line) in lines.into_iter().enumerate() {
                                                    if i + 1 == count {
                                                        content = content.child(
                                                            div().child(format!("{line}▍"))
                                                        );
                                                    } else if line.is_empty() {
                                                        content = content.child(div().h(px(8.0)));
                                                    } else {
                                                        content = content.child(div().child(line.to_string()));
                                                    }
                                                }
                                                content.into_any_element()
                                            }
                                        } else {
                                            TextView::markdown(("agent-msg", idx), msg.response.clone(), window, cx)
                                                .selectable(true)
                                                .scrollable(false)
                                                .into_any_element()
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
                .child(Button::new("agent-model-select").label(if self.models_loading { "Loading…".to_string() } else { selected.clone() }).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                    for model in &models {
                        let value=model.clone(); let checked=value==selected; let owner=owner.clone();
                        menu=menu.item(PopupMenuItem::new(value.clone()).checked(checked).on_click(move |_, _, app| {
                            let _ = owner.update(app, |panel, cx| { panel.selected_model=value.clone(); panel.error=None; cx.notify(); });
                        }));
                    }
                    menu.scrollable(true).max_h(px(420.0)) }))
                .child(Button::new("agent-send").label(if self.loading { "…" } else { "Send" }).on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))))
            .into_any_element()
    }
}
