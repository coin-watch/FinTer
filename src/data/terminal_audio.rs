//! Native terminal synthesizer. One lazy device-owning thread, one replaceable
//! pending cue, at most one active voice. No audio IO or synthesis on GPUI.
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};
use rodio::{OutputStream, Sink};
use rodio::buffer::SamplesBuffer;

const GAP: Duration = Duration::from_millis(55);
const WHALE_GAP: Duration = Duration::from_millis(500);
static ENABLED: AtomicBool = AtomicBool::new(true);
static AUDIO: OnceLock<TerminalAudio> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone { Buy, Sell, Whale, Preview }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStatus { Uninitialized, Initializing, Ready, Unavailable }

#[derive(Default)]
struct RateLimiter { last: Option<Instant>, whale: Option<Instant> }
impl RateLimiter {
    fn allow(&mut self, tone: Tone, now: Instant) -> bool {
        if tone == Tone::Whale {
            // Whales bypass a recent tick, but cannot flood or overlap each other.
            if self.whale.is_some_and(|t| now.duration_since(t) < WHALE_GAP) { return false; }
            self.whale = Some(now);
        } else if self.last.is_some_and(|t| now.duration_since(t) < GAP) { return false; }
        self.last = Some(now);
        true
    }
}

#[derive(Default)]
struct Mailbox { pending: Option<(Tone, Instant)> }
impl Mailbox {
    fn put(&mut self, tone: Tone, now: Instant) {
        // Never replace a pending whale with a tick. Everything else coalesces.
        if self.pending.is_none_or(|(old, _)| old != Tone::Whale || tone == Tone::Whale) {
            self.pending = Some((tone, now));
        }
    }
}

pub struct TerminalAudio {
    pending: Mutex<Mailbox>,
    status: AtomicU8,
    limiter: Mutex<RateLimiter>,
}
impl TerminalAudio {
    pub fn shared() -> &'static Self {
        AUDIO.get_or_init(|| {
            // Spawn only. The worker obtains AUDIO after initialization completes.
            let state = Self { pending: Mutex::new(Mailbox::default()), status: AtomicU8::new(1), limiter: Mutex::new(RateLimiter::default()) };
            if std::thread::Builder::new().name("finter-audio".into()).spawn(worker).is_err() {
                state.status.store(3, Ordering::Release);
            }
            state
        })
    }
    /// Does not initialize the device.
    pub fn current_status() -> AudioStatus {
        match AUDIO.get().map(|a| a.status.load(Ordering::Acquire)) {
            None => AudioStatus::Uninitialized, Some(1) => AudioStatus::Initializing,
            Some(2) => AudioStatus::Ready, _ => AudioStatus::Unavailable,
        }
    }
    pub fn set_global_enabled(enabled: bool) {
        ENABLED.store(enabled, Ordering::Release);
        if !enabled {
            if let Some(audio) = AUDIO.get() {
                if let Ok(mut mailbox) = audio.pending.lock() { mailbox.pending = None; }
            }
        }
    }
    pub fn play(&self, tone: Tone) {
        if !ENABLED.load(Ordering::Acquire) || Self::current_status() == AudioStatus::Unavailable { return; }
        // try_lock means playback never waits on the native audio thread.
        let Ok(mut limiter) = self.limiter.try_lock() else { return };
        let now = Instant::now();
        if !limiter.allow(tone, now) { return; }
        if let Ok(mut pending) = self.pending.try_lock() { pending.put(tone, now); }
    }
}

