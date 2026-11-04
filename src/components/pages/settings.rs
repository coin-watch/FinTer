use crate::components::pages::api_management_view::ApiManagementView;
use crate::data::models::SettingsData;
use crate::data::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::checkbox::Checkbox;
use gpui_component::popover::Popover;
use gpui_component::scroll::ScrollableElement;
use gpui_component::Sizable;
use gpui_component::{
    setting::{
        NumberFieldOptions, RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings,
    },
    ActiveTheme,
};
use std::sync::Arc;

type OnChangeFn = Option<Arc<dyn Fn(&mut App) + Send + Sync>>;

fn finter_timezones() -> Vec<(SharedString, SharedString)> {
    [
        ("UTC", "UTC"),
        ("GMT", "GMT"),
        ("America/New_York", "New York, United States"),
        ("America/Chicago", "Chicago, United States"),
        ("America/Denver", "Denver, United States"),
        ("America/Los_Angeles", "Los Angeles, United States"),
        ("America/Toronto", "Toronto, Canada"),
        ("America/Vancouver", "Vancouver, Canada"),
        ("America/Sao_Paulo", "São Paulo, Brazil"),
        ("America/Mexico_City", "Mexico City, Mexico"),
        ("America/Buenos_Aires", "Buenos Aires, Argentina"),
        ("Europe/London", "London, United Kingdom"),
        ("Europe/Paris", "Paris, France"),
        ("Europe/Berlin", "Berlin, Germany"),
        ("Europe/Rome", "Rome, Italy"),
        ("Europe/Madrid", "Madrid, Spain"),
        ("Europe/Amsterdam", "Amsterdam, Netherlands"),
        ("Europe/Zurich", "Zurich, Switzerland"),
        ("Europe/Vienna", "Vienna, Austria"),
        ("Europe/Stockholm", "Stockholm, Sweden"),
        ("Europe/Oslo", "Oslo, Norway"),
        ("Europe/Copenhagen", "Copenhagen, Denmark"),
        ("Europe/Helsinki", "Helsinki, Finland"),
        ("Europe/Warsaw", "Warsaw, Poland"),
        ("Europe/Prague", "Prague, Czech Republic"),
        ("Europe/Budapest", "Budapest, Hungary"),
        ("Europe/Athens", "Athens, Greece"),
        ("Europe/Istanbul", "Istanbul, Turkey"),
        ("Europe/Moscow", "Moscow, Russia"),
        ("Asia/Tokyo", "Tokyo, Japan"),
        ("Asia/Shanghai", "Shanghai, China"),
        ("Asia/Hong_Kong", "Hong Kong"),
        ("Asia/Singapore", "Singapore"),
        ("Asia/Seoul", "Seoul, South Korea"),
        ("Asia/Taipei", "Taipei, Taiwan"),
        ("Asia/Bangkok", "Bangkok, Thailand"),
        ("Asia/Jakarta", "Jakarta, Indonesia"),
        ("Asia/Kuala_Lumpur", "Kuala Lumpur, Malaysia"),
        ("Asia/Manila", "Manila, Philippines"),
        ("Asia/Ho_Chi_Minh", "Ho Chi Minh City, Vietnam"),
        ("Asia/Mumbai", "Mumbai, India"),
        ("Asia/Kolkata", "Kolkata, India"),
        ("Asia/Dubai", "Dubai, United Arab Emirates"),
        ("Asia/Tehran", "Tehran, Iran"),
        ("Asia/Karachi", "Karachi, Pakistan"),
        ("Asia/Dhaka", "Dhaka, Bangladesh"),
        ("Australia/Sydney", "Sydney, Australia"),
        ("Australia/Melbourne", "Melbourne, Australia"),
        ("Australia/Perth", "Perth, Australia"),
        ("Australia/Brisbane", "Brisbane, Australia"),
        ("Australia/Adelaide", "Adelaide, Australia"),
        ("Pacific/Auckland", "Auckland, New Zealand"),
        ("Pacific/Fiji", "Fiji"),
        ("Pacific/Honolulu", "Honolulu, United States"),
        ("Africa/Cairo", "Cairo, Egypt"),
        ("Africa/Johannesburg", "Johannesburg, South Africa"),
        ("Africa/Lagos", "Lagos, Nigeria"),
        ("Africa/Nairobi", "Nairobi, Kenya"),
        ("Africa/Casablanca", "Casablanca, Morocco"),
        ("Africa/Tunis", "Tunis, Tunisia"),
        ("Africa/Algiers", "Algiers, Algeria"),
    ]
    .into_iter()
    .map(|(value, label)| (value.into(), label.into()))
    .collect()
}

