use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            x: 240.0,
            y: 48.0,
            width: 480.0,
            height: 420.0,
        }
    }
}

/// Footprint cell rendering mode.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FootprintMode {
    /// Bid volume on the left, ask volume on the right.
    #[default]
    BidAsk,
    /// Signed delta only.
    Delta,
    /// Total volume only.
    Volume,
}

impl FootprintMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::BidAsk => "Bid × Ask",
            Self::Delta => "Delta",
            Self::Volume => "Volume",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_imbalance_ratio() -> f64 {
    3.0
}

fn default_min_row_height() -> f32 {
    12.0
}

fn default_ticks_per_row() -> u32 {
    10
}

/// Footprint display mode and analytics settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FootprintSettings {
    #[serde(default)]
    pub mode: FootprintMode,
    #[serde(default = "default_true")]
    pub show_poc: bool,
    #[serde(default = "default_true")]
    pub show_value_area: bool,
    #[serde(default = "default_true")]
    pub show_imbalance: bool,
    #[serde(default = "default_true")]
    pub show_summary: bool,
    #[serde(default = "default_imbalance_ratio")]
    pub imbalance_ratio: f64,
    /// Minimum pixel height of a footprint row before rows are merged.
    #[serde(default = "default_min_row_height")]
    pub min_row_height: f32,
    /// Price rows per footprint bucket when the instrument tick size is known.
    #[serde(default = "default_ticks_per_row")]
    pub ticks_per_row: u32,
}

