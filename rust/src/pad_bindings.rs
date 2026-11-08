//! Per-controller binding tables (config version 2).
//!
//! Each controller kind has its own `PadBindings`: the base table (every id
//! `0..=32`, iron rule 1), two quick-slot pages that a held trigger in page
//! mode swaps in for the D-pad and face buttons, and the two trigger modes.

use crate::config::{Binding, ButtonEntry, MouseButton};
use crate::gamepad::ControllerKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which of a pad's three tables a binding lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Base,
    QuickL,
    QuickR,
}

/// A trigger side: L = ZL / L2 (id 23), R = ZR / R2 (id 24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    L,
    R,
}

impl Side {
    /// The page this side's trigger selects in page mode.
    pub fn page(self) -> Page {
        match self {
            Side::L => Page::QuickL,
            Side::R => Page::QuickR,
        }
    }
}

/// What a trigger does: select its quick page, or act as one ordinary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Page,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerModes {
    pub l: TriggerMode,
    pub r: TriggerMode,
}

impl Default for TriggerModes {
    fn default() -> Self {
        TriggerModes { l: TriggerMode::Page, r: TriggerMode::Page }
    }
}

impl TriggerModes {
    pub fn get(&self, side: Side) -> TriggerMode {
        match side {
            Side::L => self.l,
            Side::R => self.r,
        }
    }