#[derive(Clone)]
pub struct AppSettings {
    pub server_url: SharedString,
    pub dark_mode: bool,
    pub theme: SharedString,
    pub enable_animations: bool,
    pub default_exchange: SharedString,
    pub default_trading_pair: SharedString,
    pub max_fps: f64,
    pub enable_price_alerts: bool,
    pub enable_desktop_notifications: bool,
    pub enable_sound_alerts: bool,
    pub auto_save_dashboard: bool,
    pub data_update_frequency: f64,
    pub enable_chart_grid: bool,
    pub enable_magnet_snap: bool,
    pub auto_check_updates: bool,
    pub update_channel: SharedString,
    pub sync_dashboard_tabs: bool,
    pub default_timezone: gpui::SharedString,
    /// Local-only startup tab behavior; never included in SettingsData.
    pub startup_tabs_mode: SharedString,
    /// Local-only dashboard ids selected for `StartupTabMode::Selected`.
    pub startup_tab_ids: Vec<String>,
    /// Local-only Agent configuration; never included in SettingsData.
    pub agent_base_url: SharedString,
    pub agent_api_key: SharedString,
    /// Local-only exchange API credentials.
    pub api_exchange: SharedString,
    pub api_key: SharedString,
    pub api_secret: SharedString,
    pub api_passphrase: SharedString,
}

impl Default for AppSettings {
    fn default() -> Self {
        let saved_creds = crate::data::api_management::load();
        let tab_preferences =
            crate::data::tab_settings::load(crate::data::tab_settings::TabProfile::Guest);
        let initial_ex = saved_creds
            .first()
            .map(|c| c.exchange.clone())
            .unwrap_or_else(|| "binance".to_string());
        let initial_cred = saved_creds
            .iter()
            .find(|c| c.exchange.eq_ignore_ascii_case(&initial_ex))
            .cloned()
            .unwrap_or_default();

        Self {
            server_url: crate::data::client::default_api_url().into(),
            dark_mode: false,
            theme: "gruvbox".into(),
            enable_animations: true,
            default_exchange: "binance".into(),
            default_trading_pair: "BTC/USDT".into(),
            max_fps: 60.0,
            enable_price_alerts: true,
            enable_desktop_notifications: true,
            enable_sound_alerts: true,
            auto_save_dashboard: true,
            data_update_frequency: 100.0,
            enable_chart_grid: true,
            enable_magnet_snap: true,
            auto_check_updates: true,
            update_channel: crate::data::version::selected_channel().as_str().into(),
            sync_dashboard_tabs: true,
            default_timezone: "UTC".into(),
            startup_tabs_mode: tab_preferences.startup_mode.as_str().into(),
            startup_tab_ids: tab_preferences.selected_tab_ids,
            agent_base_url: std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()).into(),
            agent_api_key: std::env::var("FINTER_AI_API_KEY").unwrap_or_default().into(),
            api_exchange: initial_ex.into(),
            api_key: initial_cred.api_key.into(),
            api_secret: initial_cred.api_secret.into(),
            api_passphrase: initial_cred.passphrase.into(),
        }
    }
}

