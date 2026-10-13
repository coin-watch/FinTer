use crate::chart::model::{IndicatorInstance, IndicatorKind};
use crate::data::models::OhlcvCandle;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Stable-within-a-build cache identity for an indicator result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IndicatorCacheKey(pub u64);

impl IndicatorCacheKey {
    pub fn new(feed_version: u64, indicator: &IndicatorInstance) -> Self {
        let mut hasher = DefaultHasher::new();
        feed_version.hash(&mut hasher);
        indicator.kind.hash(&mut hasher);
        for (name, value) in &indicator.params {
            name.hash(&mut hasher);
            value.to_bits().hash(&mut hasher);
        }
        Self(hasher.finish())
    }
}

/// A computed series line ready for rendering or plotting.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedLineSpec {
    pub name: String,
    pub series: Vec<(i64, f64)>,
    pub color_offset: usize,
    pub is_histogram: bool,
    pub is_main_pane: bool,
    pub base_color: Option<String>,
}

/// Validates user parameters for an indicator.
/// Returns Ok(()) if valid, or Err(String) with an inline actionable error message.
pub fn validate_params(kind: &IndicatorKind, params: &BTreeMap<String, f64>) -> Result<(), String> {
    match kind {
        IndicatorKind::Ma | IndicatorKind::Ema | IndicatorKind::Sma => {
            let period = params.get("period").copied().unwrap_or(20.0);
            if period < 1.0 {
                return Err("Period must be at least 1".into());
            }
            if period > 5000.0 {
                return Err("Period cannot exceed 5000".into());
            }
        }
        IndicatorKind::Boll => {
            let period = params.get("period").copied().unwrap_or(20.0);
            let dev = params.get("deviations").copied().unwrap_or(2.0);
            if period < 2.0 {
                return Err("Period must be at least 2 for Bollinger Bands".into());
            }
            if dev <= 0.0 || dev > 10.0 {
                return Err("Standard deviations must be between 0.1 and 10.0".into());
            }
        }
        IndicatorKind::Macd => {
            let short = params.get("short").copied().unwrap_or(12.0);
            let long = params.get("long").copied().unwrap_or(26.0);
            let signal = params.get("signal").copied().unwrap_or(9.0);
            if short < 1.0 {
                return Err("Fast period must be at least 1".into());
            }
            if long <= short {
                return Err("Slow period must be greater than fast period".into());
            }
            if signal < 1.0 {
                return Err("Signal period must be at least 1".into());
            }
        }
        IndicatorKind::Kdj => {
            let period = params.get("period").copied().unwrap_or(9.0);
            let k_p = params.get("k_period").copied().unwrap_or(3.0);
            let d_p = params.get("d_period").copied().unwrap_or(3.0);
            if period < 1.0 || k_p < 1.0 || d_p < 1.0 {
                return Err("Periods must be at least 1".into());
            }
        }
        IndicatorKind::Rsi | IndicatorKind::Wr | IndicatorKind::Cci | IndicatorKind::Roc
        | IndicatorKind::Mtm | IndicatorKind::Bias | IndicatorKind::Psy => {
            let period = params.get("period").copied().unwrap_or(14.0);
            if period < 1.0 {
                return Err("Period must be at least 1".into());
            }
        }
        IndicatorKind::Sar => {
            let step = params.get("step").copied().unwrap_or(0.02);
            let max_step = params.get("max_step").copied().unwrap_or(0.2);
            if step <= 0.0 || step > 1.0 {
                return Err("Acceleration step must be between 0.001 and 1.0".into());
            }
            if max_step < step {
                return Err("Max step must be greater than or equal to step".into());
            }
        }
        IndicatorKind::Trx => {
            let period = params.get("period").copied().unwrap_or(12.0);
            let signal = params.get("signal").copied().unwrap_or(9.0);
            if period < 1.0 || signal < 1.0 {
                return Err("Periods must be at least 1".into());
            }
        }
        IndicatorKind::Vol | IndicatorKind::Bbi | IndicatorKind::Avp | IndicatorKind::Obv
        | IndicatorKind::Pvt | IndicatorKind::Other(_) => {}
    }
    Ok(())
}

