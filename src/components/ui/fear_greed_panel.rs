use gpui::{
    div, px, Context, IntoElement, ParentElement, Render, Styled, Window,
    SharedString, rgba, prelude::FluentBuilder, Hsla
};
use gpui_component::theme::ActiveTheme;
use serde::Deserialize;

use crate::data::client::tokio_runtime;

#[derive(Deserialize, Debug, Clone)]
struct FngData {
    value: String,
    value_classification: String,
    timestamp: String,
}

#[derive(Deserialize, Debug, Clone)]
struct FngResponse {
    data: Vec<FngData>,
}

#[derive(Clone)]

pub struct FearGreedPanel {
    value: Option<f32>,
    classification: Option<SharedString>,
    updated_at: Option<chrono::DateTime<chrono::Local>>,
    is_loading: bool,
    error: Option<SharedString>,
    width: f32,
    height: f32,
}

impl FearGreedPanel {
    pub fn agent_snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "value": self.value,
            "classification": self.classification.as_ref().map(|s| s.to_string()),
            "updated_at": self.updated_at.map(|dt| dt.to_rfc3339()),
            "loading": self.is_loading
        })
    }

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

        let mut panel = Self {
            value: None,
            classification: None,
            updated_at: None,
            is_loading: true,
            error: None,
            width: 300.0,
            height: 200.0,
        };

        panel.fetch_data(cx);
        panel
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

    fn fetch_data(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = tokio_runtime().spawn(async move {
                let res = reqwest::get("https://api.alternative.me/fng/?limit=1")
                    .await
                    .and_then(|r| r.error_for_status());
                    
                match res {
                    Ok(r) => {
                        match r.json::<FngResponse>().await {
                            Ok(json) => Ok(json),
                            Err(_) => Err("Failed to parse JSON".to_string()),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                }
            }).await.unwrap_or_else(|e| Err(e.to_string()));

            let _ = this.update(cx, |this, cx| {
                this.is_loading = false;
                match result {
                    Ok(resp) => {
                        if let Some(d) = resp.data.first() {
                            this.value = d.value.parse::<f32>().ok();
                            this.classification = Some(d.value_classification.clone().into());
                            
                            if let Ok(ts) = d.timestamp.parse::<i64>() {
                                this.updated_at = chrono::DateTime::from_timestamp(ts, 0)
                                    .map(|dt| dt.with_timezone(&chrono::Local));
                            }
                            this.error = None;
                        } else {
                            this.error = Some("No data returned".into());
                        }
                    }
                    Err(e) => {
                        this.error = Some(e.into());
                    }
                }
                cx.notify();
            });
        }).detach();
    }
}

