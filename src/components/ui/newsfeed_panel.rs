use std::{collections::HashSet, time::Duration};

use futures_util::{SinkExt, StreamExt};
use gpui::{
    div, prelude::FluentBuilder, AppContext, BorrowAppContext, Context, ElementId, Entity,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Window, px,
};
use gpui_component::{scroll::ScrollableElement, theme::ActiveTheme, Sizable};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::data::client::tokio_runtime;

const LIMIT: usize = 50;

fn articles_url() -> String {
    if let Ok(base) = std::env::var("FINTER_NEWS_API_URL") {
        let base = base.trim_end_matches('/');
        if !base.is_empty() {
            if base.contains("newsfeed.coinwat.ch") {
                return format!("{base}/api/articles?limit={LIMIT}");
            } else {
                return format!("{base}/api/newsfeed/articles?limit={LIMIT}");
            }
        }
    }
    let base = crate::data::client::service_base_url();
    format!("{base}/api/newsfeed/articles?limit={LIMIT}")
}

fn news_ws_url() -> String {
    std::env::var("FINTER_NEWS_WS_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "wss://newsfeed.coinwat.ch/ws".to_string())
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
struct Article {
    id: String,
    title: String,
    url: String,
    source: String,
    published_at: Option<String>,
    summary: Option<String>,
    category: String,
    priority: i32,
}

#[derive(Deserialize)]
struct ArticlesResponse {
    articles: Vec<Article>,
}

#[derive(Deserialize)]
struct NewsEvent {
    event: String,
    article: Option<Article>,
}

enum FeedEvent {
    Loaded(Vec<Article>),
    Created(Article),
    Connected(bool),
    Error,
}

pub struct NewsfeedPanel {
    articles: Vec<Article>,
    rx: mpsc::Receiver<FeedEvent>,
    loading: bool,
    connected: bool,
    failed: bool,
    filter_input: Option<Entity<InputState>>,
    priority: String,
    filter: String,
    pane_id: String,
    timezone: String,
    width: f32,
    height: f32,
}

impl NewsfeedPanel {
    pub fn agent_snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "articles": self.articles.iter().take(15).collect::<Vec<_>>(),
            "loading": self.loading
        })
    }

    pub fn new(pane_id: String, priority: String, filter: String, cx: &mut Context<Self>) -> Self {
        // Coalesce external updates; weak access ends the task on panel close.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                if this.update(cx, |panel, cx| {
                    if panel.pump() { cx.notify(); }
                }).is_err() { break; }
            }
        }).detach();
        let (tx, rx) = mpsc::channel(64);
        tokio_runtime().spawn(run_feed(tx, priority.clone(), filter.clone()));
        Self {
            articles: Vec::new(),
            rx,
            loading: true,
            connected: false,
            failed: false,
            filter_input: None,
            priority,
            filter,
            pane_id,
            timezone: "UTC".into(),
            width: 320.0,
            height: 300.0,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        if (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5 {
            return;
        }
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    fn ensure_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter keywords")
                .default_value(self.filter.clone())
        });
        cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.filter = input.read(cx).value().trim().to_string();
                this.restart(cx);
            }
        })
        .detach();
        self.filter_input = Some(input);
    }

    pub fn set_timezone(&mut self, timezone: impl Into<String>) {
        self.timezone = timezone.into();
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        let pane_id = self.pane_id.clone();
        let priority = self.priority.clone();
        let filter = self.filter.clone();
        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
            let _ = global.view.update(cx, |view, cx| {
                let mut config = serde_json::Map::new();
                config.insert("priority".to_string(), serde_json::Value::String(priority));
                config.insert("filter".to_string(), serde_json::Value::String(filter));
                view.handle_pane_event(
                    &crate::components::ui::pane_grid::renderer::PaneGridEvent::UpdateModuleConfig {
                        pane_id,
                        config: serde_json::Value::Object(config),
                    },
                    cx,
                );
            });
        });
        let (tx, rx) = mpsc::channel(64);
        self.rx = rx;
        self.loading = self.articles.is_empty();
        self.failed = false;
        tokio_runtime().spawn(run_feed(tx, self.priority.clone(), self.filter.clone()));
        cx.notify();
    }

    fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.rx.try_recv() {
            changed = true;
            match event {
                FeedEvent::Loaded(mut articles) => {
                    prioritize(&mut articles);
                    articles.truncate(LIMIT);
                    self.articles = articles;
                    self.loading = false;
                    self.failed = false;
                }
                FeedEvent::Created(article) => {
                    let category_match = self.priority == "latest" || article.category == self.priority;
                    let keywords_match = self.filter.split(',').map(str::trim).filter(|value| !value.is_empty()).all(|keyword| {
                        let keyword = keyword.to_lowercase();
                        clean_text(&article.title).to_lowercase().contains(&keyword)
                            || clean_text(&article.summary.clone().unwrap_or_default()).to_lowercase().contains(&keyword)
                    });
                    if category_match && keywords_match && !self.articles.iter().any(|item| item.id == article.id) {
                        self.articles.push(article);
                        prioritize(&mut self.articles);
                        self.articles.truncate(LIMIT);
                    }
                }
                FeedEvent::Connected(connected) => self.connected = connected,
                FeedEvent::Error => {
                    self.loading = false;
                    self.failed = self.articles.is_empty();
                }
            }
        }
        changed
    }
}