/// Computes the series lines for an indicator instance given candles.
pub fn compute_indicator(
    indicator: &IndicatorInstance,
    candles: &[OhlcvCandle],
) -> Vec<ComputedLineSpec> {
    if candles.is_empty() {
        return Vec::new();
    }

    let _is_main = indicator.pane == "main";
    let base_color = Some(indicator.style.color.clone());

    match indicator.kind {
        IndicatorKind::Ma | IndicatorKind::Sma => {
            let period = indicator.params.get("period").copied().unwrap_or(20.0) as usize;
            let pts = crate::klinechart::sma(candles, period);
            vec![ComputedLineSpec {
                name: format!("MA {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: true,
                base_color,
            }]
        }
        IndicatorKind::Ema => {
            let period = indicator.params.get("period").copied().unwrap_or(20.0) as usize;
            let pts = crate::klinechart::ema(candles, period);
            vec![ComputedLineSpec {
                name: format!("EMA {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 1,
                is_histogram: false,
                is_main_pane: true,
                base_color,
            }]
        }
        IndicatorKind::Boll => {
            let period = indicator.params.get("period").copied().unwrap_or(20.0) as usize;
            let dev = indicator.params.get("deviations").copied().unwrap_or(2.0);
            let pts = crate::klinechart::bollinger(candles, period, dev);
            let mut mid = Vec::with_capacity(pts.len());
            let mut upper = Vec::with_capacity(pts.len());
            let mut lower = Vec::with_capacity(pts.len());
            for p in pts {
                mid.push((p.timestamp, p.middle));
                upper.push((p.timestamp, p.upper));
                lower.push((p.timestamp, p.lower));
            }
            vec![
                ComputedLineSpec {
                    name: "BOLL".into(),
                    series: mid,
                    color_offset: 0,
                    is_histogram: false,
                    is_main_pane: true,
                    base_color: base_color.clone(),
                },
                ComputedLineSpec {
                    name: "UB".into(),
                    series: upper,
                    color_offset: 1,
                    is_histogram: false,
                    is_main_pane: true,
                    base_color: None,
                },
                ComputedLineSpec {
                    name: "LB".into(),
                    series: lower,
                    color_offset: 2,
                    is_histogram: false,
                    is_main_pane: true,
                    base_color: None,
                },
            ]
        }
        IndicatorKind::Macd => {
            let short = indicator.params.get("short").copied().unwrap_or(12.0) as usize;
            let long = indicator.params.get("long").copied().unwrap_or(26.0) as usize;
            let signal = indicator.params.get("signal").copied().unwrap_or(9.0) as usize;
            let pts = crate::klinechart::macd(candles, short, long, signal);
            let mut dif = Vec::with_capacity(pts.len());
            let mut dea = Vec::with_capacity(pts.len());
            let mut macd_hist = Vec::with_capacity(pts.len());
            for p in pts {
                dif.push((p.timestamp, p.dif));
                dea.push((p.timestamp, p.dea));
                macd_hist.push((p.timestamp, p.histogram));
            }
            vec![
                ComputedLineSpec {
                    name: "DIF".into(),
                    series: dif,
                    color_offset: 0,
                    is_histogram: false,
                    is_main_pane: false,
                    base_color: base_color.clone(),
                },
                ComputedLineSpec {
                    name: "DEA".into(),
                    series: dea,
                    color_offset: 1,
                    is_histogram: false,
                    is_main_pane: false,
                    base_color: None,
                },
                ComputedLineSpec {
                    name: "MACD".into(),
                    series: macd_hist,
                    color_offset: 2,
                    is_histogram: true,
                    is_main_pane: false,
                    base_color: None,
                },
            ]
        }
        IndicatorKind::Kdj => {
            let period = indicator.params.get("period").copied().unwrap_or(9.0) as usize;
            let k_p = indicator.params.get("k_period").copied().unwrap_or(3.0) as usize;
            let d_p = indicator.params.get("d_period").copied().unwrap_or(3.0) as usize;
            let pts = crate::klinechart::kdj(candles, period, k_p, d_p);
            let mut k_s = Vec::with_capacity(pts.len());
            let mut d_s = Vec::with_capacity(pts.len());
            let mut j_s = Vec::with_capacity(pts.len());
            for p in pts {
                k_s.push((p.timestamp, p.k));
                d_s.push((p.timestamp, p.d));
                j_s.push((p.timestamp, p.j));
            }
            vec![
                ComputedLineSpec {
                    name: "K".into(),
                    series: k_s,
                    color_offset: 0,
                    is_histogram: false,
                    is_main_pane: false,
                    base_color: base_color.clone(),
                },
                ComputedLineSpec {
                    name: "D".into(),
                    series: d_s,
                    color_offset: 1,
                    is_histogram: false,
                    is_main_pane: false,
                    base_color: None,
                },
                ComputedLineSpec {
                    name: "J".into(),
                    series: j_s,
                    color_offset: 2,
                    is_histogram: false,
                    is_main_pane: false,
                    base_color: None,
                },
            ]
        }
        IndicatorKind::Rsi => {
            let period = indicator.params.get("period").copied().unwrap_or(14.0) as usize;
            let pts = crate::klinechart::rsi(candles, period);
            vec![ComputedLineSpec {
                name: format!("RSI {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Sar => {
            let step = indicator.params.get("step").copied().unwrap_or(0.02);
            let max_step = indicator.params.get("max_step").copied().unwrap_or(0.2);
            let pts = crate::klinechart::sar(candles, step, max_step);
            vec![ComputedLineSpec {
                name: "SAR".into(),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: true,
                base_color,
            }]
        }
        IndicatorKind::Bbi => {
            let pts = crate::klinechart::bbi(candles);
            vec![ComputedLineSpec {
                name: "BBI".into(),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: true,
                base_color,
            }]
        }
        IndicatorKind::Avp => {
            let pts = crate::klinechart::avp(candles);
            vec![ComputedLineSpec {
                name: "AVP".into(),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: true,
                base_color,
            }]
        }
        IndicatorKind::Wr => {
            let period = indicator.params.get("period").copied().unwrap_or(14.0) as usize;
            let pts = crate::klinechart::williams_r(candles, period);
            vec![ComputedLineSpec {
                name: format!("WR {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Cci => {
            let period = indicator.params.get("period").copied().unwrap_or(20.0) as usize;
            let pts = crate::klinechart::cci(candles, period);
            vec![ComputedLineSpec {
                name: format!("CCI {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Roc => {
            let period = indicator.params.get("period").copied().unwrap_or(12.0) as usize;
            let pts = crate::klinechart::roc(candles, period);
            vec![ComputedLineSpec {
                name: format!("ROC {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Mtm => {
            let period = indicator.params.get("period").copied().unwrap_or(12.0) as usize;
            let pts = crate::klinechart::mtm(candles, period);
            vec![ComputedLineSpec {
                name: format!("MTM {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Trx => {
            let period = indicator.params.get("period").copied().unwrap_or(12.0) as usize;
            let signal = indicator.params.get("signal").copied().unwrap_or(9.0) as usize;
            let pts = crate::klinechart::trix(candles, period, signal);
            vec![ComputedLineSpec {
                name: format!("TRIX {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Bias => {
            let period = indicator.params.get("period").copied().unwrap_or(6.0) as usize;
            let pts = crate::klinechart::bias(candles, period);
            vec![ComputedLineSpec {
                name: format!("BIAS {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Obv => {
            let pts = crate::klinechart::obv(candles);
            vec![ComputedLineSpec {
                name: "OBV".into(),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Pvt => {
            let pts = crate::klinechart::pvt(candles);
            vec![ComputedLineSpec {
                name: "PVT".into(),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Psy => {
            let period = indicator.params.get("period").copied().unwrap_or(12.0) as usize;
            let pts = crate::klinechart::psy(candles, period);
            vec![ComputedLineSpec {
                name: format!("PSY {}", period),
                series: pts.into_iter().map(|p| (p.timestamp, p.value)).collect(),
                color_offset: 0,
                is_histogram: false,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Vol => {
            vec![ComputedLineSpec {
                name: "VOL".into(),
                series: candles.iter().map(|c| (c.timestamp, c.volume)).collect(),
                color_offset: 0,
                is_histogram: true,
                is_main_pane: false,
                base_color,
            }]
        }
        IndicatorKind::Other(_) => Vec::new(),
    }
}

/// In-memory cache for computed indicator series.
/// Keyed by (feed_version, indicator_kind, indicator_params).
#[derive(Default, Debug)]
pub struct IndicatorCache {
    entries: HashMap<IndicatorCacheKey, Arc<Vec<ComputedLineSpec>>>,
}

impl IndicatorCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve or compute indicator lines.
    pub fn get_or_compute(
        &mut self,
        feed_version: u64,
        indicator: &IndicatorInstance,
        candles: &[OhlcvCandle],
    ) -> Arc<Vec<ComputedLineSpec>> {
        let key = IndicatorCacheKey::new(feed_version, indicator);
        if let Some(cached) = self.entries.get(&key) {
            return Arc::clone(cached);
        }

        if self.entries.len() > 128 {
            self.entries.clear();
        }

        let computed = Arc::new(compute_indicator(indicator, candles));
        self.entries.insert(key, Arc::clone(&computed));
        computed
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::model::{IndicatorKind, IndicatorStyle};
    use std::collections::BTreeMap;

    fn indicator(id: &str, period: f64) -> IndicatorInstance {
        IndicatorInstance {
            id: id.into(),
            kind: IndicatorKind::Ema,
            pane: "main".into(),
            params: BTreeMap::from([("period".into(), period)]),
            style: IndicatorStyle::default(),
            visible: true,
        }
    }

    fn sample_candles(count: usize) -> Vec<OhlcvCandle> {
        (0..count)
            .map(|i| OhlcvCandle {
                timestamp: (i as i64) * 60,
                open: 100.0 + (i as f64),
                high: 105.0 + (i as f64),
                low: 95.0 + (i as f64),
                close: 102.0 + (i as f64),
                volume: 1000.0,
            })
            .collect()
    }

    #[test]
    fn cache_key_uses_feed_version_kind_and_parameters_not_instance_id() {
        assert_eq!(
            IndicatorCacheKey::new(7, &indicator("a", 20.0)),
            IndicatorCacheKey::new(7, &indicator("b", 20.0))
        );
        assert_ne!(
            IndicatorCacheKey::new(7, &indicator("a", 20.0)),
            IndicatorCacheKey::new(8, &indicator("a", 20.0))
        );
        assert_ne!(
            IndicatorCacheKey::new(7, &indicator("a", 20.0)),
            IndicatorCacheKey::new(7, &indicator("a", 50.0))
        );
    }

    #[test]
    fn validate_params_catches_invalid_inputs() {
        let mut params = BTreeMap::new();
        params.insert("period".into(), 0.0);
        assert!(validate_params(&IndicatorKind::Ema, &params).is_err());

        params.insert("period".into(), 20.0);
        assert!(validate_params(&IndicatorKind::Ema, &params).is_ok());

        let mut macd_params = BTreeMap::new();
        macd_params.insert("short".into(), 26.0);
        macd_params.insert("long".into(), 12.0); // short >= long is invalid
        macd_params.insert("signal".into(), 9.0);
        assert!(validate_params(&IndicatorKind::Macd, &macd_params).is_err());
    }

    #[test]
    fn cache_reuses_computation_for_same_version() {
        let mut cache = IndicatorCache::new();
        let candles = sample_candles(50);
        let inst = indicator("test-1", 10.0);

        let res1 = cache.get_or_compute(1, &inst, &candles);
        let res2 = cache.get_or_compute(1, &inst, &candles);
        assert!(Arc::ptr_eq(&res1, &res2));

        // Version changed -> new computation
        let res3 = cache.get_or_compute(2, &inst, &candles);
        assert!(!Arc::ptr_eq(&res1, &res3));
    }
}
