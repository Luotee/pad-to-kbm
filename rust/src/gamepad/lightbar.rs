//! DualSense lightbar (v3.5.0): config types, frame math and the 0x31 reports.
//! The scheduler and the HID writes live in the HID worker; everything in
//! this file is pure data and arithmetic.
//!
//! # Prior art (research 2026-10-21)
//!
//! The settings shape follows what other pad tools converged on: the effect
//! is the top-level pick and carries its own parameters; rainbow is its own
//! effect with its own speed; "off" is an explicit pick; state-dependent
//! lighting means each profile / layer carries a full light setting.
//!
//! - reWASD (<https://help.rewasd.com/preferences/led-settings.html>): modes
//!   Solid glow / Shimmer / Heartbeat / Stroboscope / Breath / Rainbow, a LED
//!   setting per config and per Shift layer, plus battery low / critical LED.
//! - DSX: Static Color, Rainbow, Custom, Breathing, Strobing, Battery;
//!   rainbow speed presets Slow / Medium / Fast / Hyper (our speed names).
//! - DualSenseZ (DSZ): static, rainbow, breathing, strobe, battery gradient,
//!   off, loudness; brightness + speed; lighting follows the per-game profile;
//!   animation is capped at <= 40 reports per second (our scheduler follows).
//! - DSLightshow: Solid, Pulse (colour A <-> colour B, speed), Rainbow (speed).
//! - Logitech: Breathing (colour + rate + brightness) and Color cycle
//!   (rate + brightness) are separate effects; Razer likewise keeps Breathing
//!   (one colour) and Spectrum cycling apart.
//! - DS4Windows has a lightbar passthrough option; Steam Input overrides the
//!   DualSense light unless disabled per game. That is why the "native"
//!   (original light) mode exists: it writes nothing and leaves the light to
//!   the controller / the game.
//!
//! # Byte layout
//!
//! From Linux `drivers/hid/hid-playstation.c`
//! (<https://github.com/torvalds/linux/blob/master/drivers/hid/hid-playstation.c>),
//! `struct dualsense_output_report_common` and `dualsense_reset_leds`. In the
//! Bluetooth report 0x31 the common data starts at `r[3]`:
//! `valid_flag1` = `r[4]` bit 0x04 (LIGHTBAR_CONTROL_ENABLE);
//! `valid_flag2` = `r[41]` bit 0x02 (LIGHTBAR_SETUP_CONTROL_ENABLE);
//! `lightbar_setup` = `r[44]`, 0x02 = LIGHT_OUT; RGB = `r[47..50]`.

use anyhow::{bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Who drives the light: nobody (dark), the controller / games, or us.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightMode {
    Off,
    Native,
    Custom,
}

impl Default for LightMode {
    fn default() -> Self { LightMode::Custom }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Off,
    Fixed,
    Breathe,
    Rainbow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Slow,
    Medium,
    Fast,
    Hyper,
}

impl Speed {
    /// 0..=3 = slow, medium, fast, hyper.
    pub fn index(self) -> usize {
        match self {
            Speed::Slow => 0,
            Speed::Medium => 1,
            Speed::Fast => 2,
            Speed::Hyper => 3,
        }
    }
}

/// A colour, written in the config as the string `"#RRGGBB"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

const COLOR_ERR: &str = "lightbar color must be \"#RRGGBB\"";

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8]))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2])
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| D::Error::custom(format!("{COLOR_ERR}, got {s:?}")))
    }
}

/// What the bar does in one situation. A hand-written partial object takes
/// `StateLight::default()` for each missing field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateLight {
    pub effect: Effect,
    pub color: Rgb,
    pub speed: Speed,
    pub rainbow_breath: bool,
    pub brightness: u8,
}

impl Default for StateLight {
    fn default() -> Self {
        StateLight {
            effect: Effect::Off,
            color: Rgb([0xFF, 0xFF, 0xFF]),
            speed: Speed::Medium,
            rainbow_breath: false,
            brightness: 80,
        }
    }
}

/// `kbm` = mapping on; `other` = pad / mapping off / paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightbarConfig {
    pub mode: LightMode,
    pub kbm: StateLight,
    pub other: StateLight,
}

