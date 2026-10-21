use chrono::Utc;
use chrono_tz::Tz;
use gpui::{div, px, Context, IntoElement, ParentElement, Render, Styled, Window};
use gpui_component::{theme::ActiveTheme, v_flex};

pub struct ClockPanel {
    timezone: String,
    width: f32,
    height: f32,
}

impl ClockPanel {
    pub fn new(_pane_id: String, timezone: String, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1000))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            timezone,
            width: 300.0,
            height: 200.0,
        }
    }

    pub fn set_timezone(&mut self, timezone: impl Into<String>) {
        let timezone = timezone.into();
        if self.timezone != timezone {
            self.timezone = timezone;
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
}

impl Render for ClockPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let now = Utc::now();
        let tz: Option<Tz> = self.timezone.parse().ok();
        let (date_str, time_str, offset_str, weekday_str) = match tz {
            Some(tz) => {
                let local = now.with_timezone(&tz);
                (
                    local.format("%d %b %Y").to_string(),
                    local.format("%H:%M:%S").to_string(),
                    local.format("UTC%:z").to_string(),
                    local.format("%A").to_string(),
                )
            }
            None => (
                now.format("%d %b %Y").to_string(),
                now.format("%H:%M:%S").to_string(),
                "UTC".to_string(),
                now.format("%A").to_string(),
            ),
        };

        let tz_label = self
            .timezone
            .split('/')
            .last()
            .unwrap_or(&self.timezone)
            .replace('_', " ");

        let is_horizontal = self.height < 95.0 || (self.width / self.height.max(1.0) > 3.0 && self.height < 140.0);

        if is_horizontal {
            let time_size = (self.height * 0.40).clamp(15.0, 32.0);
            let meta_size = (time_size * 0.42).clamp(9.0, 12.0);

            gpui_component::h_flex()
                .size_full()
                .items_center()
                .justify_between()
                .px(px(14.0))
                .bg(colors.background)
                .child(
                    gpui_component::h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(
                            div()
                                .text_size(px(meta_size))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(colors.primary)
                                .child(weekday_str.to_uppercase()),
                        )
                        .child(
                            div()
                                .text_size(px(meta_size))
                                .text_color(colors.muted_foreground)
                                .child(date_str),
                        ),
                )
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(time_size))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(colors.foreground)
                        .child(time_str),
                )
                .child(
                    gpui_component::h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded_sm()
                                .bg(colors.muted)
                                .text_size(px(meta_size))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors.foreground)
                                .child(tz_label),
                        )
                        .child(
                            div()
                                .text_size(px(meta_size - 1.0))
                                .text_color(colors.muted_foreground)
                                .child(offset_str),
                        ),
                )
        } else {
            let time_size = (self.width * 0.16).min(self.height * 0.28).clamp(18.0, 64.0);
            let date_size = (time_size * 0.32).clamp(10.0, 15.0);
            let badge_size = (time_size * 0.28).clamp(9.0, 13.0);
            let v_gap = (self.height * 0.02).clamp(2.0, 10.0);

            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .gap(px(v_gap))
                .child(
                    gpui_component::h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .text_size(px(date_size))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(colors.primary)
                                .child(weekday_str.to_uppercase()),
                        )
                        .child(
                            div()
                                .text_size(px(date_size))
                                .text_color(colors.muted_foreground)
                                .child(date_str),
                        ),
                )
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(time_size))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(colors.foreground)
                        .child(time_str),
                )
                .child(
                    gpui_component::h_flex()
                        .gap(px(6.0))
                        .items_center()
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(2.0))
                                .rounded_full()
                                .bg(colors.muted.opacity(0.8))
                                .border_1()
                                .border_color(colors.border.opacity(0.5))
                                .text_size(px(badge_size))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(colors.foreground)
                                .child(tz_label),
                        )
                        .child(
                            div()
                                .text_size(px(badge_size))
                                .text_color(colors.muted_foreground)
                                .child(offset_str),
                        ),
                )
        }
    }
}