impl AppSettings {
    pub fn from_settings_data(
        data: &SettingsData,
        profile: crate::data::tab_settings::TabProfile,
    ) -> Self {
        let tab_preferences = crate::data::tab_settings::load(profile);
        Self {
            server_url: data
                .server_url
                .as_deref()
                .and_then(crate::data::client::normalize_api_origin)
                .unwrap_or_else(crate::data::client::default_api_url)
                .into(),
            dark_mode: data.dark_mode.as_deref() == Some("dark"),
            theme: data
                .theme
                .clone()
                .unwrap_or_else(|| "gruvbox".into())
                .into(),
            enable_animations: data.enable_animations.unwrap_or(true),
            default_exchange: data
                .default_ticker_exchange
                .clone()
                .unwrap_or_else(|| "binance".into())
                .into(),
            default_trading_pair: data
                .default_trading_pair
                .clone()
                .unwrap_or_else(|| "BTC/USDT".into())
                .into(),
            max_fps: data.max_fps.unwrap_or(60) as f64,
            enable_price_alerts: data.enable_price_alerts.unwrap_or(true),
            enable_desktop_notifications: data.alert_desktop_notifications.unwrap_or(true),
            enable_sound_alerts: data.alert_sound_enabled.unwrap_or(true),
            auto_save_dashboard: data.auto_backup_enabled.unwrap_or(true),
            data_update_frequency: data.data_update_frequency.unwrap_or(100) as f64,
            enable_chart_grid: data.chart_grid_visible.unwrap_or(false),
            enable_magnet_snap: data.magnet_snap_enabled.unwrap_or(true),
            sync_dashboard_tabs: data.link_dashboard_tabs.unwrap_or(true),
            default_timezone: data
                .timezone
                .clone()
                .unwrap_or_else(|| "UTC".into())
                .into(),
            auto_check_updates: data.auto_check_updates.unwrap_or(true),
            update_channel: data
                .update_channel
                .as_deref()
                .and_then(crate::data::version::UpdateChannel::parse)
                .unwrap_or_else(crate::data::version::selected_channel)
                .as_str()
                .into(),
            startup_tabs_mode: tab_preferences.startup_mode.as_str().into(),
            startup_tab_ids: tab_preferences.selected_tab_ids,
            agent_base_url: std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()).into(),
            agent_api_key: std::env::var("FINTER_AI_API_KEY").unwrap_or_default().into(),
            api_exchange: crate::data::api_management::load().first().map(|c| c.exchange.clone()).unwrap_or_else(|| "binance".into()).into(),
            api_key: String::new().into(),
            api_secret: String::new().into(),
            api_passphrase: String::new().into(),
        }
    }

    pub fn to_settings_data(&self) -> SettingsData {
        SettingsData {
            server_url: crate::data::client::normalize_api_origin(&self.server_url),
            dark_mode: Some(if self.dark_mode {
                "dark".into()
            } else {
                "light".into()
            }),
            theme: Some(self.theme.to_string()),
            enable_animations: Some(self.enable_animations),
            // Map desktop "default_exchange" → web "defaultTickerExchange"
            default_ticker_exchange: Some(self.default_exchange.to_string()),
            default_trading_pair: Some(self.default_trading_pair.to_string()),
            max_fps: Some(self.max_fps as u32),
            enable_price_alerts: Some(self.enable_price_alerts),
            alert_desktop_notifications: Some(self.enable_desktop_notifications),
            alert_sound_enabled: Some(self.enable_sound_alerts),
            auto_backup_enabled: Some(self.auto_save_dashboard),
            data_update_frequency: Some(self.data_update_frequency as u32),
            chart_grid_visible: Some(self.enable_chart_grid),
            magnet_snap_enabled: Some(self.enable_magnet_snap),
            link_dashboard_tabs: Some(self.sync_dashboard_tabs),
            timezone: Some(self.default_timezone.to_string()),
            auto_check_updates: Some(self.auto_check_updates),
            update_channel: Some(self.update_channel.to_string()),
            ..Default::default()
        }
    }
}