impl Default for LightbarConfig {
    fn default() -> Self {
        LightbarConfig {
            mode: LightMode::Custom,
            kbm: StateLight { effect: Effect::Rainbow, ..StateLight::default() },
            other: StateLight::default(),
        }
    }
}

impl LightbarConfig {
    pub fn validate(&self) -> Result<()> {
        for (name, s) in [("kbm", &self.kbm), ("other", &self.other)] {
            let b = s.brightness;
            if !(10..=100).contains(&b) || b % 10 != 0 {
                bail!("lightbar.{name}.brightness must be 10..=100 in steps of 10, got {b}");
            }
        }
        Ok(())
    }
}

/// Breathing period per speed (slow..hyper), seconds.
pub const BREATH_S: [f32; 4] = [6.0, 3.0, 1.5, 0.8];
/// Full rainbow cycle per speed (slow..hyper), seconds.
pub const CYCLE_S: [f32; 4] = [12.0, 6.0, 3.0, 1.5];

/// Raised-cosine pulse 0..1: 0 at t=0, 1 at t=P/2.
fn pulse(t: f32, p: f32) -> f32 {
    (1.0 - (std::f32::consts::TAU * t / p).cos()) / 2.0
}

/// HSV with s = v = 1; `h` in degrees, any real.
fn hue_rgb(h: f32) -> [f32; 3] {
    let h6 = h.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h6 % 2.0 - 1.0).abs();
    match h6 as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

fn to_rgb(c: [f32; 3], k: f32) -> Rgb {
    Rgb(c.map(|v| (v * k).round().clamp(0.0, 255.0) as u8))
}

/// The colour of the bar `t` seconds into the session, brightness included.
pub fn frame_at(s: &StateLight, t: f32) -> Rgb {
    let b = s.brightness as f32 / 100.0;
    let i = s.speed.index();
    let col = s.color.0.map(f32::from);
    match s.effect {
        Effect::Off => Rgb([0, 0, 0]),
        Effect::Fixed => to_rgb(col, b),
        Effect::Breathe => {
            let e = pulse(t, BREATH_S[i]);
            to_rgb(col, b * e * e)
        }
        Effect::Rainbow if s.rainbow_breath => {
            let p = BREATH_S[i];
            let n = (t / p).floor();
            let e = pulse(t, p);
            to_rgb(hue_rgb(60.0 * n).map(|v| v * 255.0), b * e * e)
        }
        Effect::Rainbow => {
            to_rgb(hue_rgb(360.0 * t / CYCLE_S[i]).map(|v| v * 255.0), b)
        }
    }
}

/// Append the DualSense BT CRC32 (seed 0xA2 over `r[..74]`, LE) to `r[74..78]`.
pub(crate) fn seal(r: &mut [u8; 78]) {
    let mut h = crc32fast::Hasher::new();
    h.update(&[0xA2]);
    h.update(&r[..74]);
    r[74..78].copy_from_slice(&h.finalize().to_le_bytes());
}

fn ds_base() -> [u8; 78] {
    let mut r = [0u8; 78];
    r[0] = 0x31;
    r[2] = 0x10;
    r
}

/// Report 0x31 that sets only the lightbar colour; motors, audio and
/// triggers are left alone (their valid flags stay 0).
pub fn ds_lightbar_report(c: Rgb) -> [u8; 78] {
    let mut r = ds_base();
    r[4] = 0x04;
    r[47..50].copy_from_slice(&c.0);
    seal(&mut r);
    r
}

/// Report 0x31 that releases the lightbar to us (LIGHT_OUT), sent once per
/// session before colour reports.
pub fn ds_lightbar_setup_report() -> [u8; 78] {
    let mut r = ds_base();
    r[41] = 0x02;
    r[44] = 0x02;
    seal(&mut r);
    r
}

/// Lightbar settings shared between the GUI / config side and the HID worker.
/// Clones share the same state.
#[derive(Clone, Default)]
pub struct LightbarLive {
    cfg: Arc<Mutex<LightbarConfig>>,
    kbm: Arc<AtomicBool>,
}

impl LightbarLive {
    pub fn set_config(&self, c: LightbarConfig) {
        *self.cfg.lock().unwrap_or_else(|e| e.into_inner()) = c;
    }

