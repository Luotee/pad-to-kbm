//! Controller kinds the mapper can drive. Standalone: no dependency on
//! `config` so the gamepad layer stays self-contained.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ControllerKind {
    #[serde(rename = "dualsense")]
    DualSense,
    #[serde(rename = "ns_pro")]
    NsPro,
    #[serde(rename = "ns2_pro")]
    Ns2Pro,
    #[serde(rename = "xbox_nano")]
    XboxNano,
}

/// Button ids 0..=28 (the full DualSense surface).
const DUALSENSE_CAPS: [u32; 29] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28,
];

/// Switch Pro: 0..=24 (no touchpad quadrants 25..=28) plus 29 (Capture).
const NS_PRO_CAPS: [u32; 26] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 29,
];

/// Switch 2 Pro: 0..=24 plus 29 (Capture) and 30..=32 (GL / GR / C).
const NS2_PRO_CAPS: [u32; 29] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 29,
    30, 31, 32,
];

/// Xbox Nano: 0..=24 plus 29 (Share: listed, but never sent to the PC, so
/// the board greys it out). Same set as the
/// Switch Pro; the ids keep the DualSense numbering.
const XBOX_NANO_CAPS: [u32; 26] = NS_PRO_CAPS;

impl ControllerKind {
    pub const ALL: [ControllerKind; 4] = [
        ControllerKind::DualSense,
        ControllerKind::NsPro,
        ControllerKind::Ns2Pro,
        ControllerKind::XboxNano,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            ControllerKind::DualSense => "dualsense",
            ControllerKind::NsPro => "ns_pro",
            ControllerKind::Ns2Pro => "ns2_pro",
            ControllerKind::XboxNano => "xbox_nano",
        }
    }

    pub fn from_slug(s: &str) -> Option<ControllerKind> {
        Self::ALL.into_iter().find(|k| k.slug() == s)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ControllerKind::DualSense => "DualSense",
            ControllerKind::NsPro => "Switch Pro Controller",
            ControllerKind::Ns2Pro => "Switch 2 Pro Controller",
            ControllerKind::XboxNano => "Xbox Nano",
        }
    }

    pub fn capabilities(self) -> &'static [u32] {
        match self {
            ControllerKind::DualSense => &DUALSENSE_CAPS,
            ControllerKind::NsPro => &NS_PRO_CAPS,
            ControllerKind::Ns2Pro => &NS2_PRO_CAPS,
            ControllerKind::XboxNano => &XBOX_NANO_CAPS,
        }
    }

    /// Windows has a driver games understand for this pad, so it can be left
    /// to the game (`OutputMode::Pad`). The Switch 2 Pro has none: its only
    /// non-keyboard output is the virtual Xbox pad.
    pub fn has_native_driver(self) -> bool {
        !matches!(self, ControllerKind::Ns2Pro)
    }

    /// The pad's only non-keyboard output is the virtual Xbox pad (no driver
    /// games understand): the opposite of `has_native_driver`.
    pub fn needs_virtual_pad(self) -> bool {
        !self.has_native_driver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_roundtrip() {
        for k in ControllerKind::ALL {
            assert_eq!(ControllerKind::from_slug(k.slug()), Some(k));
        }
        assert_eq!(ControllerKind::from_slug("ps5"), None);
    }

    #[test]
    fn only_ns2_lacks_a_native_driver() {
        assert!(ControllerKind::DualSense.has_native_driver());
        assert!(ControllerKind::NsPro.has_native_driver());
        assert!(!ControllerKind::Ns2Pro.has_native_driver());
    }

    #[test]
    fn ns_pro_capabilities() {
        let mut want: Vec<u32> = (0..=24).collect();
        want.push(29);
        assert_eq!(ControllerKind::NsPro.capabilities(), want);
        assert_eq!(ControllerKind::NsPro.capabilities().len(), 26);
    }

    #[test]
    fn ns2_capabilities() {
        let mut want: Vec<u32> = (0..=24).collect();
        want.extend(29..=32);
        assert_eq!(ControllerKind::Ns2Pro.capabilities(), want);
        assert_eq!(ControllerKind::Ns2Pro.capabilities().len(), 29);
    }

    #[test]
    fn dualsense_capabilities_are_0_to_28() {
        assert_eq!(
            ControllerKind::DualSense.capabilities(),
            (0..=28).collect::<Vec<u32>>()
        );
    }

    #[test]
    fn deserializes_from_slug() {
        let k: ControllerKind = serde_json::from_str("\"ns2_pro\"").unwrap();
        assert_eq!(k, ControllerKind::Ns2Pro);
        assert!(serde_json::from_str::<ControllerKind>("\"ps5\"").is_err());
    }

    #[test]
    fn serializes_as_slug() {
        assert_eq!(
            serde_json::to_string(&ControllerKind::NsPro).unwrap(),
            "\"ns_pro\""
        );
    }

    #[test]
    fn xbox_nano_slug_roundtrip() {
        assert_eq!(ControllerKind::XboxNano.slug(), "xbox_nano");
        assert_eq!(ControllerKind::from_slug("xbox_nano"), Some(ControllerKind::XboxNano));
        assert_eq!(ControllerKind::XboxNano.display_name(), "Xbox Nano");
        let k: ControllerKind = serde_json::from_str("\"xbox_nano\"").unwrap();
        assert_eq!(k, ControllerKind::XboxNano);
    }

    #[test]
    fn xbox_nano_capabilities() {
        let want: Vec<u32> = (0..=24).chain([29]).collect();
        assert_eq!(ControllerKind::XboxNano.capabilities(), want);
    }

    #[test]
    fn xbox_nano_has_native_driver_and_no_virtual_pad() {
        assert!(ControllerKind::XboxNano.has_native_driver());
        assert!(!ControllerKind::XboxNano.needs_virtual_pad());
        assert!(ControllerKind::Ns2Pro.needs_virtual_pad());
        assert!(!ControllerKind::DualSense.needs_virtual_pad());
        for k in ControllerKind::ALL {
            assert_eq!(k.needs_virtual_pad(), !k.has_native_driver(), "{k:?}");
        }
    }
}
