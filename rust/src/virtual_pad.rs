//! Platform-neutral mirror of a virtual Xbox 360 pad's state.
//!
//! In Xbox output mode the engine forwards a pad (any kind) to games as a
//! virtual Xbox 360 pad (`xinput_sink`, ViGEmBus). This module only keeps
//! the state that pad should report and translates our button ids into it,
//! so the mapping is unit-tested on every platform.
//!
//! The id table is positional (the Xbox button in the same place, not with
//! the same letter) and shared by every pad: NS B / A / Y / X, DualSense
//! Cross / Circle / Square / Triangle (ids 0–3) → Xbox A / B / X / Y.
//! Sticks come only from the `Stick` axes; the stick virtual direction ids
//! (15..=22) never set D-pad bits. Capture, GL, GR and C (29..=32) and the
//! DualSense touchpad quadrants are inert (no Xbox equivalent).

use crate::gamepad::GamepadEvent;

/// XInput `wButtons` bits (`XINPUT_GAMEPAD_*`; the same values ViGEm's
/// `XUSB_REPORT` uses).
pub mod xbutton {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    pub const GUIDE: u16 = 0x0400;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// Button id of ZL / ZR (digital; the analog `Trigger` axes 4 / 5 drive the
/// same Xbox triggers).
const ID_ZL: u32 = 23;
const ID_ZR: u32 = 24;

/// What the virtual pad reports: XInput layout, sticks Y up positive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XState {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

/// Xbox bit for a button id; `None` for ids with no Xbox button (triggers are
/// handled separately).
fn button_bit(id: u32) -> Option<u16> {
    use xbutton::*;
    Some(match id {
        0 => A,
        1 => B,
        2 => X,
        3 => Y,
        4 => BACK,
        5 => GUIDE,
        6 => START,
        7 => LEFT_THUMB,
        8 => RIGHT_THUMB,
        9 => LEFT_SHOULDER,
        10 => RIGHT_SHOULDER,
        11 => DPAD_UP,
        12 => DPAD_DOWN,
        13 => DPAD_LEFT,
        14 => DPAD_RIGHT,
        _ => return None,
    })
}

/// [-1, 1] → full i16 stick range (clamped; ±1 → ±32767).
fn stick(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// [0, 1] → 0..=255 (clamped).
fn trigger(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl XState {
    /// Fold one gamepad event into the state. Events with no Xbox
    /// equivalent leave it unchanged.
    pub fn apply(&mut self, ev: &GamepadEvent) {
        match *ev {
            GamepadEvent::ButtonDown(id) | GamepadEvent::ButtonUp(id) => {
                let down = matches!(ev, GamepadEvent::ButtonDown(_));
                match id {
                    ID_ZL => self.lt = if down { 255 } else { 0 },
                    ID_ZR => self.rt = if down { 255 } else { 0 },
                    _ => {
                        if let Some(bit) = button_bit(id) {
                            if down {
                                self.buttons |= bit;
                            } else {
                                self.buttons &= !bit;
                            }
                        }
                    }
                }
            }
            GamepadEvent::Stick { axis, value } => match axis {
                0 => self.lx = stick(value),
                1 => self.ly = stick(value),
                2 => self.rx = stick(value),
                3 => self.ry = stick(value),
                _ => {}
            },
            GamepadEvent::Trigger { axis, value } => match axis {
                4 => self.lt = trigger(value),
                5 => self.rt = trigger(value),
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::GamepadEvent;

    fn down(s: &mut XState, id: u32) {
        s.apply(&GamepadEvent::ButtonDown(id));
    }

    #[test]
    fn virtual_pad_mapping_table() {
        // Positional face buttons, shoulders, system, thumbs, D-pad.
        let table: [(u32, u16); 15] = [
            (0, xbutton::A),
            (1, xbutton::B),
            (2, xbutton::X),
            (3, xbutton::Y),
            (4, xbutton::BACK),
            (5, xbutton::GUIDE),
            (6, xbutton::START),
            (7, xbutton::LEFT_THUMB),
            (8, xbutton::RIGHT_THUMB),
            (9, xbutton::LEFT_SHOULDER),
            (10, xbutton::RIGHT_SHOULDER),
            (11, xbutton::DPAD_UP),
            (12, xbutton::DPAD_DOWN),
            (13, xbutton::DPAD_LEFT),
            (14, xbutton::DPAD_RIGHT),
        ];
        for (id, bit) in table {
            let mut s = XState::default();
            down(&mut s, id);
            assert_eq!(s.buttons, bit, "id {id} → bit {bit:#06x}");
            s.apply(&GamepadEvent::ButtonUp(id));
            assert_eq!(s, XState::default(), "id {id} release clears its bit");
        }
        // Every bit distinct.
        let all = table.iter().fold(0u16, |acc, (_, b)| {
            assert_eq!(acc & b, 0, "bit {b:#06x} used twice");
            acc | b
        });
        assert_eq!(all.count_ones(), 15);

        // ZL / ZR digital ids → full triggers.
        let mut s = XState::default();
        down(&mut s, 23);
        down(&mut s, 24);
        assert_eq!((s.lt, s.rt, s.buttons), (255, 255, 0));
        s.apply(&GamepadEvent::ButtonUp(23));
        s.apply(&GamepadEvent::ButtonUp(24));
        assert_eq!((s.lt, s.rt), (0, 0));

        // Analog triggers scale [0, 1] → [0, 255], clamped.
        let mut s = XState::default();
        s.apply(&GamepadEvent::Trigger { axis: 4, value: 1.0 });
        s.apply(&GamepadEvent::Trigger { axis: 5, value: 0.5 });
        assert_eq!((s.lt, s.rt), (255, 128));
        s.apply(&GamepadEvent::Trigger { axis: 4, value: 7.0 });
        s.apply(&GamepadEvent::Trigger { axis: 5, value: -1.0 });
        assert_eq!((s.lt, s.rt), (255, 0));

        // Sticks scale [-1, 1] → i16, Y up positive kept as is (XInput).
        let mut s = XState::default();
        s.apply(&GamepadEvent::Stick { axis: 0, value: 1.0 });
        s.apply(&GamepadEvent::Stick { axis: 1, value: -1.0 });
        s.apply(&GamepadEvent::Stick { axis: 2, value: 0.5 });
        s.apply(&GamepadEvent::Stick { axis: 3, value: 2.0 });
        assert_eq!((s.lx, s.ly, s.rx, s.ry), (32767, -32767, 16384, 32767));
        assert_eq!(s.buttons, 0, "sticks set no buttons");
        s.apply(&GamepadEvent::Stick { axis: 0, value: 0.0 });
        assert_eq!(s.lx, 0);

        // Stick virtual directions (15..=22) never touch the D-pad; capture,
        // GL, GR and C (29..=32) and the touchpad quadrants are inert.
        for id in (15..=22).chain(25..=32) {
            let mut s = XState::default();
            down(&mut s, id);
            assert_eq!(s, XState::default(), "id {id} must be inert");
        }

        // Non-button events leave the state alone.
        let mut s = XState::default();
        down(&mut s, 0);
        let before = s;
        s.apply(&GamepadEvent::MouseDelta { dx: 3, dy: 4 });
        s.apply(&GamepadEvent::Trigger { axis: 9, value: 1.0 });
        s.apply(&GamepadEvent::Stick { axis: 7, value: 1.0 });
        s.apply(&GamepadEvent::Connected(crate::gamepad::ControllerKind::Ns2Pro));
        assert_eq!(s, before);
    }
}
