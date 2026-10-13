use crate::data::update_manager::UpdateManager;
use crate::data::version::UpdateState;
use gpui::{
    Context, EventEmitter, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window,
    div, px, rgb,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use std::sync::Arc;

/// Event emitted by the update banner.
pub enum UpdateBannerEvent {
    /// User clicked "Download" — MainView should start the download.
    DownloadRequested,
    /// User clicked "Install" — MainView should install the downloaded update.
    InstallRequested(String),
    /// User clicked "Restart Now" after update installation.
    RestartRequested,
    /// User dismissed the banner.
    Dismissed,
    /// User clicked "Check Now" (from idle/failed state).
    CheckRequested,
}

impl EventEmitter<UpdateBannerEvent> for UpdateBanner {}

/// A thin notification banner that appears at the top of the content area
/// when an update is available, downloading, or ready to install.
pub struct UpdateBanner {
    update_manager: Arc<UpdateManager>,
}

impl UpdateBanner {
    pub fn new(update_manager: Arc<UpdateManager>) -> Self {
        Self { update_manager }
    }
}

impl Render for UpdateBanner {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.update_manager.state();

        match state {
            UpdateState::Idle | UpdateState::Checking => div().id("update-banner-idle"),
            UpdateState::Available(info) => {
                let version = info.version.clone();
                let changelog_preview = info
                    .changelog
                    .as_deref()
                    .unwrap_or("Bug fixes and improvements")
                    .lines()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");

                let banner_bg = rgb(0x1a3a2a);
                let text_color = rgb(0x4ade80);

                div()
                    .id("update-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px(px(16.0))
                    .py(px(8.0))
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(rgb(0x22543d))
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(text_color)
                            .child(format!("Update v{} available", version)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(0x86efac))
                            .child(changelog_preview),
                    )
                    .child(
                        Button::new("download-update")
                            .label("Download")
                            .small()
                            .primary()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::DownloadRequested);
                            })),
                    )
                    .child(
                        Button::new("dismiss-update")
                            .label("Dismiss")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::Dismissed);
                            })),
                    )
            }
            UpdateState::Downloading { version, progress } => {
                let banner_bg = rgb(0x1a2a3a);
                let pct = (progress * 100.0) as u32;

                div()
                    .id("update-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px(px(16.0))
                    .py(px(8.0))
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(rgb(0x1e3a5f))
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x60a5fa))
                            .child(format!("Downloading v{}...", version)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(4.0))
                            .bg(rgb(0x1e293b))
                            .rounded(px(2.0))
                            .child(
                                div()
                                    .h_full()
                                    .bg(rgb(0x3b82f6))
                                    .rounded(px(2.0))
                                    .w(gpui::relative(progress as f32)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x93c5fd))
                            .child(format!("{}%", pct)),
                    )
            }
            UpdateState::ReadyToInstall { version, path } => {
                let banner_bg = rgb(0x2a1a3a);
                let install_path = path.clone();

                div()
                    .id("update-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px(px(16.0))
                    .py(px(8.0))
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(rgb(0x3b1f5e))
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xc084fc))
                            .child(format!("v{} ready to install", version)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("install-update")
                            .label("Install Now")
                            .small()
                            .primary()
                            .on_click(cx.listener(move |_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::InstallRequested(install_path.clone()));
                            })),
                    )
                    .child(
                        Button::new("dismiss-install")
                            .label("Later")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::Dismissed);
                            })),
                    )
            }
            UpdateState::RestartRequired { version } => {
                let banner_bg = rgb(0x1a3a2a);

                div()
                    .id("update-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px(px(16.0))
                    .py(px(8.0))
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(rgb(0x22543d))
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x4ade80))
                            .child(format!("v{} installed! Restart application to apply changes.", version)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("restart-app")
                            .label("Restart Now")
                            .small()
                            .primary()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::RestartRequested);
                            })),
                    )
                    .child(
                        Button::new("dismiss-restart")
                            .label("Later")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::Dismissed);
                            })),
                    )
            }
            UpdateState::Failed(msg) => {
                let banner_bg = rgb(0x3a1a1a);

                div()
                    .id("update-banner")
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .px(px(16.0))
                    .py(px(6.0))
                    .bg(banner_bg)
                    .border_b_1()
                    .border_color(rgb(0x5f1e1e))
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xfca5a5))
                            .child(format!("Update check failed: {}", msg)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("retry-update")
                            .label("Retry")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::CheckRequested);
                            })),
                    )
                    .child(
                        Button::new("dismiss-error")
                            .label("Dismiss")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|_this, _event, _window, cx| {
                                cx.emit(UpdateBannerEvent::Dismissed);
                            })),
                    )
            }
        }
    }
}