async fn run_feed(tx: mpsc::Sender<FeedEvent>, priority: String, filter: String) {
    let url = articles_url();
    let client = reqwest::Client::builder()
        .user_agent("FinTer Native")
        .build()
        .expect("reqwest client");
    let mut request = client.get(&url).header("X-API-Key", "secret");
    if priority != "latest" {
        request = request.query(&[("category", priority.as_str())]);
    }
    if !filter.is_empty() {
        request = request.query(&[("query", filter.as_str())]);
    }
    match request.send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.json::<ArticlesResponse>().await {
                Ok(payload) => {
                    let _ = tx.send(FeedEvent::Loaded(payload.articles)).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse newsfeed articles JSON: {e}");
                    let _ = tx.send(FeedEvent::Error).await;
                }
            },
            Err(e) => {
                tracing::warn!("Failed to fetch newsfeed from primary endpoint: {e}");
                // Fallback attempt: try direct upstream newsfeed.coinwat.ch with secret API key
                let fallback_url = format!("https://newsfeed.coinwat.ch/api/articles?limit={LIMIT}");
                let mut fb_req = client.get(&fallback_url).header("X-API-Key", "secret");
                if priority != "latest" {
                    fb_req = fb_req.query(&[("category", priority.as_str())]);
                }
                if !filter.is_empty() {
                    fb_req = fb_req.query(&[("query", filter.as_str())]);
                }
                let mut loaded = false;
                if let Ok(resp) = fb_req.send().await {
                    if let Ok(resp) = resp.error_for_status() {
                        if let Ok(payload) = resp.json::<ArticlesResponse>().await {
                            let _ = tx.send(FeedEvent::Loaded(payload.articles)).await;
                            loaded = true;
                        }
                    }
                }
                if !loaded {
                    let _ = tx.send(FeedEvent::Error).await;
                }
            }
        },
        Err(e) => {
            tracing::warn!("Failed to connect to newsfeed endpoint: {e}");
            // Fallback attempt
            let fallback_url = format!("https://newsfeed.coinwat.ch/api/articles?limit={LIMIT}");
            let mut fb_req = client.get(&fallback_url).header("X-API-Key", "secret");
            if priority != "latest" {
                fb_req = fb_req.query(&[("category", priority.as_str())]);
            }
            if !filter.is_empty() {
                fb_req = fb_req.query(&[("query", filter.as_str())]);
            }
            let mut loaded = false;
            if let Ok(resp) = fb_req.send().await {
                if let Ok(resp) = resp.error_for_status() {
                    if let Ok(payload) = resp.json::<ArticlesResponse>().await {
                        let _ = tx.send(FeedEvent::Loaded(payload.articles)).await;
                        loaded = true;
                    }
                }
            }
            if !loaded {
                let _ = tx.send(FeedEvent::Error).await;
            }
        }
    }

    let ws_url = news_ws_url();
    loop {
        match connect_async(&ws_url).await {
            Ok((socket, _)) => {
                let _ = tx.send(FeedEvent::Connected(true)).await;
                let (mut writer, mut reader) = socket.split();
                let categories: Vec<&str> = if priority == "latest" {
                    Vec::new()
                } else {
                    vec![priority.as_str()]
                };
                let keywords: Vec<&str> = filter
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect();
                let subscription = serde_json::json!({
                    "action": "subscribe",
                    "categories": categories,
                    "keywords": keywords,
                });
                let _ = writer
                    .send(Message::Text(subscription.to_string().into()))
                    .await;
                while let Some(message) = reader.next().await {
                    let Ok(Message::Text(text)) = message else { continue };
                    let Ok(event) = serde_json::from_str::<NewsEvent>(&text) else { continue };
                    if event.event == "ping" {
                        let _ = writer
                            .send(Message::Text(r#"{"action":"pong"}"#.into()))
                            .await;
                    } else if event.event == "article.created" {
                        if let Some(article) = event.article {
                            let _ = tx.send(FeedEvent::Created(article)).await;
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let _ = tx.send(FeedEvent::Connected(false)).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn prioritize(articles: &mut Vec<Article>) {
    let mut ids = HashSet::new();
    articles.retain(|article| ids.insert(article.id.clone()));
    articles.sort_by(|a, b| b.priority.cmp(&a.priority));
}

fn clean_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 8);
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => {
                in_tag = true;
                output.push(' ');
            }
            '>' => {
                in_tag = false;
                output.push(' ');
            }
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_source(source: &str, category: &str) -> String {
    let source = clean_text(source);
    let category = category.to_uppercase();
    let upper = source.to_uppercase();
    for separator in [" | ", " - "] {
        let prefix = format!("{category}{separator}");
        if upper.starts_with(&prefix) {
            return source[prefix.len()..].trim().to_uppercase();
        }
    }
    source.to_uppercase()
}

fn published_time(value: Option<&str>, timezone: &str) -> String {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|date| {
            timezone
                .parse::<chrono_tz::Tz>()
                .map(|tz| date.with_timezone(&tz).format("%H:%M").to_string())
                .unwrap_or_else(|_| date.format("%H:%M").to_string())
        })
        .unwrap_or_else(|| "--:--".into())
}

impl Render for NewsfeedPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_input(window, cx);
        self.pump();

        let colors = &cx.theme().colors;
        let input = self.filter_input.as_ref().unwrap().clone();
        let status = if self.connected { "LIVE WIRE" } else { "CONNECTING" };
        let status_color = if self.connected { colors.primary } else { colors.muted_foreground };
        let priority = self.priority.clone();
        let pane_id = self.pane_id.clone();
        let is_narrow = self.width < 340.0;
        let is_compact_row = self.width < 320.0;
        let header = div()
            .w_full()
            .flex_shrink_0()
            .h(px(32.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(6.0))
            .px(px(8.0))
            .bg(colors.muted.opacity(0.18))
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .flex_shrink_0()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child(div().size(px(5.0)).bg(status_color))
                    .when(!is_narrow, |this| this.child(status)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(4.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .max_w(px(280.0))
                            .child(Input::new(&input).small().w_full().cleanable(true)),
                    )
                    .child(
                        Button::new("news-category")
                            .label(if is_narrow {
                                priority.chars().take(3).collect::<String>().to_uppercase()
                            } else {
                                priority.to_uppercase()
                            })
                            .ghost()
                            .xsmall()
                            .dropdown_caret(true)
                            .dropdown_menu_with_anchor(gpui::Corner::TopRight, {
                                let current = priority.clone();
                                move |mut menu, _window, _cx| {
                                    for (value, label) in [
                                        ("latest", "Latest"),
                                        ("world", "World"),
                                        ("politics", "Politics"),
                                        ("business", "Business"),
                                        ("science-tech", "Science & technology"),
                                        ("society", "Society"),
                                        ("sports", "Sports"),
                                    ] {
                                        let selected = value;
                                        let p_id = pane_id.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(label)
                                                .checked(current == selected)
                                                .on_click(move |_event, window, cx| {
                                                    let selected = selected.to_string();
                                                    let p_id = p_id.clone();
                                                    window.defer(cx, move |_window, cx| {
                                                        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                            let _ = global.view.update(cx, |view, cx| {
                                                                let mut config = serde_json::Map::new();
                                                                config.insert("priority".to_string(), serde_json::Value::String(selected.to_string()));
                                                                view.handle_pane_event(
                                                                    &crate::components::ui::pane_grid::renderer::PaneGridEvent::UpdateModuleConfig {
                                                                        pane_id: p_id,
                                                                        config: serde_json::Value::Object(config),
                                                                    },
                                                                    cx,
                                                                );
                                                            });
                                                        });
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                }
                            }),
                    )
                    .child(
                        Button::new("news-refresh")
                            .label(if is_narrow { "↻" } else { "REFRESH" })
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter = this
                                    .filter_input
                                    .as_ref()
                                    .map(|input| input.read(cx).value().trim().to_string())
                                    .unwrap_or_default();
                                this.restart(cx);
                            })),
                    ),
            );

        let mut state_id = 0;

        let body = if self.loading {
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Loading headlines")
                .into_any_element()
        } else if self.failed {
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Unable to load headlines")
                .into_any_element()
        } else if self.articles.is_empty() {
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("No headlines yet")
                .into_any_element()
        } else {
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .overflow_y_scrollbar()
                .bg(colors.background)
                .child(
                    div()
                        .w_full()
                        .when(self.width >= 900.0, |this| this.max_w(px(1100.0)).mx_auto())
                        .children(self.articles.iter().map(|article| {
                    let url = article.url.clone();
                    let id = ElementId::Name(format!("news-{}", article.id).into());
                    state_id += 1;
                    let local_state_id = state_id;
                    let title = clean_text(&article.title);
                    let category = article.category.replace("science-tech", "science/tech");
                    let time = published_time(article.published_at.as_deref(), &self.timezone);
                    let summary = article.summary.as_deref().map(clean_text).unwrap_or_default();
                    let bg_hover = colors.muted;

                    let summary_color = colors.foreground.opacity(0.75);

                    let source_upper = display_source(&article.source, &category);
                    let is_top = article.priority >= 60;
                    let flash_bg = gpui::rgba(0xf59e0b2e);
                    let flash_border = gpui::rgba(0xf59e0b73);

                    if is_compact_row {
                        div()
                            .id(id)
                            .w_full()
                            .px(px(8.0))
                            .py(px(7.0))
                            .border_b_1()
                            .border_color(colors.border.opacity(0.45))
                            .cursor_pointer()
                            .hover(|s| s.bg(bg_hover.opacity(0.45)))
                            .on_click(move |_, _, _| {
                                let _ = webbrowser::open(&url);
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(5.0))
                                            .min_w_0()
                                            .flex_1()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .font_family(cx.theme().mono_font_family.clone())
                                                    .text_size(px(9.5))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(colors.muted_foreground)
                                                    .child(time),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(9.5))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(colors.primary)
                                                    .child(source_upper),
                                            )
                                            .when(is_top, |el| {
                                                el.child(
                                                    div()
                                                        .px(px(3.0))
                                                        .py(px(0.5))
                                                        .bg(flash_bg)
                                                        .border_1()
                                                        .border_color(flash_border)
                                                        .text_size(px(8.0))
                                                        .font_weight(gpui::FontWeight::BOLD)
                                                        .text_color(gpui::rgb(0xf59e0b))
                                                        .child("FLASH"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px(px(4.0))
                                            .py(px(0.5))
                                            .bg(colors.muted.opacity(0.35))
                                            .border_1()
                                            .border_color(colors.border.opacity(0.6))
                                            .text_size(px(8.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors.muted_foreground)
                                            .child(category.to_uppercase()),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(3.0))
                                    .w_full()
                                    .text_size(px(12.0))
                                    .line_height(gpui::relative(1.28))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .child(title),
                            )
                            .when(!summary.is_empty(), |element| {
                                element.child(
                                    div()
                                        .pt(px(3.0))
                                        .text_size(px(10.5))
                                        .line_height(gpui::relative(1.3))
                                        .text_color(summary_color)
                                        .w_full()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .line_clamp(3)
                                        .child(summary),
                                )
                            })
                    } else {
                        div()
                            .id(id)
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(colors.border.opacity(0.45))
                            .cursor_pointer()
                            .hover(|s| s.bg(bg_hover.opacity(0.45)))
                            .on_click(move |_, _, _| {
                                let _ = webbrowser::open(&url);
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .min_w_0()
                                            .flex_1()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .font_family(cx.theme().mono_font_family.clone())
                                                    .text_size(px(10.0))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(colors.muted_foreground)
                                                    .child(time),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(10.0))
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(colors.primary)
                                                    .child(source_upper),
                                            )
                                            .when(is_top, |el| {
                                                el.child(
                                                    div()
                                                        .px(px(3.5))
                                                        .py(px(0.5))
                                                        .bg(flash_bg)
                                                        .border_1()
                                                        .border_color(flash_border)
                                                        .text_size(px(8.5))
                                                        .font_weight(gpui::FontWeight::BOLD)
                                                        .text_color(gpui::rgb(0xf59e0b))
                                                        .child("FLASH"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px(px(5.0))
                                            .py(px(1.0))
                                            .bg(colors.muted.opacity(0.35))
                                            .border_1()
                                            .border_color(colors.border.opacity(0.6))
                                            .text_size(px(9.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors.muted_foreground)
                                            .child(category.to_uppercase()),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(4.0))
                                    .w_full()
                                    .text_size(px(13.0))
                                    .line_height(gpui::relative(1.3))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .group_hover(format!("news_row_{local_state_id}"), |s| s.text_color(colors.primary))
                                    .child(title),
                            )
                            .when(!summary.is_empty(), |element| {
                                element.child(
                                    div()
                                        .pt(px(3.0))
                                        .text_size(px(11.0))
                                        .line_height(gpui::relative(1.32))
                                        .text_color(summary_color)
                                        .w_full()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .line_clamp(3)
                                        .child(summary),
                                )
                            })
                            .group(format!("news_row_{local_state_id}"))
                    }
                        })),
                )
                .into_any_element()
        };

        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(header)
            .child(body)
    }
}