    pub fn set_kbm(&self, on: bool) {
        self.kbm.store(on, Ordering::Relaxed);
    }

    pub fn kbm(&self) -> bool {
        self.kbm.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> (LightbarConfig, bool) {
        (*self.cfg.lock().unwrap_or_else(|e| e.into_inner()), self.kbm())
    }
}

/// One lightbar write the worker should make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightWrite {
    Setup,
    Color(Rgb),
}

/// Animation write interval.
const ANIM_EVERY: Duration = Duration::from_millis(50);
/// Re-send interval for still frames (the pad / other software may overwrite).
const STILL_EVERY: Duration = Duration::from_secs(2);
/// After a failed write, no retry before this long ...
const RETRY_AFTER: Duration = Duration::from_millis(50);
/// ... and after `SLOW_AFTER_FAILS` failures in a row, one try per this long.
/// hidapi's Windows write can block for up to a second, on the read thread.
const SLOW_RETRY_AFTER: Duration = Duration::from_secs(2);
const SLOW_AFTER_FAILS: u32 = 3;
/// LCM of every animation period (incl. 6 rainbow-breath steps), seconds.
const WRAP_S: u64 = 72;

/// Seconds since `start`, wrapped at `WRAP_S` from the integer `Duration`
/// (never accumulated in f32) so long sessions keep full precision.
fn wrapped_secs(start: Instant, now: Instant) -> f32 {
    let d = now.saturating_duration_since(start);
    ((d.as_secs() % WRAP_S) as f64 + f64::from(d.subsec_nanos()) / 1e9) as f32
}

const BLACK: Rgb = Rgb([0, 0, 0]);

#[derive(Clone, Copy)]
struct Memory {
    setup_done: bool,
    last: Option<(Rgb, Instant)>,
}

/// Decides when the HID worker writes the lightbar.
pub struct LightbarPlayer {
    start: Instant,
    mem: Memory,
    /// State before the most recent output, restored by `on_write_failed`.
    undo: Memory,
    /// When the most recent output was handed out, while it may still fail.
    pending: Option<Instant>,
    /// Failed writes in a row.
    fails: u32,
    /// No output before this (backoff after a failed write).
    hold_until: Option<Instant>,
}

impl LightbarPlayer {
    pub fn new(start: Instant) -> Self {
        let mem = Memory { setup_done: false, last: None };
        LightbarPlayer { start, mem, undo: mem, pending: None, fails: 0, hold_until: None }
    }

    pub fn take_due(&mut self, now: Instant, cfg: &LightbarConfig, kbm: bool) -> Option<LightWrite> {
        // The previous output was not reported failed: it was written.
        if self.pending.take().is_some() {
            self.fails = 0;
            self.hold_until = None;
        }
        if cfg.mode == LightMode::Native {
            return None;
        }
        if self.hold_until.is_some_and(|h| now < h) {
            return None;
        }
        if !self.mem.setup_done {
            self.undo = self.mem;
            self.mem.setup_done = true;
            self.pending = Some(now);
            return Some(LightWrite::Setup);
        }
        let (color, animated) = if cfg.mode == LightMode::Off {
            (BLACK, false)
        } else {
            let s = if kbm { &cfg.kbm } else { &cfg.other };
            let t = wrapped_secs(self.start, now);
            (frame_at(s, t), matches!(s.effect, Effect::Breathe | Effect::Rainbow))
        };
        let due = match self.mem.last {
            None => true,
            Some((c, at)) => {
                let age = now.saturating_duration_since(at);
                if animated { age >= ANIM_EVERY } else { c != color || age >= STILL_EVERY }
            }
        };
        if !due {
            return None;
        }
        self.undo = self.mem;
        self.mem.last = Some((color, now));
        self.pending = Some(now);
        Some(LightWrite::Color(color))
    }

    /// The last output was not written: forget it so it is offered again,
    /// after `RETRY_AFTER`, or `SLOW_RETRY_AFTER` once `SLOW_AFTER_FAILS`
    /// writes in a row have failed.
    pub fn on_write_failed(&mut self) {
        let Some(at) = self.pending.take() else { return };
        self.mem = self.undo;
        self.fails += 1;
        let wait = if self.fails >= SLOW_AFTER_FAILS { SLOW_RETRY_AFTER } else { RETRY_AFTER };
        self.hold_until = Some(at + wait);
    }

