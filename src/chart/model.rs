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
}

impl ChartLayout {
    pub fn new(
        exchange: impl Into<String>,
        symbol: impl Into<String>,
        timeframe: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: 1,
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
