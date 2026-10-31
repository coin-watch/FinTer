use crate::components::pages::api_management_view::ApiManagementView;
use crate::data::models::SettingsData;
use crate::data::theme;
use gpui::*;
use gpui_component::{
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
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
    pub sync_dashboard_tabs: bool,
    pub default_timezone: gpui::SharedString,
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
            server_url: crate::data::client::DEFAULT_FINTER_API_URL.into(),
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
            sync_dashboard_tabs: true,
            default_timezone: "UTC".into(),
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
    pub fn from_settings_data(data: &SettingsData) -> Self {
        Self {
            server_url: data
                .server_url
                .clone()
                .unwrap_or_else(|| crate::data::client::DEFAULT_FINTER_API_URL.to_string())
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
            server_url: Some(self.server_url.to_string()),
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
            ..Default::default()
        }
    }
}

pub struct SettingsPage {
    settings: Entity<AppSettings>,
    committed: Entity<AppSettings>,
    on_change: OnChangeFn,
    api_management: Entity<ApiManagementView>,
}

impl SettingsPage {
    pub fn new(cx: &mut App) -> Self {
        Self {
            settings: cx.new(|_| AppSettings::default()),
            committed: cx.new(|_| AppSettings::default()),
            on_change: None,
            api_management: cx.new(|cx| ApiManagementView::new(cx)),
        }
    }

    pub fn new_with_data(data: &SettingsData, cx: &mut App) -> Self {
        Self {
            settings: cx.new(|_| AppSettings::from_settings_data(data)),
            committed: cx.new(|_| AppSettings::from_settings_data(data)),
            on_change: None,
            api_management: cx.new(|cx| ApiManagementView::new(cx)),
        }
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
                            "updates" => { saved.auto_check_updates = values.auto_check_updates.clone(); },
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
                            .default_value(SharedString::from(crate::data::client::DEFAULT_FINTER_API_URL)),
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