    /// The still frame to leave on the bar when the app exits.
    pub fn exit_frame(&self, now: Instant, cfg: &LightbarConfig) -> Option<Rgb> {
        match cfg.mode {
            LightMode::Native => None,
            LightMode::Off => Some(BLACK),
            LightMode::Custom => Some(match cfg.other.effect {
                Effect::Rainbow => frame_at(&cfg.other, wrapped_secs(self.start, now)),
                Effect::Off => BLACK,
                Effect::Fixed | Effect::Breathe => {
                    frame_at(&StateLight { effect: Effect::Fixed, ..cfg.other }, 0.0)
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_matches_spec() {
        let d = LightbarConfig::default();
        assert_eq!(d.mode, LightMode::Custom);
        assert_eq!(d.kbm.effect, Effect::Rainbow);
        assert_eq!(d.other.effect, Effect::Off);
        for s in [d.kbm, d.other] {
            assert_eq!(s.color, Rgb([0xFF, 0xFF, 0xFF]));
            assert_eq!(s.speed, Speed::Medium);
            assert_eq!(s.brightness, 80);
            assert!(!s.rainbow_breath);
        }
    }

    #[test]
    fn round_trips_json() {
        let d = LightbarConfig::default();
        let v = serde_json::to_value(d).unwrap();
        assert_eq!(
            v,
            json!({"mode":"custom",
                   "kbm":{"effect":"rainbow","color":"#FFFFFF","speed":"medium","rainbow_breath":false,"brightness":80},
                   "other":{"effect":"off","color":"#FFFFFF","speed":"medium","rainbow_breath":false,"brightness":80}})
        );
        let back: LightbarConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn color_parses_lowercase_and_writes_upper() {
        let c: Rgb = serde_json::from_value(json!("#2ea8ff")).unwrap();
        assert_eq!(c, Rgb([0x2E, 0xA8, 0xFF]));
        assert_eq!(serde_json::to_value(c).unwrap(), json!("#2EA8FF"));
    }

    #[test]
    fn bad_color_is_rejected() {
        for bad in ["red", "#12345", "#GGGGGG"] {
            let err = serde_json::from_value::<Rgb>(json!(bad)).unwrap_err().to_string();
            assert!(err.contains("#RRGGBB"), "{bad}: {err}");
        }
    }

    #[test]
    fn partial_state_takes_field_defaults() {
        let c: LightbarConfig =
            serde_json::from_value(json!({"kbm":{"effect":"fixed","color":"#FF0000"}})).unwrap();
        assert_eq!(c.mode, LightMode::Custom);
        assert_eq!(c.kbm.effect, Effect::Fixed);
        assert_eq!(c.kbm.color, Rgb([0xFF, 0, 0]));
        assert_eq!(c.kbm.speed, Speed::Medium);
        assert_eq!(c.kbm.brightness, 80);
        assert_eq!(c.other, LightbarConfig::default().other);
    }

    #[test]
    fn bad_brightness_is_rejected() {
        for b in [85u8, 0, 110] {
            let mut c = LightbarConfig::default();
            c.kbm.brightness = b;
            let err = c.validate().unwrap_err().to_string();
            assert!(err.contains("lightbar.kbm.brightness"), "{b}: {err}");
        }
        let mut c = LightbarConfig::default();
        c.other.brightness = 85;
        assert!(c.validate().unwrap_err().to_string().contains("lightbar.other.brightness"));
        for b in [10u8, 100] {
            let mut c = LightbarConfig::default();
            c.kbm.brightness = b;
            c.validate().unwrap();
        }
    }

    #[test]
    fn unknown_speed_is_rejected() {
        assert!(serde_json::from_value::<StateLight>(json!({"speed":"turbo"})).is_err());
        assert!(serde_json::from_value::<StateLight>(json!({"speed":5})).is_err());
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    fn st(effect: Effect, color: [u8; 3], speed: Speed, rb: bool, brightness: u8) -> StateLight {
        StateLight { effect, color: Rgb(color), speed, rainbow_breath: rb, brightness }
    }

    #[test]
    fn fixed_scales_by_brightness() {
        let s = st(Effect::Fixed, [0xFF, 0x80, 0], Speed::Medium, false, 50);
        assert_eq!(frame_at(&s, 0.0), Rgb([128, 64, 0]));
    }

    #[test]
    fn off_is_black() {
        let s = st(Effect::Off, [255, 255, 255], Speed::Medium, false, 100);
        for t in [0.0, 1.3, 99.0] {
            assert_eq!(frame_at(&s, t), Rgb([0, 0, 0]));
        }
    }

    #[test]
    fn breathe_trough_peak() {
        let s = st(Effect::Breathe, [255, 255, 255], Speed::Medium, false, 100);
        assert_eq!(frame_at(&s, 0.0), Rgb([0, 0, 0]));
        assert_eq!(frame_at(&s, 1.5), Rgb([255, 255, 255]));
        assert_eq!(frame_at(&s, 0.75), Rgb([64, 64, 64]));
    }

    #[test]
    fn rainbow_hue_walks() {
        let s = st(Effect::Rainbow, [0, 0, 0], Speed::Medium, false, 100);
        assert_eq!(frame_at(&s, 0.0), Rgb([255, 0, 0]));
        assert_eq!(frame_at(&s, 1.0), Rgb([255, 255, 0]));
        assert_eq!(frame_at(&s, 2.0), Rgb([0, 255, 0]));
        assert_eq!(frame_at(&s, 4.0), Rgb([0, 0, 255]));
    }

    #[test]
    fn rainbow_breath_steps_60_per_breath() {
        let s = st(Effect::Rainbow, [0, 0, 0], Speed::Medium, true, 100);
        assert_eq!(frame_at(&s, 1.5), Rgb([255, 0, 0]));
        assert_eq!(frame_at(&s, 4.5), Rgb([255, 255, 0]));
        assert_eq!(frame_at(&s, 19.5), Rgb([255, 0, 0]));
    }

    #[test]
    fn speed_indexes_periods() {
        let s = st(Effect::Breathe, [255, 255, 255], Speed::Hyper, false, 100);
        assert_eq!(frame_at(&s, 0.4), Rgb([255, 255, 255]));
        assert_eq!(frame_at(&s, 0.0), Rgb([0, 0, 0]));
    }

    fn crc_ok(r: &[u8; 78]) -> bool {
        let mut h = crc32fast::Hasher::new();
        h.update(&[0xA2]);
        h.update(&r[..74]);
        r[74..78] == h.finalize().to_le_bytes()
    }

    #[test]
    fn lightbar_report_layout() {
        let r = ds_lightbar_report(Rgb([1, 2, 3]));
        assert_eq!(
            [r[0], r[2], r[4], r[47], r[48], r[49]],
            [0x31, 0x10, 0x04, 1, 2, 3]
        );
    }

    #[test]
    fn lightbar_report_leaves_motors_alone() {
        let r = ds_lightbar_report(Rgb([9, 9, 9]));
        assert!(r[3] == 0 && r[5] == 0 && r[6] == 0 && r[41] == 0);
    }

    #[test]
    fn setup_report_layout() {
        let r = ds_lightbar_setup_report();
        assert!(r[41] == 0x02 && r[44] == 0x02 && r[4] == 0);
        assert_eq!([r[0], r[2]], [0x31, 0x10]);
    }

    #[test]
    fn reports_carry_valid_crc() {
        assert!(crc_ok(&ds_lightbar_report(Rgb([200, 100, 50]))));
        assert!(crc_ok(&ds_lightbar_setup_report()));
    }
}

#[cfg(test)]
mod player_tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn fixed(c: [u8; 3]) -> StateLight {
        StateLight { effect: Effect::Fixed, color: Rgb(c), brightness: 100, ..StateLight::default() }
    }

    fn cfg(kbm: StateLight, other: StateLight) -> LightbarConfig {
        LightbarConfig { mode: LightMode::Custom, kbm, other }
    }

    fn colors(
        p: &mut LightbarPlayer,
        t0: Instant,
        c: &LightbarConfig,
        kbm: bool,
        range: std::ops::Range<u64>,
    ) -> Vec<(u64, Rgb)> {
        range
            .filter_map(|n| match p.take_due(t0 + ms(n), c, kbm) {
                Some(LightWrite::Color(x)) => Some((n, x)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn native_never_writes() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let mut c = LightbarConfig::default();
        c.mode = LightMode::Native;
        for n in 0..1000 {
            assert_eq!(p.take_due(t0 + ms(n * 10), &c, n % 2 == 0), None);
        }
        assert_eq!(p.exit_frame(t0 + ms(5), &c), None);
    }

    #[test]
    fn setup_comes_first_once() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = LightbarConfig::default();
        assert_eq!(p.take_due(t0, &c, true), Some(LightWrite::Setup));
        for n in 1..500 {
            assert_ne!(p.take_due(t0 + ms(n), &c, true), Some(LightWrite::Setup));
        }
    }

    #[test]
    fn animated_writes_every_50ms() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = LightbarConfig::default(); // kbm = rainbow
        let w = colors(&mut p, t0, &c, true, 0..1000);
        assert!((19..=21).contains(&w.len()), "{}", w.len());
    }

    #[test]
    fn static_writes_on_change_and_every_2s() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let mut c = cfg(fixed([255, 255, 255]), StateLight::default());
        let w = colors(&mut p, t0, &c, true, 0..5000);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w.iter().all(|(_, x)| *x == Rgb([255, 255, 255])));
        c.kbm.color = Rgb([255, 0, 0]);
        assert_eq!(
            p.take_due(t0 + ms(5000), &c, true),
            Some(LightWrite::Color(Rgb([255, 0, 0])))
        );
        assert_eq!(p.take_due(t0 + ms(5001), &c, true), None);
    }

    #[test]
    fn kbm_flag_picks_state() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = cfg(fixed([255, 0, 0]), fixed([0, 0, 255]));
        assert_eq!(p.take_due(t0, &c, true), Some(LightWrite::Setup));
        assert_eq!(p.take_due(t0 + ms(1), &c, true), Some(LightWrite::Color(Rgb([255, 0, 0]))));
        assert_eq!(p.take_due(t0 + ms(2), &c, false), Some(LightWrite::Color(Rgb([0, 0, 255]))));
    }

    #[test]
    fn mode_off_is_black_for_both() {
        let t0 = Instant::now();
        let mut c = LightbarConfig::default();
        c.mode = LightMode::Off;
        for kbm in [true, false] {
            let mut p = LightbarPlayer::new(t0);
            assert_eq!(p.take_due(t0, &c, kbm), Some(LightWrite::Setup));
            assert_eq!(p.take_due(t0 + ms(1), &c, kbm), Some(LightWrite::Color(Rgb([0, 0, 0]))));
        }
    }

    #[test]
    fn failed_setup_is_retried() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = LightbarConfig::default();
        assert_eq!(p.take_due(t0, &c, true), Some(LightWrite::Setup));
        p.on_write_failed();
        assert_eq!(p.take_due(t0 + ms(50), &c, true), Some(LightWrite::Setup));
    }

    #[test]
    fn failed_color_is_retried() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = cfg(fixed([1, 2, 3]), StateLight::default());
        p.take_due(t0, &c, true);
        assert!(p.take_due(t0 + ms(1), &c, true).is_some());
        p.on_write_failed();
        assert_eq!(p.take_due(t0 + ms(51), &c, true), Some(LightWrite::Color(Rgb([1, 2, 3]))));
    }

    #[test]
    fn failed_write_waits_50ms_before_retry() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = cfg(fixed([1, 2, 3]), StateLight::default());
        assert_eq!(p.take_due(t0, &c, true), Some(LightWrite::Setup));
        p.on_write_failed();
        for n in 1..50 {
            assert_eq!(p.take_due(t0 + ms(n), &c, true), None, "setup retried at {n} ms");
        }
        assert_eq!(p.take_due(t0 + ms(50), &c, true), Some(LightWrite::Setup));
        assert_eq!(p.take_due(t0 + ms(51), &c, true), Some(LightWrite::Color(Rgb([1, 2, 3]))));
        p.on_write_failed();
        for n in 52..101 {
            assert_eq!(p.take_due(t0 + ms(n), &c, true), None, "colour retried at {n} ms");
        }
        assert_eq!(p.take_due(t0 + ms(101), &c, true), Some(LightWrite::Color(Rgb([1, 2, 3]))));
    }

    /// Every write fails for 10 s, polled every 1 ms: 50 ms gaps for the first
    /// failures, then one try per 2 s.
    #[test]
    fn three_failures_slow_retries_to_2s() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = LightbarConfig::default(); // kbm = rainbow, animated
        let mut tries = Vec::new();
        for n in 0..10_000 {
            if p.take_due(t0 + ms(n), &c, true).is_some() {
                tries.push(n);
                p.on_write_failed();
            }
        }
        assert!(tries.len() <= 3 + 10 / 2, "{tries:?}");
        assert!(tries.len() >= 6, "{tries:?}");
        for (i, w) in tries.windows(2).enumerate() {
            let min = if i < 2 { 50 } else { 2000 };
            assert!(w[1] - w[0] >= min, "gap {} after try {} in {tries:?}", w[1] - w[0], i + 1);
        }
    }