/// Multi-select control for the dashboards that should open on startup.
///
/// The trigger summarizes the current selection and opens a checkbox list.
/// Selections are written to the draft settings entity immediately; the Tabs
/// page's Save Section button commits them.
fn dashboard_multi_select(
    settings: Entity<AppSettings>,
    dashboards: Vec<(String, String)>,
    options: &RenderOptions,
    cx: &mut App,
) -> AnyElement {
    let selected_ids = settings.read(cx).startup_tab_ids.clone();
    let selected_count = dashboards
        .iter()
        .filter(|(id, _)| selected_ids.contains(id))
        .count();
    let label = if dashboards.is_empty() {
        "No dashboards available".to_string()
    } else if selected_count == 0 {
        "Select dashboards".to_string()
    } else if selected_count == dashboards.len() {
        "All dashboards".to_string()
    } else {
        format!("{selected_count} dashboards selected")
    };

    let trigger = Button::new("tabs-dashboard-multiselect")
        .when(options.layout == Axis::Vertical, |this| this.w_full())
        .label(label)
        .dropdown_caret(true)
        .outline()
        .with_size(options.size);

    let content_settings = settings.clone();
    let content_dashboards = dashboards.clone();

    Popover::new("tabs-dashboard-multiselect-popover")
        .anchor(Corner::TopRight)
        .trigger(trigger)
        .content(move |_state, _window, cx| {
            let settings = content_settings.clone();
            let dashboards = content_dashboards.clone();
            let selected_ids = settings.read(cx).startup_tab_ids.clone();
            let all_selected = !dashboards.is_empty()
                && dashboards
                    .iter()
                    .all(|(id, _)| selected_ids.contains(id));

            let select_all_settings = settings.clone();
            let select_all_dashboards = dashboards.clone();
            let select_all = Checkbox::new("tabs-dashboard-select-all")
                .label(if all_selected {
                    "Clear all"
                } else {
                    "Select all"
                })
                .checked(all_selected)
                .on_click(move |checked, _window, cx| {
                    let ids = select_all_dashboards
                        .iter()
                        .map(|(id, _)| id.clone())
                        .collect::<Vec<_>>();
                    select_all_settings.update(cx, |saved, _| {
                        if *checked {
                            for id in ids {
                                if !saved.startup_tab_ids.contains(&id) {
                                    saved.startup_tab_ids.push(id);
                                }
                            }
                        } else {
                            saved.startup_tab_ids.clear();
                        }
                    });
                    cx.refresh_windows();
                });

            let mut list = div()
                .w(px(280.0))
                .max_h(px(320.0))
                .overflow_y_scrollbar()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .p(px(6.0))
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(4.0))
                        .border_b_1()
                        .border_color(cx.theme().colors.border)
                        .child(select_all),
                );

            for (id, name) in dashboards {
                let checked = selected_ids.contains(&id);
                let checkbox_settings = settings.clone();
                let checkbox_id = id.clone();
                list = list.child(
                    div().px(px(6.0)).py(px(3.0)).child(
                        Checkbox::new(ElementId::Name(SharedString::from(format!(
                            "tabs-dashboard-{id}"
                        ))))
                        .label(name)
                        .checked(checked)
                        .on_click(move |checked, _window, cx| {
                            checkbox_settings.update(cx, |saved, _| {
                                if *checked {
                                    if !saved.startup_tab_ids.contains(&checkbox_id) {
                                        saved.startup_tab_ids.push(checkbox_id.clone());
                                    }
                                } else {
                                    saved
                                        .startup_tab_ids
                                        .retain(|saved_id| saved_id != &checkbox_id);
                                }
                            });
                            cx.refresh_windows();
                        }),
                    ),
                );
            }

            list.into_any_element()
        })
        .into_any_element()
}

pub struct SettingsPage {
    settings: Entity<AppSettings>,
    committed: Entity<AppSettings>,
    on_change: OnChangeFn,
    api_management: Entity<ApiManagementView>,
    /// Live dashboard list used by the Tabs settings page.
    dashboards: Vec<(String, String)>,
}

impl SettingsPage {
    pub fn new(cx: &mut App) -> Self {
        Self {
            settings: cx.new(|_| AppSettings::default()),
            committed: cx.new(|_| AppSettings::default()),
            on_change: None,
            api_management: cx.new(|cx| ApiManagementView::new(cx)),
            dashboards: Vec::new(),
        }
    }

    pub fn new_with_data(
        data: &SettingsData,
        profile: crate::data::tab_settings::TabProfile,
        cx: &mut App,
    ) -> Self {
        Self {
            settings: cx.new(|_| AppSettings::from_settings_data(data, profile)),
            committed: cx.new(|_| AppSettings::from_settings_data(data, profile)),
            on_change: None,
            api_management: cx.new(|cx| ApiManagementView::new(cx)),
            dashboards: Vec::new(),
        }
    }

    pub fn with_dashboards(mut self, dashboards: Vec<(String, String)>) -> Self {
        self.dashboards = dashboards;
        self
    }

    /// Refresh the dashboard list shown on the Tabs settings page.
    pub fn update_dashboards(&mut self, dashboards: Vec<(String, String)>, cx: &mut Context<Self>) {
        self.dashboards = dashboards;
        cx.notify();
    }

    /// Read the local-only startup tab preference from the committed settings.
    pub fn tab_preferences(
        &self,
        cx: &App,
    ) -> (crate::data::tab_settings::StartupTabMode, Vec<String>) {
        let settings = self.committed.read(cx);
        (
            crate::data::tab_settings::StartupTabMode::from_value(&settings.startup_tabs_mode),
            settings.startup_tab_ids.clone(),
        )
    }

    pub fn with_on_change(mut self, callback: Arc<dyn Fn(&mut App) + Send + Sync>) -> Self {
        self.on_change = Some(callback);
        self
    }

    #[allow(dead_code)]
    fn trigger_on_change(&self, cx: &mut App) {
        if let Some(ref callback) = self.on_change {
            callback(cx);
        }
    }