fn worker() {
    let audio = loop {
        if let Some(audio) = AUDIO.get() { break audio; }
        std::thread::sleep(Duration::from_millis(1));
    };
    let Ok((_stream, handle)) = OutputStream::try_default() else {
        audio.status.store(3, Ordering::Release);
        if let Ok(mut pending) = audio.pending.lock() { pending.pending = None; }
        tracing::warn!("Terminal audio unavailable: no output device");
        return;
    };
    let Ok(sink) = Sink::try_new(&handle) else {
        audio.status.store(3, Ordering::Release);
        return;
    };
    audio.status.store(2, Ordering::Release);
    let mut active_whale = false;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        if !ENABLED.load(Ordering::Acquire) { sink.clear(); continue; }
        let cue = audio.pending.lock().ok().and_then(|mut m| m.pending.take());
        let Some((tone, at)) = cue else { continue };
        // Device initialization or a stalled worker must not replay old cues.
        if at.elapsed() > Duration::from_millis(200) { continue; }
        if !sink.empty() {
            if tone != Tone::Whale || active_whale { continue; }
            sink.clear(); // whale preempts a micro-tick, never builds a sink queue
        }
        if !ENABLED.load(Ordering::Acquire) { continue; }
        active_whale = tone == Tone::Whale;
        sink.append(SamplesBuffer::new(1, 48_000, synthesize(tone)));
    }
}

/// CoinWatch triangle ticks (1480→1800 / 820→540 Hz, 16ms) and
/// staggered 880/1320/1760Hz whale bell with exponential gain envelopes.
fn synthesize(tone: Tone) -> Vec<f32> {
    let seconds = if tone == Tone::Whale { 0.45 } else { 0.016 };
    let mut phase = 0.0_f64;
    (0..(48_000.0 * seconds) as usize).map(|i| {
        let t = i as f64 / 48_000.0;
        if tone == Tone::Whale {
            [880.0, 1320.0, 1760.0].iter().enumerate().map(|(idx, freq)| {
                let age = t - idx as f64 * 0.04;
                if age < 0.0 { return 0.0; }
                let gain = 0.08 / (idx + 1) as f64;
                let decay = (0.0001 / gain).powf(age / (0.35 + idx as f64 * 0.01));
                // Small attack/release avoids clicks at oscillator boundaries.
                (std::f64::consts::TAU * freq * age).sin() * gain * decay
                    * (age / 0.002).min(1.0) * ((0.45 - t) / 0.01).min(1.0)
            }).sum::<f64>() as f32 * 0.3
        } else {
            let (start, end): (f64, f64) = if tone == Tone::Sell { (820.0, 540.0) } else { (1480.0, 1800.0) };
            phase += start * (end / start).powf((t / 0.012).min(1.0)) / 48_000.0;
            let triangle = 4.0 * (phase.fract() - 0.5).abs() - 1.0;
            (triangle * 0.04 * (0.0001_f64 / 0.04).powf(t / 0.015)
                * (t / 0.001).min(1.0) * ((0.016 - t) / 0.001).min(1.0) * 0.3) as f32
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_rate_limit_and_whale_priority() {
        let now = Instant::now();
        let mut r = RateLimiter::default();
        assert!(r.allow(Tone::Buy, now));
        assert!(!r.allow(Tone::Sell, now + Duration::from_millis(54)));
        assert!(r.allow(Tone::Whale, now + Duration::from_millis(54)));
        assert!(!r.allow(Tone::Whale, now + Duration::from_millis(100)));
        assert!(!r.allow(Tone::Buy, now + Duration::from_millis(100)));
        assert!(r.allow(Tone::Sell, now + Duration::from_millis(110)));
        assert!(r.allow(Tone::Whale, now + Duration::from_millis(554)));
    }
    #[test]
    fn mailbox_bounded_and_whale_wins() {
        let now = Instant::now();
        let mut m = Mailbox::default();
        for _ in 0..10_000 { m.put(Tone::Buy, now); }
        m.put(Tone::Whale, now);
        m.put(Tone::Sell, now);
        assert_eq!(m.pending.take().unwrap().0, Tone::Whale);
        assert!(m.pending.is_none());
    }
    #[test]
    fn synthesized_cues_are_short_finite_and_quiet() {
        for tone in [Tone::Buy, Tone::Sell, Tone::Whale, Tone::Preview] {
            let samples = synthesize(tone);
            assert!(samples.len() <= 21_600);
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() < 0.1));
        }
    }
}