    #[test]
    fn success_resets_backoff() {
        let t0 = Instant::now();
        let mut p = LightbarPlayer::new(t0);
        let c = cfg(fixed([1, 2, 3]), StateLight::default());
        // Three failed setups: the next try waits 2 s.
        for n in [0, 50, 100] {
            assert_eq!(p.take_due(t0 + ms(n), &c, true), Some(LightWrite::Setup), "{n}");
            p.on_write_failed();
        }
        assert_eq!(p.take_due(t0 + ms(2099), &c, true), None);
        // This setup goes through (no on_write_failed before the next take_due).
        assert_eq!(p.take_due(t0 + ms(2100), &c, true), Some(LightWrite::Setup));
        assert_eq!(p.take_due(t0 + ms(2101), &c, true), Some(LightWrite::Color(Rgb([1, 2, 3]))));
        p.on_write_failed();
        // One failure after a success: back to the 50 ms wait, not 2 s.
        assert_eq!(p.take_due(t0 + ms(2150), &c, true), None);
        assert_eq!(p.take_due(t0 + ms(2151), &c, true), Some(LightWrite::Color(Rgb([1, 2, 3]))));
    }

    #[test]
    fn exit_frame_uses_other_state() {
        let t0 = Instant::now();
        let p = LightbarPlayer::new(t0);
        let breathe = StateLight {
            effect: Effect::Breathe,
            color: Rgb([255, 0, 0]),
            brightness: 50,
            ..StateLight::default()
        };
        let mut c = cfg(StateLight::default(), breathe);
        assert_eq!(p.exit_frame(t0 + ms(50), &c), Some(Rgb([128, 0, 0])));
        c.other = StateLight::default(); // effect off
        assert_eq!(p.exit_frame(t0 + ms(50), &c), Some(Rgb([0, 0, 0])));
        let mut c = LightbarConfig::default();
        c.mode = LightMode::Off;
        assert_eq!(p.exit_frame(t0 + ms(50), &c), Some(Rgb([0, 0, 0])));
    }

    #[test]
    fn time_wraps_at_72s_without_seam() {
        let t0 = Instant::now();
        let p = LightbarPlayer::new(t0);
        let other = StateLight { effect: Effect::Rainbow, brightness: 100, ..StateLight::default() };
        let c = cfg(StateLight::default(), other);
        let a = p.exit_frame(t0 + Duration::from_secs(1), &c);
        let b = p.exit_frame(t0 + Duration::from_secs(73), &c);
        assert_eq!(a, b);
        assert_eq!(a, Some(frame_at(&other, 1.0)));
    }

    #[test]
    fn live_snapshot_reflects_setters() {
        let live = LightbarLive::default();
        assert_eq!(live.snapshot(), (LightbarConfig::default(), false));
        let other = live.clone();
        let mut c = LightbarConfig::default();
        c.mode = LightMode::Off;
        other.set_config(c);
        other.set_kbm(true);
        assert_eq!(live.snapshot(), (c, true));
        assert!(live.kbm());
    }
}