impl Render for FearGreedPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;

        let credit_error = crate::data::credits::CreditStore::module_error("FearGreed");
        if let Some(ref ce) = credit_error {
            return div()
                .size_full()
                .bg(colors.background)
                .child(crate::components::ui::credit_alert::render_credits_exhausted_empty(
                    "Fear & Greed",
                    Some(ce),
                    self.height < 120.0 || self.width < 200.0,
                    cx.theme(),
                ))
                .into_any_element();
        }

        if self.is_loading {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child("Loading index..."),
                )
                .into_any_element();
        }

        if let Some(err) = &self.error {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    div()
                        .text_sm()
                        .text_color(rgba(0xef4444ff))
                        .child(err.clone()),
                )
                .into_any_element();
        }

        let val = self.value.unwrap_or(0.0);
        let classification = self.classification.as_ref().map(|s| s.as_ref()).unwrap_or("Unknown");
        let val_pct = (val / 100.0).clamp(0.0, 1.0);

        let sentiment_color: Hsla = if val < 25.0 {
            gpui::rgba(0xe85d38ff).into()
        } else if val < 45.0 {
            gpui::rgba(0xf18545ff).into()
        } else if val < 55.0 {
            gpui::rgba(0x9ca3afff).into()
        } else if val < 75.0 {
            gpui::rgba(0x8bc34aff).into()
        } else {
            gpui::rgba(0x28a745ff).into()
        };

        let render_gauge_track = |h_track: f32, h_thumb: f32| {
            div()
                .relative()
                .w_full()
                .h(px(h_thumb))
                .child(
                    // Track
                    div()
                        .absolute()
                        .top(px((h_thumb - h_track) / 2.0))
                        .w_full()
                        .h(px(h_track))
                        .flex()
                        .overflow_hidden()
                        .child(div().w(gpui::relative(0.25)).h_full().bg(gpui::rgba(0xe85d38ff)))
                        .child(div().w(gpui::relative(0.20)).h_full().bg(gpui::rgba(0xf18545ff)))
                        .child(div().w(gpui::relative(0.10)).h_full().bg(gpui::rgba(0x9ca3afff)))
                        .child(div().w(gpui::relative(0.20)).h_full().bg(gpui::rgba(0x8bc34aff)))
                        .child(div().w(gpui::relative(0.25)).h_full().bg(gpui::rgba(0x28a745ff))),
                )
                .child(
                    // Thumb
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(gpui::relative(val_pct))
                        .ml(px(-4.))
                        .w(px(8.))
                        .h(px(h_thumb))
                        .bg(gpui::white())
                        .border_1()
                        .border_color(gpui::rgba(0x00000044))
                        .shadow_md(),
                )
        };

        let is_horizontal = self.height < 140.0 || (self.width >= 320.0 && self.height < 190.0);

        if is_horizontal {
            let score_size = (self.height * 0.32).clamp(24.0, 42.0);
            gpui_component::h_flex()
                .size_full()
                .bg(colors.background)
                .items_center()
                .px(px(14.0))
                .gap(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .w(px(90.0))
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(score_size))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(sentiment_color)
                                .child(format!("{:.0}", val)),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .bg(sentiment_color.opacity(0.15))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(sentiment_color)
                                .child(classification.to_uppercase()),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(render_gauge_track(8.0, 16.0))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .w_full()
                                .text_size(px(9.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(div().text_color(gpui::rgba(0xe85d38ff)).child("Ext. Fear"))
                                .child(div().text_color(gpui::rgba(0x9ca3afff)).child("Neutral"))
                                .child(div().text_color(gpui::rgba(0x28a745ff)).child("Ext. Greed")),
                        )
                        .when_some(self.updated_at, |this, dt| {
                            this.child(
                                div()
                                    .text_color(colors.muted_foreground)
                                    .text_size(px(9.0))
                                    .child(format!("Updated {}", dt.format("%b %-d, %I:%M %p"))),
                            )
                        }),
                )
                .into_any_element()
        } else {
            let val_size = (self.width * 0.16).min(self.height * 0.24).clamp(26.0, 54.0);
            let class_size = (val_size * 0.30).clamp(10.0, 13.0);
            let track_margin = (self.height * 0.06).clamp(8.0, 18.0);
            let pad = (self.width * 0.06).min(self.height * 0.06).clamp(8.0, 20.0);

            div()
                .size_full()
                .bg(colors.background)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(pad))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(val_size))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(sentiment_color)
                                .child(format!("{:.0}", val)),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(2.0))
                                .bg(sentiment_color.opacity(0.14))
                                .text_size(px(class_size))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(sentiment_color)
                                .child(classification.to_uppercase()),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .max_w(px(320.0))
                        .mt(px(track_margin))
                        .flex()
                        .flex_col()
                        .child(render_gauge_track(8.0, 16.0))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .w_full()
                                .mt(px(6.0))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(div().text_color(gpui::rgba(0xe85d38ff)).child("Ext. Fear"))
                                .child(div().text_color(gpui::rgba(0x9ca3afff)).child("Neutral"))
                                .child(div().text_color(gpui::rgba(0x28a745ff)).child("Ext. Greed")),
                        ),
                )
                .when_some(self.updated_at, |this, dt| {
                    if self.height >= 170.0 {
                        this.child(
                            div()
                                .mt(px(track_margin * 0.8))
                                .text_color(colors.muted_foreground)
                                .text_size(px(10.0))
                                .child(format!("Updated {}", dt.format("%b %-d, %I:%M %p"))),
                        )
                    } else {
                        this
                    }
                })
                .into_any_element()
        }
    }
}
