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

#[derive(Clone, Debug, Deserialize)]
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
    let mut output = String::with_capacity(value.len());
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        let status = if self.connected { "NEWS MONITOR" } else { "RECONNECTING" };
        let status_color = if self.connected { gpui::green() } else { colors.muted_foreground };
        let priority = self.priority.clone();
        let pane_id = self.pane_id.clone();
        let is_narrow = self.width < 340.0;
        let is_compact_row = self.width < 320.0;
        let header = div()
            .h_8()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_shrink_0()
                    .text_size(px(10.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.muted_foreground)
                    .child(div().size(px(6.0)).rounded_full().bg(status_color))
                    .when(!is_narrow, |this| this.child(status)),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Input::new(&input).small())
                    .child(
                        Button::new("news-category")
                            .label(if is_narrow {
                                priority.chars().take(3).collect::<String>().to_uppercase()
                            } else {
                                priority.to_uppercase()
                            })
                            .outline()
                            .small()
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
                            .small()
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
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Loading headlines")
                .into_any_element()
        } else if self.failed {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Unable to load headlines")
                .into_any_element()
        } else if self.articles.is_empty() {
            div()
                .size_full()
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
                .children(self.articles.iter().map(|article| {
                    let url = article.url.clone();
                    let id = ElementId::Name(format!("news-{}", article.id).into());
                    state_id += 1;
                    let local_state_id = state_id;
                    let title = clean_text(&article.title);
                    let source = clean_text(&article.source);
                    let category = article.category.replace("science-tech", "science/tech");
                    let time = published_time(article.published_at.as_deref(), &self.timezone);
                    let summary = article.summary.as_deref().map(|s| clean_text(s)).unwrap_or_default();
                    let top = (article.priority >= 60).then_some("TOP").unwrap_or("");
                    let bg_hover = colors.muted;

                    let summary_color = colors.foreground.opacity(0.75);

                    if is_compact_row {
                        div()
                            .id(id)
                            .w_full()
                            .px_2()
                            .py(px(6.0))
                            .border_b_1()
                            .border_color(colors.border.opacity(0.7))
                            .cursor_pointer()
                            .hover(|s| s.bg(bg_hover.opacity(0.6)))
                            .on_click(move |_, _, _| {
                                let _ = webbrowser::open(&url);
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .max_w(px(110.0))
                                                    .overflow_hidden()
                                                    .text_size(px(9.5))
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(colors.primary)
                                                    .child(source),
                                            )
                                            .child(
                                                div()
                                                    .rounded_sm()
                                                    .bg(colors.muted)
                                                    .px(px(3.0))
                                                    .py(px(1.0))
                                                    .text_size(px(8.5))
                                                    .text_color(colors.foreground.opacity(0.7))
                                                    .child(category.to_uppercase()),
                                            )
                                            .when(!top.is_empty(), |element| {
                                                element.child(
                                                    div()
                                                        .text_size(px(8.5))
                                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                                        .text_color(gpui::rgb(0xf59e0b))
                                                        .child(top),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(9.0))
                                            .text_color(colors.muted_foreground)
                                            .child(time),
                                    ),
                            )
                            .child(
                                div()
                                    .pt_1()
                                    .w_full()
                                    .text_size(px(12.0))
                                    .line_height(gpui::relative(1.2))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .child(title),
                            )
                            .when(!summary.is_empty(), |element| {
                                element.child(
                                    div()
                                        .pt_1()
                                        .text_size(px(10.5))
                                        .line_height(gpui::relative(1.2))
                                        .text_color(summary_color)
                                        .child(summary),
                                )
                            })
                    } else {
                        div()
                            .id(id)
                            .w_full()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(colors.border.opacity(0.7))
                            .cursor_pointer()
                            .hover(|s| s.bg(bg_hover.opacity(0.6)))
                            .on_click(move |_, _, _| {
                                let _ = webbrowser::open(&url);
                            })
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(10.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors.foreground.opacity(0.7))
                                            .child(time),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(144.0))
                                            .flex_shrink_0()
                                            .overflow_hidden()
                                            .text_size(px(10.0))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(colors.primary)
                                            .child(source),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .rounded_sm()
                                            .bg(colors.muted)
                                            .px(px(4.0))
                                            .py(px(2.0))
                                            .text_size(px(9.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(colors.foreground.opacity(0.7))
                                            .child(category.to_uppercase()),
                                    )
                                    .when(!top.is_empty(), |element| {
                                        element.child(
                                            div()
                                                .flex_shrink_0()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(gpui::rgb(0xf59e0b)) // amber-500
                                                .child(top),
                                        )
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_size(px(13.0))
                                            .line_height(gpui::relative(1.2))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(colors.foreground)
                                            .group_hover(format!("news_row_{local_state_id}"), |s| s.text_color(colors.primary))
                                            .child(title),
                                    ),
                            )
                            .when(!summary.is_empty(), |element| {
                                element.child(
                                    div()
                                        .pt_2()
                                        .pl(px(52.0))
                                        .text_xs()
                                        .line_height(gpui::relative(1.25))
                                        .text_color(summary_color)
                                        .child(summary),
                                )
                            })
                            .group(format!("news_row_{local_state_id}"))
                    }
                }))
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