    pub fn settings(&self) -> &Entity<AppSettings> {
        &self.settings
    }

    pub fn get_settings_data(&self, cx: &App) -> SettingsData {
        self.committed.read(cx).to_settings_data()
    }

    fn build_settings(&self, _cx: &App) -> Settings {
        let settings = self.settings.clone();
        let on_change = self.on_change.clone();

        Settings::new("app-settings")
            .sidebar_width(px(220.))
            .page(self.build_appearance_page(settings.clone(), on_change.clone()))
            .page(self.build_network_page(settings.clone(), on_change.clone()))
            .page(self.build_agent_page(settings.clone()))
            .page(self.build_api_management_page(settings.clone()))
            .page(self.build_trading_page(settings.clone(), on_change.clone()))
            .page(self.build_alerts_page(settings.clone(), on_change.clone()))
            .page(self.build_performance_page(settings.clone(), on_change.clone()))
            .page(self.build_layout_page(settings.clone(), on_change.clone()))
            .page(self.build_tabs_page(settings.clone(), on_change.clone()))
            .page(self.build_updates_page(settings.clone(), on_change.clone()))
    }

    fn save_section(&self, section: &'static str, draft: Entity<AppSettings>, callback: OnChangeFn) -> SettingGroup {
        let committed = self.committed.clone();
        SettingGroup::new().item(SettingItem::render(move |_, _, _| {
            let draft=draft.clone(); let committed=committed.clone(); let callback=callback.clone();
            gpui_component::button::Button::new(SharedString::from(format!("save-{section}")))
                .label("Save Section")
                .on_click(move |_, _, cx| {
                    let values=draft.read(cx).clone();
                    committed.update(cx, |saved, _| {
                        match section {
                            "network" => { saved.server_url = values.server_url.clone(); },
                            "appearance" => { saved.theme = values.theme.clone(); saved.dark_mode = values.dark_mode.clone(); saved.enable_animations = values.enable_animations.clone(); },
                            "trading" => { saved.default_exchange = values.default_exchange.clone(); saved.default_trading_pair = values.default_trading_pair.clone(); },
                            "alerts" => { saved.enable_price_alerts = values.enable_price_alerts.clone(); saved.enable_sound_alerts = values.enable_sound_alerts.clone(); saved.enable_desktop_notifications = values.enable_desktop_notifications.clone(); },
                            "performance" => { saved.max_fps = values.max_fps.clone(); saved.data_update_frequency = values.data_update_frequency.clone(); },
                            "layout" => { saved.sync_dashboard_tabs = values.sync_dashboard_tabs.clone(); saved.default_timezone = values.default_timezone.clone(); saved.auto_save_dashboard = values.auto_save_dashboard.clone(); },
                            "tabs" => { saved.startup_tabs_mode = values.startup_tabs_mode.clone(); saved.startup_tab_ids = values.startup_tab_ids.clone(); },
                            "updates" => { saved.auto_check_updates = values.auto_check_updates.clone(); saved.update_channel = values.update_channel.clone(); },
                            _ => {}
                        }
                    });
                    if let Some(cb)=&callback { cb(cx); }
                })
        }))
    }

    fn build_network_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        let settings_clone = settings.clone();

