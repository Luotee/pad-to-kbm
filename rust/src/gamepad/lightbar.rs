//! DualSense lightbar settings (v3.5.0): the config types and their serde.
//! Frame math, the scheduler and the HID writes live further down this file
//! / in the HID worker; this part is data only.

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
