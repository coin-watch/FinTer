use crate::chart::model::{ChartLayout, IndicatorInstance, IndicatorKind};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn default_chart_dir() -> PathBuf {
    dirs::config_dir()
        .map(|mut p| {
            p.push("finter");
            p
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn layout_path(root: &Path, exchange: &str, symbol: &str, timeframe: &str) -> PathBuf {
    root.join("charts")
        .join(safe_component(exchange))
        .join(safe_component(symbol))
        .join(format!("{}.json", safe_component(timeframe)))
}

pub fn save_layout(root: &Path, layout: &ChartLayout) -> io::Result<()> {
    let path = layout_path(root, &layout.exchange, &layout.symbol, &layout.timeframe);
    let parent = path.parent().expect("layout path has parent");
    fs::create_dir_all(parent)?;
    let encoded = serde_json::to_vec_pretty(layout).map_err(io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, path)
}

/// Load an exact saved layout, or a new v1 layout when absent, corrupt, or unsupported.
pub fn load_layout(root: &Path, exchange: &str, symbol: &str, timeframe: &str) -> ChartLayout {
    let fallback = ChartLayout::new(exchange, symbol, timeframe);
    let path = layout_path(root, exchange, symbol, timeframe);
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChartLayout>(&bytes).ok())
        .filter(|layout| layout.schema_version == 1)
        .unwrap_or(fallback)
}

pub fn preset_path(root: &Path, preset_name: &str) -> PathBuf {
    root.join("charts")
        .join("presets")
        .join(format!("{}.json", safe_component(preset_name)))
}

pub fn save_preset(root: &Path, preset_name: &str, layout: &ChartLayout) -> io::Result<()> {
    let path = preset_path(root, preset_name);
    let parent = path.parent().expect("preset path has parent");
    fs::create_dir_all(parent)?;
    let encoded = serde_json::to_vec_pretty(layout).map_err(io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, encoded)?;
    fs::rename(temporary, path)
}

pub fn load_preset(root: &Path, preset_name: &str) -> Option<ChartLayout> {
    let path = preset_path(root, preset_name);
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChartLayout>(&bytes).ok())
        .filter(|layout| layout.schema_version == 1)
}

pub fn list_presets(root: &Path) -> Vec<String> {
    let dir = root.join("charts").join("presets");
    let mut names = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names
}

/// Returns standard built-in layout presets.
pub fn built_in_presets(exchange: &str, symbol: &str, timeframe: &str) -> Vec<(&'static str, ChartLayout)> {
    let mut default_layout = ChartLayout::new(exchange, symbol, timeframe);
    default_layout.indicators = vec![
        IndicatorInstance::new("ma-20", IndicatorKind::Ma),
        IndicatorInstance::new("vol", IndicatorKind::Vol),
    ];

    let mut trend_layout = ChartLayout::new(exchange, symbol, timeframe);
    let mut ema20 = IndicatorInstance::new("ema-20", IndicatorKind::Ema);
    ema20.params.insert("period".into(), 20.0);
    let mut ema50 = IndicatorInstance::new("ema-50", IndicatorKind::Ema);
    ema50.params.insert("period".into(), 50.0);
    let macd = IndicatorInstance::new("macd-std", IndicatorKind::Macd);
    trend_layout.indicators = vec![ema20, ema50, macd];

    let mut scalping_layout = ChartLayout::new(exchange, symbol, timeframe);
    let mut ema9 = IndicatorInstance::new("ema-9", IndicatorKind::Ema);
    ema9.params.insert("period".into(), 9.0);
    let mut ema21 = IndicatorInstance::new("ema-21", IndicatorKind::Ema);
    ema21.params.insert("period".into(), 21.0);
    let rsi = IndicatorInstance::new("rsi-14", IndicatorKind::Rsi);
    scalping_layout.indicators = vec![ema9, ema21, rsi];

    let mut vol_layout = ChartLayout::new(exchange, symbol, timeframe);
    let boll = IndicatorInstance::new("boll-std", IndicatorKind::Boll);
    vol_layout.indicators = vec![boll];

    vec![
        ("Default", default_layout),
        ("Trend Following", trend_layout),
        ("Scalping", scalping_layout),
        ("Volatility (Bands)", vol_layout),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "cwn-chart-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn saves_and_loads_per_symbol_timeframe() {
        let root = root();
        let layout = ChartLayout::new("binance", "BTC/USDT", "1h");
        save_layout(&root, &layout).unwrap();
        assert_eq!(load_layout(&root, "binance", "BTC/USDT", "1h"), layout);
        assert_ne!(
            layout_path(&root, "binance", "BTC/USDT", "1h"),
            layout_path(&root, "binance", "BTC/USDT", "5m")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_file_falls_back_to_new_v1_layout() {
        let root = root();
        let path = layout_path(&root, "binance", "BTC/USDT", "1h");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"not json").unwrap();
        assert_eq!(
            load_layout(&root, "binance", "BTC/USDT", "1h"),
            ChartLayout::new("binance", "BTC/USDT", "1h")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preset_save_load_roundtrips() {
        let root = root();
        let layout = ChartLayout::new("binance", "ETH/USDT", "4h");
        save_preset(&root, "my_strategy", &layout).unwrap();
        let loaded = load_preset(&root, "my_strategy");
        assert_eq!(loaded, Some(layout));
        let presets = list_presets(&root);
        assert_eq!(presets, vec!["my_strategy".to_string()]);
        fs::remove_dir_all(root).unwrap();
    }
}