impl Default for FootprintSettings {
    fn default() -> Self {
        Self {
            mode: FootprintMode::BidAsk,
            show_poc: true,
            show_value_area: true,
            show_imbalance: true,
            show_summary: true,
            imbalance_ratio: default_imbalance_ratio(),
            min_row_height: default_min_row_height(),
            ticks_per_row: default_ticks_per_row(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChartLayout {
    pub schema_version: u32,
    pub exchange: String,
    pub symbol: String,
    pub timeframe: String,
    pub candle_type: String,
    pub show_grid: bool,
    pub timezone: String,
    pub indicators: Vec<IndicatorInstance>,
    pub drawings: Vec<crate::chart::drawings::DrawingInstance>,
    pub pane_ratios: BTreeMap<String, f64>,
    #[serde(default)]
    pub library_geometry: Option<WindowGeometry>,
    /// Price scale mode: `linear`, `log`, `percent`, or `inverted`.
    #[serde(default = "default_price_scale_mode")]
    pub price_scale_mode: String,
    /// User-locked price range; `None` means auto-fit.
    #[serde(default)]
    pub manual_price_range: Option<(f64, f64)>,
    /// Anchor for percentage labels when `price_scale_mode == "percent"`.
    #[serde(default)]
    pub percent_anchor: Option<f64>,
    /// Render the chart as a bid × ask footprint.
    #[serde(default)]
    pub show_footprint: bool,
    /// Footprint display mode, analytics and row density.
    #[serde(default)]
    pub footprint: FootprintSettings,
}

fn default_price_scale_mode() -> String {
    "linear".to_string()
}

impl ChartLayout {
    pub fn new(
        exchange: impl Into<String>,
        symbol: impl Into<String>,
        timeframe: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: 2,
            exchange: exchange.into(),
            symbol: symbol.into(),
            timeframe: timeframe.into(),
            candle_type: "solid".into(),
            show_grid: false,
            timezone: "UTC".into(),
            indicators: vec![],
            drawings: vec![],
            pane_ratios: BTreeMap::new(),
            library_geometry: None,
            price_scale_mode: default_price_scale_mode(),
            manual_price_range: None,
            percent_anchor: None,
            show_footprint: false,
            footprint: FootprintSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorKind {
    Vol,
    Ma,
    Ema,
    Boll,
    Sar,
    Bbi,
    Avp,
    Macd,
    Kdj,
    Rsi,
    Wr,
    Cci,
    Roc,
    Mtm,
    Trx,
    Bias,
    Obv,
    Pvt,
    Psy,
    Sma,
    Other(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorCategory {
    Trend,
    Momentum,
    Volatility,
    Volume,
}

impl IndicatorCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Trend => "Trend",
            Self::Momentum => "Momentum",
            Self::Volatility => "Volatility",
            Self::Volume => "Volume",
        }
    }
}

impl IndicatorKind {
    pub fn label(&self) -> &str {
        match self {
            Self::Vol => "VOL",
            Self::Ma => "MA",
            Self::Ema => "EMA",
            Self::Boll => "BOLL",
            Self::Sar => "SAR",
            Self::Bbi => "BBI",
            Self::Avp => "AVP",
            Self::Macd => "MACD",
            Self::Kdj => "KDJ",
            Self::Rsi => "RSI",
            Self::Wr => "WR",
            Self::Cci => "CCI",
            Self::Roc => "ROC",
            Self::Mtm => "MTM",
            Self::Trx => "TRIX",
            Self::Bias => "BIAS",
            Self::Obv => "OBV",
            Self::Pvt => "PVT",
            Self::Psy => "PSY",
            Self::Sma => "SMA",
            Self::Other(name) => name.as_str(),
        }
    }

    pub fn category(&self) -> IndicatorCategory {
        match self {
            Self::Ma | Self::Ema | Self::Sma | Self::Sar | Self::Bbi | Self::Avp => {
                IndicatorCategory::Trend
            }
            Self::Boll => IndicatorCategory::Volatility,
            Self::Vol | Self::Obv | Self::Pvt => IndicatorCategory::Volume,
            Self::Macd
            | Self::Kdj
            | Self::Rsi
            | Self::Wr
            | Self::Cci
            | Self::Roc
            | Self::Mtm
            | Self::Trx
            | Self::Bias
            | Self::Psy => IndicatorCategory::Momentum,
            Self::Other(_) => IndicatorCategory::Trend,
        }
    }

    pub fn default_pane(&self) -> &'static str {
        match self {
            Self::Ma | Self::Ema | Self::Sma | Self::Boll | Self::Sar | Self::Bbi | Self::Avp => {
                "main"
            }
            _ => "subpane",
        }
    }

    pub fn default_params(&self) -> BTreeMap<String, f64> {
        let mut map = BTreeMap::new();
        match self {
            Self::Ma | Self::Ema | Self::Sma => {
                map.insert("period".into(), 20.0);
            }
            Self::Boll => {
                map.insert("period".into(), 20.0);
                map.insert("deviations".into(), 2.0);
            }
            Self::Sar => {
                map.insert("step".into(), 0.02);
                map.insert("max_step".into(), 0.2);
            }
            Self::Macd => {
                map.insert("short".into(), 12.0);
                map.insert("long".into(), 26.0);
                map.insert("signal".into(), 9.0);
            }
            Self::Kdj => {
                map.insert("period".into(), 9.0);
                map.insert("k_period".into(), 3.0);
                map.insert("d_period".into(), 3.0);
            }
            Self::Rsi => {
                map.insert("period".into(), 14.0);
            }
            Self::Wr => {
                map.insert("period".into(), 14.0);
            }
            Self::Cci => {
                map.insert("period".into(), 20.0);
            }
            Self::Roc => {
                map.insert("period".into(), 12.0);
            }
            Self::Mtm => {
                map.insert("period".into(), 12.0);
            }
            Self::Trx => {
                map.insert("period".into(), 12.0);
                map.insert("signal".into(), 9.0);
            }
            Self::Bias => {
                map.insert("period".into(), 6.0);
            }
            Self::Psy => {
                map.insert("period".into(), 12.0);
            }
            Self::Vol | Self::Bbi | Self::Avp | Self::Obv | Self::Pvt | Self::Other(_) => {}
        }
        map
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Ma,
            Self::Ema,
            Self::Sma,
            Self::Boll,
            Self::Sar,
            Self::Bbi,
            Self::Avp,
            Self::Macd,
            Self::Kdj,
            Self::Rsi,
            Self::Wr,
            Self::Cci,
            Self::Roc,
            Self::Mtm,
            Self::Trx,
            Self::Bias,
            Self::Obv,
            Self::Pvt,
            Self::Psy,
            Self::Vol,
        ]
    }

    pub fn all_by_category(category: IndicatorCategory) -> Vec<Self> {
        Self::all()
            .into_iter()
            .filter(|k| k.category() == category)
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IndicatorStyle {
    pub color: String,
    pub width: f64,
}

impl Default for IndicatorStyle {
    fn default() -> Self {
        Self {
            color: "#C4E855".into(),
            width: 1.5,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IndicatorInstance {
    pub id: String,
    pub kind: IndicatorKind,
    pub pane: String,
    pub params: BTreeMap<String, f64>,
    pub style: IndicatorStyle,
    pub visible: bool,
}

impl IndicatorInstance {
    pub fn new(id: impl Into<String>, kind: IndicatorKind) -> Self {
        let pane = kind.default_pane().to_string();
        let params = kind.default_params();
        Self {
            id: id.into(),
            kind,
            pane,
            params,
            style: IndicatorStyle::default(),
            visible: true,
        }
    }

    pub fn display_label(&self) -> String {
        let label = self.kind.label();
        match self.kind {
            IndicatorKind::Ma | IndicatorKind::Ema | IndicatorKind::Sma | IndicatorKind::Rsi => {
                if let Some(period) = self.params.get("period") {
                    format!("{} {}", label, *period as i64)
                } else {
                    label.to_string()
                }
            }
            IndicatorKind::Macd => {
                let s = self.params.get("short").copied().unwrap_or(12.0) as i64;
                let l = self.params.get("long").copied().unwrap_or(26.0) as i64;
                let sig = self.params.get("signal").copied().unwrap_or(9.0) as i64;
                format!("MACD ({}, {}, {})", s, l, sig)
            }
            IndicatorKind::Boll => {
                let p = self.params.get("period").copied().unwrap_or(20.0) as i64;
                let dev = self.params.get("deviations").copied().unwrap_or(2.0);
                format!("BOLL ({}, {})", p, dev)
            }
            IndicatorKind::Kdj => {
                let p = self.params.get("period").copied().unwrap_or(9.0) as i64;
                let k = self.params.get("k_period").copied().unwrap_or(3.0) as i64;
                let d = self.params.get("d_period").copied().unwrap_or(3.0) as i64;
                format!("KDJ ({}, {}, {})", p, k, d)
            }
            _ => label.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_v1_round_trips_duplicate_indicator_instances_and_drawings() {
        let mut l = ChartLayout::new("binance", "BTC/USDT", "1h");
        for id in ["ema20", "ema50"] {
            l.indicators.push(IndicatorInstance {
                id: id.into(),
                kind: IndicatorKind::Ema,
                pane: "main".into(),
                params: BTreeMap::from([("period".into(), 20.0)]),
                style: Default::default(),
                visible: true,
            });
        }
        l.library_geometry = Some(WindowGeometry {
            x: 100.0,
            y: 150.0,
            width: 500.0,
            height: 400.0,
        });
        let b = serde_json::to_vec(&l).unwrap();
        assert_eq!(serde_json::from_slice::<ChartLayout>(&b).unwrap(), l);
    }

    #[test]
    fn layout_with_legacy_orderflow_fields_still_loads() {
        // Layouts written while the chart had order-flow overlays must keep
        // loading after the feature was removed: serde ignores unknown fields.
        let layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        let mut value = serde_json::to_value(&layout).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("show_delta".into(), serde_json::json!(true));
        object.insert("show_profile".into(), serde_json::json!(true));
        object.insert("show_footprint".into(), serde_json::json!(true));
        object.insert("show_liquidity".into(), serde_json::json!(true));
        object.insert("show_l1".into(), serde_json::json!(true));
        object.insert("show_absorption".into(), serde_json::json!(true));
        object.insert("show_divergence".into(), serde_json::json!(true));
        object.insert("show_imbalance_stacks".into(), serde_json::json!(true));
        object.insert("show_cvd_regime".into(), serde_json::json!(true));
        object.insert(
            "footprint".into(),
            serde_json::json!({"mode": "delta", "row_multiplier": 4}),
        );
        let parsed: ChartLayout = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.schema_version, 2);
        assert_eq!(parsed.price_scale_mode, "linear");
    }

    #[test]
    fn layout_without_footprint_keys_uses_defaults() {
        let layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        let mut value = serde_json::to_value(&layout).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("show_footprint");
        object.remove("footprint");
        let parsed: ChartLayout = serde_json::from_value(value).unwrap();
        assert!(!parsed.show_footprint);
        assert_eq!(parsed.footprint, FootprintSettings::default());
        assert_eq!(parsed.footprint.mode, FootprintMode::BidAsk);
        assert_eq!(parsed.footprint.ticks_per_row, 10);
    }

    #[test]
    fn footprint_settings_round_trip() {
        let mut layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        layout.show_footprint = true;
        layout.footprint = FootprintSettings {
            mode: FootprintMode::Delta,
            show_poc: false,
            show_value_area: false,
            show_imbalance: true,
            show_summary: false,
            imbalance_ratio: 4.5,
            min_row_height: 8.0,
            ticks_per_row: 25,
        };
        let encoded = serde_json::to_vec(&layout).unwrap();
        let parsed: ChartLayout = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(parsed, layout);
    }

    #[test]
    fn v1_layout_without_scale_fields_uses_defaults() {
        let layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        let mut value = serde_json::to_value(&layout).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("schema_version".into(), serde_json::json!(1));
        object.remove("price_scale_mode");
        object.remove("manual_price_range");
        object.remove("percent_anchor");
        let parsed: ChartLayout = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.price_scale_mode, "linear");
        assert_eq!(parsed.manual_price_range, None);
        assert_eq!(parsed.percent_anchor, None);
    }

    #[test]
    fn scale_fields_round_trip() {
        let mut layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        layout.price_scale_mode = "percent".into();
        layout.manual_price_range = Some((100.0, 200.0));
        layout.percent_anchor = Some(150.0);
        let encoded = serde_json::to_vec(&layout).unwrap();
        let parsed: ChartLayout = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(parsed.price_scale_mode, "percent");
        assert_eq!(parsed.manual_price_range, Some((100.0, 200.0)));
        assert_eq!(parsed.percent_anchor, Some(150.0));
    }

    #[test]
    fn display_label_formats_parameters() {
        let inst = IndicatorInstance::new("test-ema", IndicatorKind::Ema);
        assert_eq!(inst.display_label(), "EMA 20");

        let mut macd = IndicatorInstance::new("test-macd", IndicatorKind::Macd);
        macd.params.insert("short".into(), 10.0);
        macd.params.insert("long".into(), 20.0);
        macd.params.insert("signal".into(), 5.0);
        assert_eq!(macd.display_label(), "MACD (10, 20, 5)");
    }
}
