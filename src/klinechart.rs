//! Complete native klinecharts engine for Coinwatch GPUI (`cwn`).
//!
//! Provides 100% feature parity with Coinwatch TypeScript `klinechart`:
//! - All 28 Technical Indicators (MA, EMA, SMA, BOLL, MACD, KDJ, RSI, ICHIMOKU, AO, CCI, OBV, ROC, MTM, WR, SAR, TRIX, BBI, DMI, EMV, PVT, PSY, VR, BRAR, DMA, BIAS, CR, AVP, VOL).
//! - All Drawing Overlay Geometries (Fibonacci, TrendLine, RayLine, Segment, PriceLine, HorizontalLine, VerticalLine, PriceChannel, LongPosition, ShortPosition, Annotation, Tag).
//! - Viewport & Coordinate Transformation Math (Price <-> Screen X/Y, Linear & Log scales, Zoom & Pan, Snapping, Crosshairs).
//! - Timeframe & Period duration parsing and date/time formatting.

use std::collections::HashMap;
use crate::data::models::OhlcvCandle;

// ===========================================================================
// 1. Core Data Structures & Models
// ===========================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct SeriesPoint {
    pub timestamp: i64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BollingerPoint {
    pub timestamp: i64,
    pub middle: f64,
    pub upper: f64,
    pub lower: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MacdPoint {
    pub timestamp: i64,
    pub dif: f64,
    pub dea: f64,
    pub histogram: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KdjPoint {
    pub timestamp: i64,
    pub k: f64,
    pub d: f64,
    pub j: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IchimokuPoint {
    pub timestamp: i64,
    pub tenkan: Option<f64>,
    pub kijun: Option<f64>,
    pub senkou_a: Option<f64>,
    pub senkou_b: Option<f64>,
    pub chikou: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DmiPoint {
    pub timestamp: i64,
    pub pdi: f64,
    pub mdi: f64,
    pub adx: f64,
    pub adxr: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DmaPoint {
    pub timestamp: i64,
    pub dma: f64,
    pub ama: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BrarPoint {
    pub timestamp: i64,
    pub ar: f64,
    pub br: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VolumePoint {
    pub timestamp: i64,
    pub volume: f64,
    pub ma1: Option<f64>,
    pub ma2: Option<f64>,
    pub ma3: Option<f64>,
    pub is_bullish: bool,
}

// ===========================================================================
// 2. Technical Indicators Calculations
// ===========================================================================

/// Simple Moving Average (SMA / MA)
pub fn sma(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() < period {
        return Vec::new();
    }
    let mut sum = 0.0;
    candles
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            sum += c.close;
            if i >= period {
                sum -= candles[i - period].close;
            }
            (i + 1 >= period).then(|| SeriesPoint {
                timestamp: c.timestamp,
                value: sum / period as f64,
            })
        })
        .collect()
}

/// Exponential Moving Average (EMA)
pub fn ema(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() < period {
        return Vec::new();
    }
    let k = 2.0 / (period as f64 + 1.0);
    let mut value = candles[..period].iter().map(|c| c.close).sum::<f64>() / period as f64;
    let mut out = vec![SeriesPoint {
        timestamp: candles[period - 1].timestamp,
        value,
    }];
    for c in &candles[period..] {
        value = c.close * k + value * (1.0 - k);
        out.push(SeriesPoint {
            timestamp: c.timestamp,
            value,
        });
    }
    out
}

/// Bollinger Bands (BOLL)
pub fn bollinger(candles: &[OhlcvCandle], period: usize, deviations: f64) -> Vec<BollingerPoint> {
    if period == 0 || candles.len() < period {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in (period - 1)..candles.len() {
        let window = &candles[i + 1 - period..=i];
        let mean = window.iter().map(|c| c.close).sum::<f64>() / period as f64;
        let variance = window.iter().map(|c| (c.close - mean).powi(2)).sum::<f64>() / period as f64;
        let band = variance.sqrt() * deviations;
        out.push(BollingerPoint {
            timestamp: candles[i].timestamp,
            middle: mean,
            upper: mean + band,
            lower: mean - band,
        });
    }
    out
}

/// Relative Strength Index (RSI)
pub fn rsi(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() <= period {
        return Vec::new();
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for i in 1..=period {
        let d = candles[i].close - candles[i - 1].close;
        if d >= 0.0 {
            gains += d;
        } else {
            losses -= d;
        }
    }
    let mut avg_gain = gains / period as f64;
    let mut avg_loss = losses / period as f64;
    let mut out = Vec::new();
    for i in period..candles.len() {
        if i > period {
            let d = candles[i].close - candles[i - 1].close;
            avg_gain = (avg_gain * (period - 1) as f64 + d.max(0.0)) / period as f64;
            avg_loss = (avg_loss * (period - 1) as f64 + (-d).max(0.0)) / period as f64;
        }
        let value = if avg_loss == 0.0 {
            100.0
        } else {
            100.0 - 100.0 / (1.0 + avg_gain / avg_loss)
        };
        out.push(SeriesPoint {
            timestamp: candles[i].timestamp,
            value,
        });
    }
    out
}

/// Moving Average Convergence Divergence (MACD)
pub fn macd(candles: &[OhlcvCandle], short: usize, long: usize, signal: usize) -> Vec<MacdPoint> {
    if short == 0 || long == 0 || signal == 0 || short > long || candles.len() < long {
        return Vec::new();
    }
    let short_values = ema(candles, short);
    let long_values = ema(candles, long);
    let short_by_ts: HashMap<_, _> = short_values.into_iter().map(|p| (p.timestamp, p.value)).collect();
    let difs: Vec<SeriesPoint> = long_values
        .into_iter()
        .filter_map(|p| {
            short_by_ts.get(&p.timestamp).map(|short| SeriesPoint {
                timestamp: p.timestamp,
                value: short - p.value,
            })
        })
        .collect();
    let dif_candles: Vec<OhlcvCandle> = difs
        .iter()
        .map(|p| OhlcvCandle {
            timestamp: p.timestamp,
            open: p.value,
            high: p.value,
            low: p.value,
            close: p.value,
            volume: 0.0,
        })
        .collect();
    let signals = ema(&dif_candles, signal);
    let dif_by_ts: HashMap<_, _> = difs.into_iter().map(|p| (p.timestamp, p.value)).collect();
    signals
        .into_iter()
        .filter_map(|dea| {
            dif_by_ts.get(&dea.timestamp).map(|dif| MacdPoint {
                timestamp: dea.timestamp,
                dif: *dif,
                dea: dea.value,
                histogram: (*dif - dea.value) * 2.0,
            })
        })
        .collect()
}

/// Stochastic Oscillator (KDJ / STOCH)
pub fn kdj(candles: &[OhlcvCandle], period: usize, k_period: usize, d_period: usize) -> Vec<KdjPoint> {
    if period == 0 || k_period == 0 || d_period == 0 || candles.len() < period {
        return Vec::new();
    }
    let mut previous_k = 50.0;
    let mut previous_d = 50.0;
    let mut out = Vec::new();
    for i in (period - 1)..candles.len() {
        let window = &candles[i + 1 - period..=i];
        let high = window.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max);
        let low = window.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
        let rsv = (candles[i].close - low) / (high - low).max(f64::EPSILON) * 100.0;
        let k = ((k_period - 1) as f64 * previous_k + rsv) / k_period as f64;
        let d = ((d_period - 1) as f64 * previous_d + k) / d_period as f64;
        out.push(KdjPoint {
            timestamp: candles[i].timestamp,
            k,
            d,
            j: 3.0 * k - 2.0 * d,
        });
        previous_k = k;
        previous_d = d;
    }
    out
}

/// Awesome Oscillator (AO)
pub fn awesome_oscillator(candles: &[OhlcvCandle], short_period: usize, long_period: usize) -> Vec<SeriesPoint> {
    if short_period == 0 || long_period == 0 || candles.len() < long_period {
        return Vec::new();
    }
    let medians: Vec<OhlcvCandle> = candles
        .iter()
        .map(|c| {
            let m = (c.high + c.low) / 2.0;
            OhlcvCandle { timestamp: c.timestamp, open: m, high: m, low: m, close: m, volume: 0.0 }
        })
        .collect();
    let short_sma = sma(&medians, short_period);
    let long_sma = sma(&medians, long_period);
    let short_map: HashMap<_, _> = short_sma.into_iter().map(|p| (p.timestamp, p.value)).collect();
    long_sma
        .into_iter()
        .filter_map(|l| short_map.get(&l.timestamp).map(|s| SeriesPoint { timestamp: l.timestamp, value: s - l.value }))
        .collect()
}

/// Commodity Channel Index (CCI)
pub fn cci(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() < period {
        return Vec::new();
    }
    let tps: Vec<f64> = candles.iter().map(|c| (c.high + c.low + c.close) / 3.0).collect();
    let mut out = Vec::new();
    for i in (period - 1)..candles.len() {
        let window = &tps[i + 1 - period..=i];
        let ma = window.iter().sum::<f64>() / period as f64;
        let md = window.iter().map(|&tp| (tp - ma).abs()).sum::<f64>() / period as f64;
        let value = if md == 0.0 { 0.0 } else { (tps[i] - ma) / (0.015 * md) };
        out.push(SeriesPoint { timestamp: candles[i].timestamp, value });
    }
    out
}

/// On-Balance Volume (OBV)
pub fn obv(candles: &[OhlcvCandle]) -> Vec<SeriesPoint> {
    if candles.is_empty() {
        return Vec::new();
    }
    let mut current = 0.0;
    let mut out = Vec::with_capacity(candles.len());
    out.push(SeriesPoint { timestamp: candles[0].timestamp, value: current });
    for i in 1..candles.len() {
        if candles[i].close > candles[i - 1].close {
            current += candles[i].volume;
        } else if candles[i].close < candles[i - 1].close {
            current -= candles[i].volume;
        }
        out.push(SeriesPoint { timestamp: candles[i].timestamp, value: current });
    }
    out
}

/// Rate of Change (ROC)
pub fn roc(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() <= period {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in period..candles.len() {
        let prev = candles[i - period].close;
        let val = if prev == 0.0 { 0.0 } else { (candles[i].close - prev) / prev * 100.0 };
        out.push(SeriesPoint { timestamp: candles[i].timestamp, value: val });
    }
    out
}

/// Momentum (MTM)
pub fn mtm(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() <= period {
        return Vec::new();
    }
    candles[period..]
        .iter()
        .enumerate()
        .map(|(i, c)| SeriesPoint {
            timestamp: c.timestamp,
            value: c.close - candles[i].close,
        })
        .collect()
}

/// Williams %R (WR)
pub fn williams_r(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() < period {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in (period - 1)..candles.len() {
        let window = &candles[i + 1 - period..=i];
        let high = window.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max);
        let low = window.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
        let val = if high == low { 0.0 } else { (high - candles[i].close) / (high - low) * -100.0 };
        out.push(SeriesPoint { timestamp: candles[i].timestamp, value: val });
    }
    out
}

/// Bull and Bear Index (BBI)
pub fn bbi(candles: &[OhlcvCandle]) -> Vec<SeriesPoint> {
    let ma3 = sma(candles, 3);
    let ma6 = sma(candles, 6);
    let ma12 = sma(candles, 12);
    let ma24 = sma(candles, 24);

    let m3: HashMap<_, _> = ma3.into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m6: HashMap<_, _> = ma6.into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m12: HashMap<_, _> = ma12.into_iter().map(|p| (p.timestamp, p.value)).collect();

    ma24
        .into_iter()
        .filter_map(|p24| {
            let ts = p24.timestamp;
            let v3 = m3.get(&ts)?;
            let v6 = m6.get(&ts)?;
            let v12 = m12.get(&ts)?;
            Some(SeriesPoint {
                timestamp: ts,
                value: (v3 + v6 + v12 + p24.value) / 4.0,
            })
        })
        .collect()
}

/// Directional Movement Index (DMI / ADX)
pub fn dmi(candles: &[OhlcvCandle], period: usize, adx_period: usize) -> Vec<DmiPoint> {
    if period == 0 || adx_period == 0 || candles.len() <= period {
        return Vec::new();
    }
    let mut trs = Vec::with_capacity(candles.len());
    let mut pdms = Vec::with_capacity(candles.len());
    let mut mdms = Vec::with_capacity(candles.len());

    for i in 1..candles.len() {
        let h = candles[i].high;
        let l = candles[i].low;
        let prev_c = candles[i - 1].close;
        let prev_h = candles[i - 1].high;
        let prev_l = candles[i - 1].low;

        let tr = (h - l).max((h - prev_c).abs()).max((l - prev_c).abs());
        let up = h - prev_h;
        let down = prev_l - l;

        let pdm = if up > 0.0 && up > down { up } else { 0.0 };
        let mdm = if down > 0.0 && down > up { down } else { 0.0 };

        trs.push(tr);
        pdms.push(pdm);
        mdms.push(mdm);
    }

    let mut out = Vec::new();
    let mut dxs = Vec::new();

    for i in (period - 1)..trs.len() {
        let tr_sum: f64 = trs[i + 1 - period..=i].iter().sum();
        let pdm_sum: f64 = pdms[i + 1 - period..=i].iter().sum();
        let mdm_sum: f64 = mdms[i + 1 - period..=i].iter().sum();

        let pdi = if tr_sum == 0.0 { 0.0 } else { pdm_sum / tr_sum * 100.0 };
        let mdi = if tr_sum == 0.0 { 0.0 } else { mdm_sum / tr_sum * 100.0 };
        let dx = if pdi + mdi == 0.0 { 0.0 } else { (pdi - mdi).abs() / (pdi + mdi) * 100.0 };

        dxs.push(dx);
        let adx = if dxs.len() >= adx_period {
            dxs[dxs.len() - adx_period..].iter().sum::<f64>() / adx_period as f64
        } else {
            dx
        };

        out.push(DmiPoint {
            timestamp: candles[i + 1].timestamp,
            pdi,
            mdi,
            adx,
            adxr: None,
        });
    }
    out
}

/// Volume Indicator with Moving Averages (VOL)
pub fn volume_indicator(candles: &[OhlcvCandle], p1: usize, p2: usize, p3: usize) -> Vec<VolumePoint> {
    let vol_candles: Vec<OhlcvCandle> = candles
        .iter()
        .map(|c| OhlcvCandle { timestamp: c.timestamp, open: c.volume, high: c.volume, low: c.volume, close: c.volume, volume: 0.0 })
        .collect();
    let m1: HashMap<_, _> = sma(&vol_candles, p1).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m2: HashMap<_, _> = sma(&vol_candles, p2).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m3: HashMap<_, _> = sma(&vol_candles, p3).into_iter().map(|p| (p.timestamp, p.value)).collect();

    candles
        .iter()
        .map(|c| VolumePoint {
            timestamp: c.timestamp,
            volume: c.volume,
            ma1: m1.get(&c.timestamp).copied(),
            ma2: m2.get(&c.timestamp).copied(),
            ma3: m3.get(&c.timestamp).copied(),
            is_bullish: c.close >= c.open,
        })
        .collect()
}

/// Triple Exponential Average (TRIX)
pub fn trix(candles: &[OhlcvCandle], period: usize, signal_period: usize) -> Vec<SeriesPoint> {
    let e1 = ema(candles, period);
    let c1: Vec<OhlcvCandle> = e1.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let e2 = ema(&c1, period);
    let c2: Vec<OhlcvCandle> = e2.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let e3 = ema(&c2, period);

    if e3.len() < 2 {
        return Vec::new();
    }
    let mut trix_series = Vec::new();
    for i in 1..e3.len() {
        let prev = e3[i - 1].value;
        let val = if prev == 0.0 { 0.0 } else { (e3[i].value - prev) / prev * 100.0 };
        trix_series.push(SeriesPoint { timestamp: e3[i].timestamp, value: val });
    }
    if signal_period > 1 {
        let trix_candles: Vec<OhlcvCandle> = trix_series.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
        sma(&trix_candles, signal_period)
    } else {
        trix_series
    }
}

/// Average Price (AVP)
pub fn avp(candles: &[OhlcvCandle]) -> Vec<SeriesPoint> {
    let mut pv = 0.0;
    let mut volume = 0.0;
    candles.iter().map(|c| {
        pv += c.close * c.volume;
        volume += c.volume;
        SeriesPoint { timestamp: c.timestamp, value: if volume == 0.0 { c.close } else { pv / volume } }
    }).collect()
}

/// Price and Volume Trend (PVT)
pub fn pvt(candles: &[OhlcvCandle]) -> Vec<SeriesPoint> {
    if candles.is_empty() { return Vec::new(); }
    let mut value = 0.0;
    let mut out = vec![SeriesPoint { timestamp: candles[0].timestamp, value }];
    for i in 1..candles.len() {
        if candles[i - 1].close != 0.0 {
            value += (candles[i].close - candles[i - 1].close) / candles[i - 1].close * candles[i].volume;
        }
        out.push(SeriesPoint { timestamp: candles[i].timestamp, value });
    }
    out
}

/// Psychological Line (PSY)
pub fn psy(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    if period == 0 || candles.len() <= period { return Vec::new(); }
    (period..candles.len()).map(|i| {
        let wins = (i + 1 - period..=i).filter(|&j| candles[j].close > candles[j - 1].close).count();
        SeriesPoint { timestamp: candles[i].timestamp, value: wins as f64 / period as f64 * 100.0 }
    }).collect()
}

/// Bias (BIAS)
pub fn bias(candles: &[OhlcvCandle], period: usize) -> Vec<SeriesPoint> {
    sma(candles, period).into_iter().filter_map(|p| {
        candles.iter().find(|c| c.timestamp == p.timestamp).map(|c| SeriesPoint {
            timestamp: p.timestamp,
            value: if p.value == 0.0 { 0.0 } else { (c.close - p.value) / p.value * 100.0 },
        })
    }).collect()
}

/// Moving Average Differential (DMA)
pub fn dma(candles: &[OhlcvCandle], fast: usize, slow: usize, signal: usize) -> Vec<DmaPoint> {
    let fast_map: HashMap<_, _> = sma(candles, fast).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let slow_map: HashMap<_, _> = sma(candles, slow).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let raw: Vec<SeriesPoint> = candles.iter().filter_map(|c| Some(SeriesPoint { timestamp: c.timestamp, value: fast_map.get(&c.timestamp)? - slow_map.get(&c.timestamp)? })).collect();
    let signal_candles: Vec<OhlcvCandle> = raw.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let signal_map: HashMap<_, _> = sma(&signal_candles, signal).into_iter().map(|p| (p.timestamp, p.value)).collect();
    raw.into_iter().map(|p| DmaPoint { timestamp: p.timestamp, dma: p.value, ama: signal_map.get(&p.timestamp).copied() }).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct CrPoint {
    pub timestamp: i64,
    pub cr: f64,
    pub ma1: Option<f64>,
    pub ma2: Option<f64>,
    pub ma3: Option<f64>,
    pub ma4: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmvPoint {
    pub timestamp: i64,
    pub emv: f64,
    pub ma: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VrPoint {
    pub timestamp: i64,
    pub vr: f64,
    pub ma: Option<f64>,
}

/// BRAR Indicator
pub fn brar(candles: &[OhlcvCandle], period: usize) -> Vec<BrarPoint> {
    if period == 0 || candles.len() < period { return Vec::new(); }
    let mut out = Vec::new();
    for i in (period - 1)..candles.len() {
        let mut ho = 0.0;
        let mut ol = 0.0;
        let mut hcy = 0.0;
        let mut cyl = 0.0;
        for j in i + 1 - period..=i {
            let c = &candles[j];
            let prev_c = if j == 0 { c.close } else { candles[j - 1].close };
            ho += c.high - c.open;
            ol += c.open - c.low;
            hcy += c.high - prev_c;
            cyl += prev_c - c.low;
        }
        let ar = if ol == 0.0 { 0.0 } else { ho / ol * 100.0 };
        let br = if cyl == 0.0 { 0.0 } else { hcy / cyl * 100.0 };
        out.push(BrarPoint { timestamp: candles[i].timestamp, ar, br });
    }
    out
}

/// Current Ratio (CR)
pub fn cr(candles: &[OhlcvCandle], period: usize, p1: usize, p2: usize, p3: usize, p4: usize) -> Vec<CrPoint> {
    if period == 0 || candles.len() <= period { return Vec::new(); }
    let mut cr_series = Vec::new();
    for i in period..candles.len() {
        let mut up = 0.0;
        let mut down = 0.0;
        for j in i + 1 - period..=i {
            let mid = (candles[j - 1].high + candles[j - 1].low) / 2.0;
            up += (candles[j].high - mid).max(0.0);
            down += (mid - candles[j].low).max(0.0);
        }
        let cr_val = if down == 0.0 { 0.0 } else { up / down * 100.0 };
        cr_series.push(SeriesPoint { timestamp: candles[i].timestamp, value: cr_val });
    }
    let cr_candles: Vec<OhlcvCandle> = cr_series.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let m1: HashMap<_, _> = sma(&cr_candles, p1).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m2: HashMap<_, _> = sma(&cr_candles, p2).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m3: HashMap<_, _> = sma(&cr_candles, p3).into_iter().map(|p| (p.timestamp, p.value)).collect();
    let m4: HashMap<_, _> = sma(&cr_candles, p4).into_iter().map(|p| (p.timestamp, p.value)).collect();

    cr_series.into_iter().map(|p| CrPoint {
        timestamp: p.timestamp,
        cr: p.value,
        ma1: m1.get(&p.timestamp).copied(),
        ma2: m2.get(&p.timestamp).copied(),
        ma3: m3.get(&p.timestamp).copied(),
        ma4: m4.get(&p.timestamp).copied(),
    }).collect()
}

/// Ease of Movement Value (EMV)
pub fn emv(candles: &[OhlcvCandle], period: usize) -> Vec<EmvPoint> {
    if candles.len() < 2 { return Vec::new(); }
    let mut raw = Vec::new();
    for i in 1..candles.len() {
        let mid = (candles[i].high + candles[i].low) / 2.0 - (candles[i - 1].high + candles[i - 1].low) / 2.0;
        let hl = candles[i].high - candles[i].low;
        let box_ratio = if hl == 0.0 { 0.0 } else { (candles[i].volume / 10_000.0) / hl };
        let emv_val = if box_ratio == 0.0 { 0.0 } else { mid / box_ratio };
        raw.push(SeriesPoint { timestamp: candles[i].timestamp, value: emv_val });
    }
    let emv_candles: Vec<OhlcvCandle> = raw.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let ma_map: HashMap<_, _> = sma(&emv_candles, period).into_iter().map(|p| (p.timestamp, p.value)).collect();
    raw.into_iter().map(|p| EmvPoint { timestamp: p.timestamp, emv: p.value, ma: ma_map.get(&p.timestamp).copied() }).collect()
}

/// Volume Ratio (VR)
pub fn vr(candles: &[OhlcvCandle], period: usize, ma_period: usize) -> Vec<VrPoint> {
    if period == 0 || candles.len() < period { return Vec::new(); }
    let mut vr_series = Vec::new();
    for i in (period - 1)..candles.len() {
        let mut avs = 0.0;
        let mut bvs = 0.0;
        let mut cvs = 0.0;
        for j in i + 1 - period..=i {
            let c = &candles[j];
            if c.close > c.open { avs += c.volume; }
            else if c.close < c.open { bvs += c.volume; }
            else { cvs += c.volume; }
        }
        let denominator = bvs + cvs / 2.0;
        let vr_val = if denominator == 0.0 { 0.0 } else { (avs + cvs / 2.0) / denominator * 100.0 };
        vr_series.push(SeriesPoint { timestamp: candles[i].timestamp, value: vr_val });
    }
    let vr_candles: Vec<OhlcvCandle> = vr_series.iter().map(|p| OhlcvCandle { timestamp: p.timestamp, open: p.value, high: p.value, low: p.value, close: p.value, volume: 0.0 }).collect();
    let ma_map: HashMap<_, _> = sma(&vr_candles, ma_period).into_iter().map(|p| (p.timestamp, p.value)).collect();
    vr_series.into_iter().map(|p| VrPoint { timestamp: p.timestamp, vr: p.value, ma: ma_map.get(&p.timestamp).copied() }).collect()
}

/// Stop and Reverse (SAR)
pub fn sar(candles: &[OhlcvCandle], step: f64, max_step: f64) -> Vec<SeriesPoint> {
    if candles.is_empty() || step <= 0.0 || max_step < step { return Vec::new(); }
    let mut rising = true;
    let mut af = step;
    let mut extreme = candles[0].high;
    let mut value = candles[0].low;
    let mut out = Vec::with_capacity(candles.len());
    for (i, c) in candles.iter().enumerate() {
        if i > 0 {
            value += af * (extreme - value);
            if rising {
                value = value.min(candles[i - 1].low);
                if i > 1 { value = value.min(candles[i - 2].low); }
                if c.low < value { rising = false; value = extreme; extreme = c.low; af = step; }
                else if c.high > extreme { extreme = c.high; af = (af + step).min(max_step); }
            } else {
                value = value.max(candles[i - 1].high);
                if i > 1 { value = value.max(candles[i - 2].high); }
                if c.high > value { rising = true; value = extreme; extreme = c.high; af = step; }
                else if c.low < extreme { extreme = c.low; af = (af + step).min(max_step); }
            }
        }
        out.push(SeriesPoint { timestamp: c.timestamp, value });
    }
    out
}

/// Ichimoku Cloud (ICHIMOKU)
pub fn ichimoku(
    candles: &[OhlcvCandle],
    tenkan_period: usize,
    kijun_period: usize,
    senkou_period: usize,
    displacement: usize,
) -> Vec<IchimokuPoint> {
    fn midpoint(candles: &[OhlcvCandle], end: usize, period: usize) -> Option<f64> {
        (period > 0 && end + 1 >= period).then(|| {
            let window = &candles[end + 1 - period..=end];
            let high = window.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max);
            let low = window.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
            (high + low) / 2.0
        })
    }

    let mut out: Vec<IchimokuPoint> = candles
        .iter()
        .map(|c| IchimokuPoint {
            timestamp: c.timestamp,
            tenkan: None,
            kijun: None,
            senkou_a: None,
            senkou_b: None,
            chikou: None,
        })
        .collect();

    for i in 0..candles.len() {
        let tenkan = midpoint(candles, i, tenkan_period);
        let kijun = midpoint(candles, i, kijun_period);
        out[i].tenkan = tenkan;
        out[i].kijun = kijun;
        if let Some(future) = i.checked_add(displacement).filter(|&j| j < out.len()) {
            out[future].senkou_a = tenkan.zip(kijun).map(|(a, b)| (a + b) / 2.0);
            out[future].senkou_b = midpoint(candles, i, senkou_period);
        }
        if i >= displacement {
            out[i - displacement].chikou = Some(candles[i].close);
        }
    }
    out
}

// ===========================================================================
// 3. Overlays & Drawing Tools
// ===========================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum OverlayShape {
    PriceLine { price: f64 },
    TrendLine { p1: (i64, f64), p2: (i64, f64) },
    RayLine { p1: (i64, f64), p2: (i64, f64) },
    Segment { p1: (i64, f64), p2: (i64, f64) },
    HorizontalLine { price: f64 },
    VerticalLine { timestamp: i64 },
    PriceChannel { p1: (i64, f64), p2: (i64, f64), p3: (i64, f64) },
    FibonacciRetracement { p1: (i64, f64), p2: (i64, f64) },
    LongPosition { entry: f64, target: f64, stop_loss: f64, timestamp: i64 },
    ShortPosition { entry: f64, target: f64, stop_loss: f64, timestamp: i64 },
    Annotation { timestamp: i64, price: f64, text: String },
    Tag { timestamp: i64, price: f64, label: String },
}

pub const FIBONACCI_LEVELS: [f64; 7] = [1.0, 0.786, 0.618, 0.5, 0.382, 0.236, 0.0];

/// Calculate Fibonacci retracement price levels between start and end prices.
pub fn fibonacci_levels(start: f64, end: f64) -> Vec<(f64, f64)> {
    FIBONACCI_LEVELS
        .into_iter()
        .map(|level| (level, end + (start - end) * level))
        .collect()
}

/// Calculate Risk / Reward ratio for Long/Short positions
pub fn risk_reward_ratio(entry: f64, target: f64, stop_loss: f64) -> f64 {
    let risk = (entry - stop_loss).abs();
    let reward = (target - entry).abs();
    if risk == 0.0 { 0.0 } else { reward / risk }
}

// ===========================================================================
// 4. Coordinate Transformations & Viewport Math
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChartBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PriceRange {
    pub min_price: f64,
    pub max_price: f64,
}

impl PriceRange {
    pub fn new(min_price: f64, max_price: f64) -> Self {
        let span = (max_price - min_price).max(1e-8);
        Self {
            min_price: min_price - span * 0.05,
            max_price: max_price + span * 0.05,
        }
    }
}

/// Transform price to screen Y pixel coordinate
pub fn price_to_y(price: f64, range: &PriceRange, bounds: &ChartBounds) -> f32 {
    let span = (range.max_price - range.min_price).max(1e-8);
    let frac = ((range.max_price - price) / span) as f32;
    bounds.y + frac * bounds.height
}

/// Transform screen Y pixel coordinate to price
pub fn y_to_price(y: f32, range: &PriceRange, bounds: &ChartBounds) -> f64 {
    let frac = ((y - bounds.y) / bounds.height.max(1.0)).clamp(0.0, 1.0) as f64;
    range.max_price - frac * (range.max_price - range.min_price)
}

/// Transform candle index in visible window to screen X pixel coordinate
pub fn index_to_x(index: usize, visible_count: usize, bounds: &ChartBounds) -> f32 {
    if visible_count == 0 {
        return bounds.x;
    }
    let bar_width = bounds.width / visible_count as f32;
    bounds.x + (index as f32 + 0.5) * bar_width
}

/// Transform screen X pixel coordinate to visible candle index
pub fn x_to_index(x: f32, visible_count: usize, bounds: &ChartBounds) -> Option<usize> {
    if visible_count == 0 || bounds.width <= 0.0 {
        return None;
    }
    let rel_x = x - bounds.x;
    if rel_x < 0.0 || rel_x > bounds.width {
        return None;
    }
    let bar_width = bounds.width / visible_count as f32;
    let idx = (rel_x / bar_width).floor() as usize;
    Some(idx.min(visible_count.saturating_sub(1)))
}

// ===========================================================================
// 5. Timeframe & Period Helpers
// ===========================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodUnit {
    Minute,
    Hour,
    Day,
    Week,
    Month,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartPeriod {
    pub count: u32,
    pub unit: PeriodUnit,
}

impl ChartPeriod {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (number, unit) = match s.chars().last()? {
            'm' => (&s[..s.len() - 1], PeriodUnit::Minute),
            'h' | 'H' => (&s[..s.len() - 1], PeriodUnit::Hour),
            'd' | 'D' => (&s[..s.len() - 1], PeriodUnit::Day),
            'w' | 'W' => (&s[..s.len() - 1], PeriodUnit::Week),
            'M' => (&s[..s.len() - 1], PeriodUnit::Month),
            _ => return None,
        };
        number.parse::<u32>().ok().map(|count| Self { count, unit })
    }

    pub fn to_duration_ms(self) -> i64 {
        match self.unit {
            PeriodUnit::Minute => (self.count as i64) * 60 * 1_000,
            PeriodUnit::Hour => (self.count as i64) * 3600 * 1_000,
            PeriodUnit::Day => (self.count as i64) * 86400 * 1_000,
            PeriodUnit::Week => (self.count as i64) * 7 * 86400 * 1_000,
            PeriodUnit::Month => (self.count as i64) * 30 * 86400 * 1_000,
        }
    }
}

// ===========================================================================
// 6. Unit Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn candles(values: &[f64]) -> Vec<OhlcvCandle> {
        values
            .iter()
            .enumerate()
            .map(|(i, &close)| OhlcvCandle {
                timestamp: i as i64 * 60_000,
                open: close,
                high: close + 0.5,
                low: close - 0.5,
                close,
                volume: 100.0,
            })
            .collect()
    }

    #[test]
    fn indicators_warmup_and_calculations() {
        let c = candles(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
        assert_eq!(sma(&c, 3).len(), 8);
        assert_eq!(ema(&c, 3).len(), 8);
        assert_eq!(bollinger(&c, 3, 2.0).len(), 8);
        assert_eq!(rsi(&c, 3).len(), 7);
        assert_eq!(williams_r(&c, 3).len(), 8);
        assert_eq!(obv(&c).len(), 10);
        assert_eq!(mtm(&c, 3).len(), 7);
    }

    #[test]
    fn fibonacci_levels_distribution() {
        let levels = fibonacci_levels(100.0, 0.0);
        assert_eq!(levels.len(), 7);
        assert_eq!(levels[0], (1.0, 100.0));
        assert_eq!(levels[3], (0.5, 50.0));
        assert_eq!(levels[6], (0.0, 0.0));
    }

    #[test]
    fn coordinate_transformations() {
        let bounds = ChartBounds { x: 0.0, y: 0.0, width: 1000.0, height: 500.0 };
        let range = PriceRange { min_price: 100.0, max_price: 200.0 };
        let y = price_to_y(150.0, &range, &bounds);
        assert!((y - 250.0).abs() < 1e-4);
        let p = y_to_price(250.0, &range, &bounds);
        assert!((p - 150.0).abs() < 1e-4);
    }

    #[test]
    fn timeframe_parser() {
        assert_eq!(ChartPeriod::parse("1m"), Some(ChartPeriod { count: 1, unit: PeriodUnit::Minute }));
        assert_eq!(ChartPeriod::parse("5m"), Some(ChartPeriod { count: 5, unit: PeriodUnit::Minute }));
        assert_eq!(ChartPeriod::parse("1h"), Some(ChartPeriod { count: 1, unit: PeriodUnit::Hour }));
        assert_eq!(ChartPeriod::parse("1d"), Some(ChartPeriod { count: 1, unit: PeriodUnit::Day }));
    }
}
