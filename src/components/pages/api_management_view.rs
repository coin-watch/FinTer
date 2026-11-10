use crate::components::ui::ExchangeSelectItem;
use crate::data::api_management::{
    self, supported_api_exchanges, ExchangeCredential,
};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputState},
    select::{SearchableVec, Select, SelectDelegate, SelectEvent, SelectState},
    v_flex, ActiveTheme, Icon, IconName, Sizable, Size,
};

pub struct ApiManagementView {
    credentials: Vec<ExchangeCredential>,
    is_editing: bool,
    editing_id: Option<String>,
    selected_exchange: String,
    is_testnet: bool,
    label_input: Option<Entity<InputState>>,
    api_key_input: Option<Entity<InputState>>,
    api_secret_input: Option<Entity<InputState>>,
    passphrase_input: Option<Entity<InputState>>,
    exchange_select: Option<Entity<SelectState<SearchableVec<ExchangeSelectItem>>>>,
    feedback_message: Option<(bool /* is_error */, String)>,
    copied_id: Option<String>,
    delete_confirm_id: Option<String>,
}

impl ApiManagementView {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let creds = api_management::load();
        let default_exchange = creds
            .first()
            .map(|c| c.exchange.clone())
            .unwrap_or_else(|| "binance".to_string());

        Self {
            credentials: creds,
            is_editing: false,
            editing_id: None,
            selected_exchange: default_exchange,
            is_testnet: false,
            label_input: None,
            api_key_input: None,
            api_secret_input: None,
            passphrase_input: None,
            exchange_select: None,
            feedback_message: None,
            copied_id: None,
            delete_confirm_id: None,
        }
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.label_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("e.g. Primary Trading, Scalper, Subaccount 1")
            });
            self.label_input = Some(input);
        }
        if self.api_key_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Paste Exchange API Key")
            });
            self.api_key_input = Some(input);
        }
        if self.api_secret_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste Exchange API Secret")
                    .masked(true)
            });
            self.api_secret_input = Some(input);
        }
        if self.passphrase_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Passphrase (required for OKX, KuCoin, Coinbase)")
                    .masked(true)
            });
            self.passphrase_input = Some(input);
        }
        if self.exchange_select.is_none() {
            let items: Vec<ExchangeSelectItem> = supported_api_exchanges()
                .into_iter()
                .map(|(id, name)| ExchangeSelectItem::new(id, name))
                .collect();
            let delegate = SearchableVec::new(items);
            let selected_id = SharedString::from(self.selected_exchange.clone());
            let selected_index = delegate.position(&selected_id);
            let select = cx.new(|cx| {
                SelectState::new(delegate, selected_index, window, cx).searchable(true)
            });
            cx.subscribe(
                &select,
                |this, _entity, event: &SelectEvent<SearchableVec<ExchangeSelectItem>>, cx| {
                    if let SelectEvent::Confirm(Some(val)) = event {
                        this.selected_exchange = val.to_string();
                        cx.notify();
                    }
                },
            )
            .detach();
            self.exchange_select = Some(select);
        }
    }

    pub fn start_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_editing = true;
        self.editing_id = None;
        self.selected_exchange = "binance".to_string();
        self.is_testnet = false;
        self.delete_confirm_id = None;
        self.feedback_message = None;
        self.ensure_inputs(window, cx);

        if let Some(ref select) = self.exchange_select {
            let ex = SharedString::from("binance");
            select.update(cx, |s, cx| {
                s.set_selected_value(&ex, window, cx);
            });
        }

        if let Some(ref input) = self.label_input {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(ref input) = self.api_key_input {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(ref input) = self.api_secret_input {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(ref input) = self.passphrase_input {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        cx.notify();
    }

    pub fn start_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cred = self.credentials.iter().find(|c| c.id == id).cloned();
        if let Some(cred) = cred {
            self.is_editing = true;
            self.editing_id = Some(cred.id.clone());
            self.selected_exchange = cred.exchange.clone();
            self.is_testnet = cred.is_testnet;
            self.delete_confirm_id = None;
            self.feedback_message = None;
            self.ensure_inputs(window, cx);

            if let Some(ref select) = self.exchange_select {
                let ex = SharedString::from(cred.exchange.clone());
                select.update(cx, |s, cx| {
                    s.set_selected_value(&ex, window, cx);
                });
            }

            if let Some(ref input) = self.label_input {
                input.update(cx, |s, cx| s.set_value(&cred.label, window, cx));
            }
            if let Some(ref input) = self.api_key_input {
                input.update(cx, |s, cx| s.set_value(&cred.api_key, window, cx));
            }
            if let Some(ref input) = self.api_secret_input {
                input.update(cx, |s, cx| s.set_value(&cred.api_secret, window, cx));
            }
            if let Some(ref input) = self.passphrase_input {
                input.update(cx, |s, cx| s.set_value(&cred.passphrase, window, cx));
            }
            cx.notify();
        }
    }

    pub fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.is_editing = false;
        self.editing_id = None;
        self.delete_confirm_id = None;
        self.feedback_message = None;
        cx.notify();
    }

    pub fn save_form(&mut self, cx: &mut Context<Self>) {
        let label = self
            .label_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let key = self
            .api_key_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let secret = self
            .api_secret_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let passphrase = self
            .passphrase_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();

        if key.trim().is_empty() {
            self.feedback_message = Some((true, "API Key is required.".to_string()));
            cx.notify();
            return;
        }
        if secret.trim().is_empty() {
            self.feedback_message = Some((true, "API Secret is required.".to_string()));
            cx.notify();
            return;
        }

        let ex = self.selected_exchange.trim().to_lowercase();
        let id = self
            .editing_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let final_label = if label.trim().is_empty() {
            format!("{} Account", ex.to_uppercase())
        } else {
            label.trim().to_string()
        };

        let cred = ExchangeCredential {
            id,
            label: final_label,
            exchange: ex,
            api_key: key.trim().to_string(),
            api_secret: secret.trim().to_string(),
            passphrase: passphrase.trim().to_string(),
            is_testnet: self.is_testnet,
            created_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            ),
        };

        if let Err(e) = api_management::save_exchange_credential(cred) {
            self.feedback_message = Some((true, format!("Failed to save credential: {e}")));
        } else {
            self.credentials = api_management::load();
            self.is_editing = false;
            self.editing_id = None;
            self.feedback_message = Some((
                false,
                "API key encrypted with AES-256-GCM and stored in ~/.config/finter/api_providers.json."
                    .to_string(),
            ));
        }
        cx.notify();
    }

    pub fn delete_key(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(e) = api_management::delete_exchange_credential(id) {
            self.feedback_message = Some((true, format!("Failed to delete key: {e}")));
        } else {
            self.credentials = api_management::load();
            self.delete_confirm_id = None;
            self.feedback_message = Some((false, "Exchange API key deleted.".to_string()));
        }
        cx.notify();
    }

    pub fn copy_key(&mut self, id: &str, key: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(key.to_string()));
        self.copied_id = Some(id.to_string());
        cx.notify();

        let id_str = id.to_string();
        cx.spawn(async move |this, cx| {
            tokio::time::sleep(std::time::Duration::from_millis(1800)).await;
            _ = this.update(cx, |this, cx| {
                if this.copied_id.as_deref() == Some(&id_str) {
                    this.copied_id = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn mask_key(key: &str) -> String {
        let len = key.len();
        if len <= 8 {
            "••••••••••••".to_string()
        } else {
            format!("{}••••••••••••{}", &key[..4], &key[len - 4..])
        }
    }
}

impl Render for ApiManagementView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inputs(window, cx);

        if let Some(ref select) = self.exchange_select {
            let cur = select.read(cx).selected_value().cloned();
            let expected = SharedString::from(self.selected_exchange.clone());
            if cur.as_ref() != Some(&expected) {
                select.update(cx, |s, cx| {
                    s.set_selected_value(&expected, window, cx);
                });
            }
        }

        let border_color = cx.theme().border;
        let card_bg = cx.theme().background;
        let text_muted = cx.theme().muted_foreground;
        let text_primary = cx.theme().foreground;
        let primary_color = cx.theme().primary;

        let total_keys = self.credentials.len();

        v_flex()
            .w_full()
            .gap(px(16.))
            .child(
                // Security Banner & Protocol Info
                h_flex()
                    .w_full()
                    .p(px(14.))
                    .rounded(px(6.))
                    .bg(card_bg)
                    .border_1()
                    .border_color(border_color)
                    .justify_between()
                    .items_center()
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .items_center()
                            .child(
                                div()
                                    .p(px(8.))
                                    .rounded(px(6.))
                                    .bg(cx.theme().muted)
                                    .child(
                                        Icon::new(IconName::SquareTerminal)
                                            .size(px(20.))
                                            .text_color(primary_color),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap(px(3.))
                                    .child(
                                        h_flex()
                                            .gap(px(8.))
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(text_primary)
                                                    .child("LOCAL CREDENTIAL VAULT"),
                                            )
                                            .child(
                                                div()
                                                    .px(px(6.))
                                                    .py(px(1.5))
                                                    .rounded(px(4.))
                                                    .bg(cx.theme().primary.opacity(0.12))
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(primary_color)
                                                    .child("AES-256-GCM · 0600"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(text_muted)
                                            .child("All API keys and secrets are encrypted with AES-256-GCM before being written to ~/.config/finter/api_providers.json (user-only permissions). The encryption key is held in the OS keyring, never in the config file. Zero cloud sync, zero telemetry."),
                                    ),
                            ),
                    ),
            )
            .when_some(self.feedback_message.clone(), |this, (is_error, msg)| {
                this.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(4.))
                        .gap(px(8.))
                        .items_center()
                        .bg(if is_error {
                            cx.theme().danger.opacity(0.15)
                        } else {
                            cx.theme().accent.opacity(0.2)
                        })
                        .border_1()
                        .border_color(if is_error {
                            cx.theme().danger.opacity(0.4)
                        } else {
                            cx.theme().primary.opacity(0.4)
                        })
                        .child(
                            Icon::new(if is_error {
                                IconName::TriangleAlert
                            } else {
                                IconName::Check
                            })
                            .size(px(14.))
                            .flex_shrink_0()
                            .text_color(if is_error {
                                cx.theme().danger
                            } else {
                                cx.theme().primary
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(msg),
                        ),
                )
            })
            // Toolbar row: Key counts + Add button
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .py(px(4.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_primary)
                                    .child("CONFIGURED EXCHANGES"),
                            )
                            .child(
                                div()
                                    .px(px(6.))
                                    .py(px(1.5))
                                    .rounded(px(10.))
                                    .bg(cx.theme().muted)
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_muted)
                                    .child(format!("{total_keys} ACTIVE")),
                            ),
                    )
                    .child(
                        if self.is_editing {
                            Button::new("cancel-btn")
                                .outline()
                                .label("Close Editor")
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.cancel_edit(cx);
                                }))
                        } else {
                            Button::new("add-key-btn")
                                .primary()
                                .label("+ Add Exchange Key")
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.start_add(window, cx);
                                }))
                        },
                    ),
            )
            // Edit / Add Form panel
            .when(self.is_editing, |this| {
                let is_updating = self.editing_id.is_some();
                let exchange_select = self.exchange_select.clone();

                this.child(
                    v_flex()
                        .w_full()
                        .p(px(16.))
                        .rounded(px(6.))
                        .bg(card_bg)
                        .border_1()
                        .border_color(primary_color.opacity(0.5))
                        .gap(px(14.))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(text_primary)
                                        .child(if is_updating {
                                            "EDIT EXCHANGE CREDENTIAL"
                                        } else {
                                            "ADD NEW EXCHANGE CREDENTIAL"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(text_muted)
                                        .child("Required: Exchange, API Key, API Secret"),
                                ),
                        )
                        // Form fields
                        .child(
                            v_flex()
                                .w_full()
                                .gap(px(12.))
                                // Row 1: Exchange Selector + Label + Environment
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap(px(12.))
                                        .items_end()
                                        // Exchange combobox
                                        .child(
                                            v_flex()
                                                .gap(px(4.))
                                                .w(px(220.))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_muted)
                                                        .child("EXCHANGE"),
                                                )
                                                .when_some(exchange_select, |this, select| {
                                                    this.child(
                                                        div()
                                                            .w_full()
                                                            .h_8()
                                                            .child(
                                                                Select::new(&select)
                                                                    .w_full()
                                                                    .h_full()
                                                                    .with_size(Size::Medium)
                                                                    .search_placeholder("Search exchange..."),
                                                            ),
                                                    )
                                                }),
                                        )
                                        // Label / Nickname input
                                        .child(
                                            v_flex()
                                                .gap(px(4.))
                                                .flex_1()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_muted)
                                                        .child("LABEL / NICKNAME"),
                                                )
                                                .when_some(self.label_input.clone(), |this, inp| {
                                                    this.child(Input::new(&inp))
                                                }),
                                        )
                                        // Testnet vs Live toggle
                                        .child(
                                            v_flex()
                                                .gap(px(4.))
                                                .w(px(180.))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_muted)
                                                        .child("ENVIRONMENT"),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap(px(4.))
                                                        .child(
                                                            Button::new("env-live")
                                                                .label("LIVE")
                                                                .when(!self.is_testnet, |s| s.primary())
                                                                .when(self.is_testnet, |s| s.outline())
                                                                .on_click(cx.listener(|this, _, _, cx| {
                                                                    this.is_testnet = false;
                                                                    cx.notify();
                                                                })),
                                                        )
                                                        .child(
                                                            Button::new("env-testnet")
                                                                .label("TESTNET")
                                                                .when(self.is_testnet, |s| s.primary())
                                                                .when(!self.is_testnet, |s| s.outline())
                                                                .on_click(cx.listener(|this, _, _, cx| {
                                                                    this.is_testnet = true;
                                                                    cx.notify();
                                                                })),
                                                        ),
                                                ),
                                        ),
                                )
                                // Row 2: API Key Input
                                .child(
                                    v_flex()
                                        .gap(px(4.))
                                        .w_full()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(text_muted)
                                                .child("API KEY"),
                                        )
                                        .when_some(self.api_key_input.clone(), |this, inp| {
                                            this.child(Input::new(&inp))
                                        }),
                                )
                                // Row 3: API Secret Input (with mask toggle)
                                .child(
                                    v_flex()
                                        .gap(px(4.))
                                        .w_full()
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(text_muted)
                                                .child("API SECRET"),
                                        )
                                        .when_some(self.api_secret_input.clone(), |this, inp| {
                                            this.child(Input::new(&inp).mask_toggle())
                                        }),
                                )
                                // Row 4: Passphrase Input (Optional, with mask toggle)
                                .child(
                                    v_flex()
                                        .gap(px(4.))
                                        .w_full()
                                        .child(
                                            h_flex()
                                                .gap(px(6.))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(text_muted)
                                                        .child("PASSPHRASE (OPTIONAL)"),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground.opacity(0.8))
                                                        .child("— Required for OKX, KuCoin, Coinbase, Bitget"),
                                                ),
                                        )
                                        .when_some(self.passphrase_input.clone(), |this, inp| {
                                            this.child(Input::new(&inp).mask_toggle())
                                        }),
                                ),
                        )
                        // Form buttons
                        .child(
                            h_flex()
                                .w_full()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    Button::new("cancel-form-btn")
                                        .outline()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_edit(cx);
                                        })),
                                )
                                .child(
                                    Button::new("save-form-btn")
                                        .primary()
                                        .label("Save & Encrypt Key")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_form(cx);
                                        })),
                                ),
                        ),
                )
            })
            // Configured Keys List or Empty State
            .child(
                if self.credentials.is_empty() && !self.is_editing {
                    // Empty State
                    v_flex()
                        .w_full()
                        .items_center()
                        .justify_center()
                        .py(px(48.))
                        .px(px(24.))
                        .gap(px(14.))
                        .border_1()
                        .border_color(border_color)
                        .rounded(px(6.))
                        .bg(card_bg)
                        .child(
                            Icon::new(IconName::SquareTerminal)
                                .size(px(32.))
                                .text_color(text_muted),
                        )
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .text_color(text_primary)
                                .child("No Exchange Keys Configured"),
                        )
                        .child(
                            div()
                                .max_w(px(460.))
                                .text_center()
                                .text_xs()
                                .text_color(text_muted)
                                .child("Connect Binance, Bybit, OKX, or other supported exchanges to enable authenticated order execution, balance tracking, and private WebSocket streams. All credentials are encrypted at rest with AES-256-GCM."),
                        )
                        .child(
                            Button::new("add-first-key-btn")
                                .primary()
                                .label("+ Add Your First Exchange Key")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_add(window, cx);
                                })),
                        )
                } else {
                    // Key Cards Grid
                    let creds = self.credentials.clone();
                    let copied_id = self.copied_id.clone();
                    let confirm_id = self.delete_confirm_id.clone();

                    v_flex()
                        .w_full()
                        .gap(px(10.))
                        .children(creds.into_iter().map(|cred| {
                            let id_for_edit = cred.id.clone();
                            let id_for_del = cred.id.clone();
                            let id_for_copy = cred.id.clone();
                            let key_for_copy = cred.api_key.clone();
                            let is_copied = copied_id.as_deref() == Some(&cred.id);
                            let is_confirming_del = confirm_id.as_deref() == Some(&cred.id);
                            let masked_key = Self::mask_key(&cred.api_key);

                            v_flex()
                                .w_full()
                                .p(px(12.))
                                .rounded(px(6.))
                                .bg(card_bg)
                                .border_1()
                                .border_color(border_color)
                                .gap(px(10.))
                                // Card Top Row
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            h_flex()
                                                .gap(px(8.))
                                                .items_center()
                                                .child(
                                                    div()
                                                        .px(px(8.))
                                                        .py(px(2.))
                                                        .rounded(px(4.))
                                                        .bg(primary_color.opacity(0.15))
                                                        .text_xs()
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(primary_color)
                                                        .child(cred.exchange.to_uppercase()),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(text_primary)
                                                        .child(cred.label.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .px(px(6.))
                                                        .py(px(1.5))
                                                        .rounded(px(4.))
                                                        .bg(if cred.is_testnet {
                                                            cx.theme().warning.opacity(0.15)
                                                        } else {
                                                            cx.theme().accent.opacity(0.2)
                                                        })
                                                        .text_xs()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(if cred.is_testnet {
                                                            cx.theme().warning
                                                        } else {
                                                            primary_color
                                                        })
                                                        .child(if cred.is_testnet { "TESTNET" } else { "LIVE" }),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap(px(6.))
                                                .items_center()
                                                .child(
                                                    Button::new(SharedString::from(format!("edit-{}", cred.id)))
                                                        .outline()
                                                        .label("Edit")
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.start_edit(&id_for_edit, window, cx);
                                                        })),
                                                )
                                                .child(
                                                    if is_confirming_del {
                                                        let id_del_exec = id_for_del.clone();
                                                        Button::new(SharedString::from(format!("confirm-del-{}", cred.id)))
                                                            .label("Confirm Delete?")
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                this.delete_key(&id_del_exec, cx);
                                                            }))
                                                    } else {
                                                        Button::new(SharedString::from(format!("del-{}", cred.id)))
                                                            .outline()
                                                            .label("Delete")
                                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                                this.delete_confirm_id = Some(id_for_del.clone());
                                                                cx.notify();
                                                            }))
                                                    },
                                                ),
                                        ),
                                )
                                // Key Details Row
                                .child(
                                    h_flex()
                                        .w_full()
                                        .p(px(8.))
                                        .rounded(px(4.))
                                        .bg(cx.theme().background)
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            h_flex()
                                                .gap(px(8.))
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(text_muted)
                                                        .child("API KEY:"),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_family("monospace")
                                                        .text_color(text_primary)
                                                        .child(masked_key),
                                                ),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("copy-{}", cred.id)))
                                                .outline()
                                                .label(if is_copied { "Copied!" } else { "Copy Key" })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.copy_key(&id_for_copy, &key_for_copy, cx);
                                                })),
                                        ),
                                )
                                // Status & Metadata Row
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .items_center()
                                        .text_xs()
                                        .text_color(text_muted)
                                        .child(
                                            h_flex()
                                                .gap(px(14.))
                                                .child(
                                                    h_flex()
                                                        .gap(px(4.))
                                                        .items_center()
                                                        .child(
                                                            Icon::new(IconName::Check)
                                                                .size(px(12.))
                                                                .text_color(primary_color),
                                                        )
                                                        .child("API Secret Stored"),
                                                )
                                                .child(
                                                    h_flex()
                                                        .gap(px(4.))
                                                        .items_center()
                                                        .child(
                                                            Icon::new(if cred.passphrase.trim().is_empty() {
                                                                IconName::Dash
                                                            } else {
                                                                IconName::Check
                                                            })
                                                            .size(px(12.))
                                                            .text_color(if cred.passphrase.trim().is_empty() {
                                                                text_muted
                                                            } else {
                                                                primary_color
                                                            }),
                                                        )
                                                        .child(if cred.passphrase.trim().is_empty() {
                                                            "No Passphrase"
                                                        } else {
                                                            "Passphrase Configured"
                                                        }),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family("monospace")
                                                .child("STORAGE: LOCAL 0600"),
                                        )
                                )
                        }))
                },
            )
    }
}
