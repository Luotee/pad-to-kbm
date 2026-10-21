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