    pub fn set(&mut self, side: Side, mode: TriggerMode) {
        match side {
            Side::L => self.l = mode,
            Side::R => self.r = mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PadBindings {
    pub buttons: BTreeMap<String, ButtonEntry>,
    #[serde(default)]
    pub quick_l: BTreeMap<String, ButtonEntry>,
    #[serde(default)]
    pub quick_r: BTreeMap<String, ButtonEntry>,
    #[serde(default)]
    pub trigger_mode: TriggerModes,
}

/// The ids a quick page may hold: face buttons 0..=3 and D-pad 11..=14.
pub const QUICK_IDS: [u32; 8] = [0, 1, 2, 3, 11, 12, 13, 14];

/// The label of every button id, as shipped in `config.example.json`.
pub const LABELS: [&str; 33] = [
    "Cross（叉叉）",
    "Circle（圈圈）",
    "Square（正方形）",
    "Triangle（三角）",
    "Share（拍照鍵）",
    "PS 按鈕",
    "Options（Menu 鍵）",
    "L3",
    "R3",
    "L1",
    "R1",
    "D-pad ↑",
    "D-pad ↓",
    "D-pad ←",
    "D-pad →",
    "L-stick ↑",
    "L-stick ↓",
    "L-stick ←",
    "L-stick →",
    "R-stick ↑",
    "R-stick ↓",
    "R-stick ←",
    "R-stick →",
    "L2（類比板機）",
    "R2（類比板機）",
    "Touchpad TL",
    "Touchpad TR",
    "Touchpad BL",
    "Touchpad BR",
    "Capture（Switch 擷取鍵）",
    "GL（Switch 2 背鍵左）",
    "GR（Switch 2 背鍵右）",
    "C（Switch 2 C 鍵）",
];

/// The Xbox Nano's own names for the ids whose shared label names a
/// DualSense control (its other ids keep `LABELS`).
const NANO_LABELS: [(u32, &str); 14] = [
    (0, "A"),
    (1, "B"),
    (2, "X"),
    (3, "Y"),
    (4, "View（檢視鍵）"),
    (5, "Xbox 按鈕"),
    (6, "Menu（選單鍵）"),
    (7, "LS（L3）"),
    (8, "RS（R3）"),
    (9, "LB"),
    (10, "RB"),
    (23, "LT（類比板機）"),
    (24, "RT（類比板機）"),
    (29, "Share（分享鍵）"),
];

/// The label of button `id` in `kind`'s table, as shipped in
/// `config.example.json`.
pub fn label(kind: ControllerKind, id: u32) -> &'static str {
    let own = match kind {
        ControllerKind::XboxNano => NANO_LABELS.iter().find(|(i, _)| *i == id).map(|(_, l)| *l),
        _ => None,
    };
    own.unwrap_or(LABELS[id as usize])
}

fn entry(kind: ControllerKind, id: u32, binding: Binding) -> ButtonEntry {
    ButtonEntry { label: label(kind, id).to_string(), binding }
}

/// The default table (spec 4.8), used by first run, `reset_pad` and the
/// fill-in of a missing pad: D-pad and left stick = arrow keys, DualSense
/// touchpad quadrants = mouse left, everything else unbound, both triggers
/// in page mode with empty pages.
pub fn default_pad_bindings(kind: ControllerKind) -> PadBindings {
    let mut buttons = BTreeMap::new();
    for id in crate::config::VALID_BUTTON_IDS {
        let binding = match id {
            11 | 15 => Binding::Key("Up".into()),
            12 | 16 => Binding::Key("Down".into()),
            13 | 17 => Binding::Key("Left".into()),
            14 | 18 => Binding::Key("Right".into()),
            25..=28 if kind == ControllerKind::DualSense => Binding::Mouse(MouseButton::Left),
            _ => Binding::Unbound,
        };
        buttons.insert(id.to_string(), entry(kind, id, binding));
    }
    PadBindings {
        buttons,
        quick_l: BTreeMap::new(),
        quick_r: BTreeMap::new(),
        trigger_mode: TriggerModes::default(),
    }
}

impl PadBindings {
    pub fn table(&self, page: Page) -> &BTreeMap<String, ButtonEntry> {
        match page {
            Page::Base => &self.buttons,
            Page::QuickL => &self.quick_l,
            Page::QuickR => &self.quick_r,
        }
    }

    pub fn table_mut(&mut self, page: Page) -> &mut BTreeMap<String, ButtonEntry> {
        match page {
            Page::Base => &mut self.buttons,
            Page::QuickL => &mut self.quick_l,
            Page::QuickR => &mut self.quick_r,
        }
    }

    /// Every base id unbound (labels kept), both pages empty, trigger modes
    /// kept: what 全部清除 writes.
    pub fn cleared(&self) -> PadBindings {
        let mut buttons = BTreeMap::new();
        for id in crate::config::VALID_BUTTON_IDS {
            let key = id.to_string();
            let label = self
                .buttons
                .get(&key)
                .map(|e| e.label.clone())
                .unwrap_or_else(|| LABELS[id as usize].to_string());
            buttons.insert(key, ButtonEntry { label, binding: Binding::Unbound });
        }
        PadBindings {
            buttons,
            quick_l: BTreeMap::new(),
            quick_r: BTreeMap::new(),
            trigger_mode: self.trigger_mode,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Binding, MouseButton};
    use crate::gamepad::ControllerKind;

    #[test]
    fn default_tables_match_spec_4_8() {
        let ds = default_pad_bindings(ControllerKind::DualSense);
        for (id, k) in [(11, "Up"), (12, "Down"), (13, "Left"), (14, "Right"), (15, "Up"), (16, "Down"), (17, "Left"), (18, "Right")] {
            assert!(matches!(ds.buttons[&id.to_string()].binding, Binding::Key(ref s) if s == k), "id {id}");
        }
        for id in 25..=28 {
            assert_eq!(ds.buttons[&id.to_string()].binding, Binding::Mouse(MouseButton::Left));
        }
        let ns = default_pad_bindings(ControllerKind::NsPro);
        for id in 25..=28 {
            assert_eq!(ns.buttons[&id.to_string()].binding, Binding::Unbound);
        }
        assert!(ds.quick_l.is_empty() && ds.quick_r.is_empty());
        assert_eq!(ds.trigger_mode, TriggerModes { l: TriggerMode::Page, r: TriggerMode::Page });
        assert_eq!(ds.buttons.len(), 33);
    }

    #[test]
    fn default_tables_leave_everything_else_unbound() {
        for k in ControllerKind::ALL {
            let p = default_pad_bindings(k);
            for (id, e) in &p.buttons {
                let id: u32 = id.parse().unwrap();
                let bound = (11..=18).contains(&id)
                    || (k == ControllerKind::DualSense && (25..=28).contains(&id));
                if !bound {
                    assert_eq!(e.binding, Binding::Unbound, "{k:?} id {id}");
                }
                assert_eq!(e.label, label(k, id));
                if k != ControllerKind::XboxNano {
                    assert_eq!(e.label, LABELS[id as usize]);
                }
            }
        }
    }

    #[test]
    fn cleared_keeps_trigger_mode_and_labels() {
        let mut p = default_pad_bindings(ControllerKind::DualSense);
        p.trigger_mode.l = TriggerMode::Key;
        p.quick_l.insert("11".into(), crate::config::ButtonEntry { label: "x".into(), binding: Binding::Key("F1".into()) });
        let c = p.cleared();
        assert_eq!(c.trigger_mode.l, TriggerMode::Key);
        assert!(c.quick_l.is_empty() && c.quick_r.is_empty());
        assert_eq!(c.buttons.len(), 33);
        assert!(c.buttons.values().all(|e| e.binding == Binding::Unbound));
        assert_eq!(c.buttons["0"].label, LABELS[0]);
    }

    #[test]
    fn serde_names() {
        assert_eq!(serde_json::to_string(&Page::QuickL).unwrap(), "\"quick_l\"");
        assert_eq!(serde_json::to_string(&Page::Base).unwrap(), "\"base\"");
        assert_eq!(serde_json::to_string(&Side::R).unwrap(), "\"r\"");
        assert_eq!(serde_json::to_string(&TriggerMode::Key).unwrap(), "\"key\"");
        let p: PadBindings = serde_json::from_value(serde_json::json!({"buttons": {}})).unwrap();
        assert_eq!(p.trigger_mode, TriggerModes::default());
        assert_eq!(TriggerModes::default().l, TriggerMode::Page);
    }

    #[test]
    fn table_selects_page() {
        let mut p = default_pad_bindings(ControllerKind::NsPro);
        p.table_mut(Page::QuickR).insert("0".into(), crate::config::ButtonEntry { label: "a".into(), binding: Binding::Key("F5".into()) });
        assert_eq!(p.table(Page::QuickR).len(), 1);
        assert!(p.table(Page::QuickL).is_empty());
        assert_eq!(p.table(Page::Base).len(), 33);
    }
}