        SettingPage::new("Network & Server")
            .description("Configure backend server API endpoint for cloud sync and account features")
            .group(
                SettingGroup::new()
                    .title("Backend Service")
                    .item(
                        SettingItem::new(
                            "API Server URL",
                            SettingField::input(
                                {
                                    let settings = settings_clone.clone();
                                    move |cx| settings.read(cx).server_url.clone()
                                },
                                {
                                    let settings = settings_clone.clone();
                                    let on_change = on_change.clone();
                                    move |value: SharedString, cx| {
                                        settings.update(cx, |s, _| s.server_url = value);
                                    }
                                },
                            )
                            .default_value(SharedString::from(crate::data::client::default_api_url())),
                        )
                        .description("URL of your API backend. Configure an endpoint for market data streaming and account synchronization."),
                    ),
            )
            .group(self.save_section("network", settings.clone(), on_change.clone()))
    }

    fn build_agent_page(&self, settings: Entity<AppSettings>) -> SettingPage {
        let url_settings = settings.clone();
        let key_settings = settings.clone();
        let save_settings = settings.clone();
        SettingPage::new("AI Agent")
            .description("Configure the OpenAI-compatible provider. These values remain local and are never saved to the database.")
            .group(SettingGroup::new().title("Provider")
                .item(SettingItem::new("Base URL", SettingField::input(
                    { let s=url_settings.clone(); move |cx| s.read(cx).agent_base_url.clone() },
                    { let s=url_settings.clone(); move |v, cx| s.update(cx, |x,_| x.agent_base_url=v) },
                )))
                .item(SettingItem::new("API Key", SettingField::input(
                    { let s=key_settings.clone(); move |cx| s.read(cx).agent_api_key.clone() },
                    { let s=key_settings.clone(); move |v, cx| s.update(cx, |x,_| x.agent_api_key=v) },
                )).description("Saved only on this machine; never sent to the FinTer database."))
                .item(SettingItem::render(move |_, _, cx| {
                    let s=save_settings.clone();
                    gpui_component::button::Button::new("save-agent-settings").label("Save Agent Settings")
                        .on_click(move |_, _, cx| {
                            let values=s.read(cx);
                            let config = crate::data::agent::LocalConfig {
                                base_url: values.agent_base_url.to_string(),
                                api_key: values.agent_api_key.to_string(),
                            };
                            if let Err(error) = crate::data::agent::save_provider_config(&config) {
                                tracing::error!(?error, "Failed to save local Agent settings");
                            }
                            // SAFETY: GPUI mutates this process environment on its UI thread.
                            unsafe {
                                std::env::set_var("FINTER_AI_BASE_URL", values.agent_base_url.as_ref());
                                std::env::set_var("FINTER_AI_API_KEY", values.agent_api_key.as_ref());
                            }
                            cx.refresh_windows();
                        })
                })))
    }

    fn build_api_management_page(&self, _settings: Entity<AppSettings>) -> SettingPage {
        let view = self.api_management.clone();
        SettingPage::new("APIs Management")
            .description("Configure and manage local exchange API keys. Stored only on this machine with 0600 permissions.")
            .group(
                SettingGroup::new()
                    .item(SettingItem::render(move |_, _, _| {
                        view.clone()
                    })),
            )
    }

    fn build_appearance_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        let settings_clone = settings.clone();
        let settings_clone2 = settings.clone();

        SettingPage::new("Theme & Appearance")
            .description("Customize the look and feel of your trading environment")
            .default_open(true)
            .group(
                SettingGroup::new()
                    .title("Theme")
                    .item(
                        SettingItem::new(
                            "Theme",
                            SettingField::dropdown(
                                theme::get_theme_names()
                                    .iter()
                                    .map(|name| {
                                        let display_name =
                                            name.chars().next().unwrap().to_uppercase().to_string()
                                                + &name[1..];
                                        (
                                            SharedString::from(*name),
                                            SharedString::from(display_name),
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).theme.clone()
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value: SharedString, cx| {
                                        let dark_mode = settings.read(cx).dark_mode;
                                        settings.update(cx, |s, _| s.theme = value.clone());

                                    }
                                },
                            )
                            .default_value(SharedString::from("corporate")),
                        )
                        .description("Select your preferred color theme"),
                    )
                    .item(
                        SettingItem::new(
                            "Dark Mode",
                            SettingField::switch(
                                {
                                    let settings = settings_clone.clone();
                                    move |cx| settings.read(cx).dark_mode
                                },
                                {
                                    let settings = settings_clone.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        let theme_name = settings.read(cx).theme.clone();
                                        settings.update(cx, |s, _| s.dark_mode = value);

                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Toggle between dark and light mode"),
                    )
                    .item(
                        SettingItem::new(
                            "Enable Animations",
                            SettingField::switch(
                                {
                                    let settings = settings_clone2.clone();
                                    move |cx| settings.read(cx).enable_animations
                                },
                                {
                                    let settings = settings_clone2.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.enable_animations = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Enable smooth UI animations"),
                    ),
            )
            .group(self.save_section("appearance", settings.clone(), on_change.clone()))
    }

    fn build_trading_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        let settings_clone = settings.clone();

        SettingPage::new("Trading & Exchange")
            .description("Configure your default trading preferences")
            .group(
                SettingGroup::new()
                    .title("Exchange Settings")
                    .item(
                        SettingItem::new(
                            "Default Exchange",
                            SettingField::dropdown(
                                vec![
                                    ("binance".into(), "Binance".into()),
                                    ("coinbase".into(), "Coinbase".into()),
                                    ("kraken".into(), "Kraken".into()),
                                    ("ftx".into(), "FTX".into()),
                                    ("bybit".into(), "Bybit".into()),
                                    ("okx".into(), "OKX".into()),
                                ],
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).default_exchange.clone()
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value: SharedString, cx| {
                                        settings.update(cx, |s, _| s.default_exchange = value);
                                    }
                                },
                            )
                            .default_value(SharedString::from("binance")),
                        )
                        .description("Select your primary trading exchange"),
                    )
                    .item(
                        SettingItem::new(
                            "Default Trading Pair",
                            SettingField::input(
                                {
                                    let settings = settings_clone.clone();
                                    move |cx| settings.read(cx).default_trading_pair.clone()
                                },
                                {
                                    let settings = settings_clone.clone();
                                    let on_change = on_change.clone();
                                    move |value: SharedString, cx| {
                                        settings.update(cx, |s, _| s.default_trading_pair = value);
                                    }
                                },
                            )
                            .default_value(SharedString::from("BTC/USDT")),
                        )
                        .description("Default symbol to load on startup"),
                    ),
            )
            .group(self.save_section("trading", settings.clone(), on_change.clone()))
    }

    fn build_alerts_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        let settings_clone2 = settings.clone();
        let settings_clone3 = settings.clone();

        SettingPage::new("Alerts & Notifications")
            .description("Manage your trading alerts and notification preferences")
            .group(
                SettingGroup::new()
                    .title("Price Alerts")
                    .item(
                        SettingItem::new(
                            "Enable Price Alerts",
                            SettingField::switch(
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).enable_price_alerts
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.enable_price_alerts = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Get notified when price targets are hit"),
                    )
                    .item(
                        SettingItem::new(
                            "Sound Alerts",
                            SettingField::switch(
                                {
                                    let settings = settings_clone2.clone();
                                    move |cx| settings.read(cx).enable_sound_alerts
                                },
                                {
                                    let settings = settings_clone2.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.enable_sound_alerts = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Play sound when alerts trigger"),
                    ),
            )
            .group(
                SettingGroup::new().title("System Notifications").item(
                    SettingItem::new(
                        "Desktop Notifications",
                        SettingField::switch(
                            {
                                let settings = settings_clone3.clone();
                                move |cx| settings.read(cx).enable_desktop_notifications
                            },
                            {
                                let settings = settings_clone3.clone();
                                let on_change = on_change.clone();
                                move |value, cx| {
                                    settings
                                        .update(cx, |s, _| s.enable_desktop_notifications = value);
                                }
                            },
                        )
                        .default_value(true),
                    )
                    .description("Show system notifications for alerts"),
                ),
            )
            .group(self.save_section("alerts", settings.clone(), on_change.clone()))
    }

    fn build_performance_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        let settings_clone = settings.clone();

        SettingPage::new("Performance")
            .description("Optimize performance for your system capabilities")
            .group(
                SettingGroup::new()
                    .title("Rendering")
                    .item(
                        SettingItem::new(
                            "Max FPS",
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 30.0,
                                    max: 120.0,
                                    step: 10.0,
                                },
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).max_fps
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.max_fps = value);
                                    }
                                },
                            )
                            .default_value(60.0),
                        )
                        .description("Maximum frames per second (30-120)"),
                    )
                    .item(
                        SettingItem::new(
                            "Data Update Frequency (ms)",
                            SettingField::number_input(
                                NumberFieldOptions {
                                    min: 50.0,
                                    max: 1000.0,
                                    step: 50.0,
                                },
                                {
                                    let settings = settings_clone.clone();
                                    move |cx| settings.read(cx).data_update_frequency
                                },
                                {
                                    let settings = settings_clone.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.data_update_frequency = value);
                                    }
                                },
                            )
                            .default_value(100.0),
                        )
                        .description("How often to refresh market data (50-1000ms)"),
                    ),
            )
            .group(self.save_section("performance", settings.clone(), on_change.clone()))
    }

    fn build_layout_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        SettingPage::new("Layout Management")
            .description("Save and restore your panel layouts")
            .group(
                SettingGroup::new()
                    .title("Layout & Formatting")
                    .item(
                        SettingItem::new(
                            "Sync Dashboard Tabs",
                            SettingField::switch(
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).sync_dashboard_tabs
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.sync_dashboard_tabs = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Automatically sync layout changes across all open tabs"),
                    )
                    .item(
                        SettingItem::new(
                            "Default Timezone",
                            SettingField::combobox(
                                finter_timezones(),
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).default_timezone.clone()
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.default_timezone = value);
                                    }
                                },
                            )
                            .default_value("UTC"),
                        )
                        .description("Used for timestamp displays on charts"),
                    )
                    .item(
                        SettingItem::new(
                            "Auto-Save Dashboard",
                            SettingField::switch(
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).auto_save_dashboard
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.auto_save_dashboard = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Automatically save dashboard changes on exit"),
                    ),
            )
            .group(self.save_section("layout", settings.clone(), on_change.clone()))
    }

    fn build_tabs_page(&self, settings: Entity<AppSettings>, on_change: OnChangeFn) -> SettingPage {
        let mode_settings = settings.clone();
        let dashboard_settings = settings.clone();
        let dashboards = self.dashboards.clone();

        SettingPage::new("Tabs")
            .description("Choose which dashboard tabs open when FinTer starts. These preferences are stored locally on this machine and are never synced to your account.")
            .group(
                SettingGroup::new()
                    .title("Startup behavior")
                    .item(
                        SettingItem::new(
                            "Tabs on startup",
                            SettingField::dropdown(
                                vec![
                                    ("restore".into(), "Restore last session".into()),
                                    ("all".into(), "Open all dashboards".into()),
                                    ("empty".into(), "Start as empty".into()),
                                    ("selected".into(), "Open selected dashboards".into()),
                                ],
                                {
                                    let settings = mode_settings.clone();
                                    move |cx| settings.read(cx).startup_tabs_mode.clone()
                                },
                                {
                                    let settings = mode_settings.clone();
                                    move |value: SharedString, cx| {
                                        settings.update(cx, |saved, _| {
                                            saved.startup_tabs_mode = value;
                                        });
                                        cx.refresh_windows();
                                    }
                                },
                            )
                            .default_value(SharedString::from("restore")),
                        )
                        .description("Restore reopens the tabs you had open last time. Open all shows every dashboard. Start as empty opens no tabs. Selected uses the dashboards chosen below."),
                    )
                    .item(
                        SettingItem::new(
                            "Dashboards to open",
                            SettingField::render(move |options, _, cx| {
                                dashboard_multi_select(
                                    dashboard_settings.clone(),
                                    dashboards.clone(),
                                    options,
                                    cx,
                                )
                            }),
                        )
                        .description("Choose multiple dashboards. Used when startup behavior is set to \"Open selected dashboards\"."),
                    ),
            )
            .group(self.save_section("tabs", settings.clone(), on_change.clone()))
    }

    fn build_updates_page(
        &self,
        settings: Entity<AppSettings>,
        on_change: OnChangeFn,
    ) -> SettingPage {
        SettingPage::new("Updates")
            .description("Configure automatic updates and version information")
            .group(
                SettingGroup::new()
                    .title(format!(
                        "Current Version: v{}",
                        crate::data::version::APP_VERSION
                    ))
                    .item(
                        SettingItem::new(
                            "Update Channel",
                            SettingField::dropdown(
                                vec![
                                    (SharedString::from("stable"), SharedString::from("Stable")),
                                    (SharedString::from("beta"), SharedString::from("Beta")),
                                    (SharedString::from("alpha"), SharedString::from("Alpha")),
                                ],
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).update_channel.clone()
                                },
                                {
                                    let settings = settings.clone();
                                    move |value: SharedString, cx| {
                                        settings.update(cx, |s, _| s.update_channel = value);
                                    }
                                },
                            )
                            .default_value(SharedString::from(
                                crate::data::version::selected_channel().as_str(),
                            )),
                        )
                        .description("Choose which release channel this installation follows. Stable receives tagged releases; Beta and Alpha receive pre-release builds."),
                    )
                    .item(
                        SettingItem::new(
                            "Auto-Check for Updates",
                            SettingField::switch(
                                {
                                    let settings = settings.clone();
                                    move |cx| settings.read(cx).auto_check_updates
                                },
                                {
                                    let settings = settings.clone();
                                    let on_change = on_change.clone();
                                    move |value, cx| {
                                        settings.update(cx, |s, _| s.auto_check_updates = value);
                                    }
                                },
                            )
                            .default_value(true),
                        )
                        .description("Automatically check for new versions every 24 hours"),
                    ),
            )
            .group(self.save_section("updates", settings.clone(), on_change.clone()))
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings_widget = self.build_settings(cx);

        div()
            .size_full()
            .bg(cx.theme().background)
            .child(settings_widget)
    }
}
