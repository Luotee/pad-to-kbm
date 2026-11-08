//! Xbox Nano Bluetooth LE (HID over GATT) input report decoder.
//!
//! Pure function, no I/O. Layout recorded on the real pad (VID 0x045E,
//! PID 0x0B4F, gamepad collection 0001/0005, fixtures in
//! `tests/fixtures/xbox_nano/`); it matches SDL's Xbox Bluetooth layout:
//!
//! | byte    | field                                                  |
//! |---------|--------------------------------------------------------|
//! | 0       | report id 0x01                                         |
//! | 1..=8   | LX, LY, RX, RY: u16 LE, centre 0x8000; Y grows downwards (HID) |
//! | 9..=12  | LT, RT: u16 LE, 10 bit (0..=0x3FF)                     |
//! | 13      | hat: 0 none, 1 N, 2 NE, 3 E, ... 8 NW                  |
//! | 14      | A 0x01, B 0x02, X 0x08, Y 0x10, LB 0x40, RB 0x80       |
//! | 15      | View 0x04, Menu 0x08, Xbox 0x10, LS 0x20, RS 0x40      |
//!
//! The pad sends a report only when something changes. Share is not in
//! this report: it arrives as a keystroke on the keyboard collection,
//! which Windows owns. Battery is not in any HID report.

use super::pad::PadState;

/// Report id of the gamepad input report.
pub const REPORT_ID: u8 = 0x01;
/// Length of the gamepad input report, report id included.
pub const REPORT_LEN: usize = 16;
/// Full-scale trigger value (10 bit).
const TRIGGER_MAX: u16 = 0x3FF;

/// Decode one gamepad report. `None` on a short buffer or another report
/// id. Button ids follow the DualSense numbering (A = Cross = 0, ...).
pub fn decode_xbox_bt(buf: &[u8]) -> Option<PadState> {
    if buf.len() < REPORT_LEN || buf[0] != REPORT_ID {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([buf[i], buf[i + 1]]);
    let mut s = PadState::default();
    s.stick_lx = stick_axis(u16_at(1));
    s.stick_ly = -stick_axis(u16_at(3));
    s.stick_rx = stick_axis(u16_at(5));
    s.stick_ry = -stick_axis(u16_at(7));
    s.trigger_l2 = trigger(u16_at(9));
    s.trigger_r2 = trigger(u16_at(11));

    let (up, right, down, left) = decode_hat(buf[13]);
    s.buttons[11] = up;
    s.buttons[12] = down;
    s.buttons[13] = left;
    s.buttons[14] = right;

    let b0 = buf[14];
    let b1 = buf[15];
    s.buttons[0] = b0 & 0x01 != 0; // A
    s.buttons[1] = b0 & 0x02 != 0; // B
    s.buttons[2] = b0 & 0x08 != 0; // X
    s.buttons[3] = b0 & 0x10 != 0; // Y
    s.buttons[9] = b0 & 0x40 != 0; // LB
    s.buttons[10] = b0 & 0x80 != 0; // RB
    s.buttons[4] = b1 & 0x04 != 0; // View
    s.buttons[6] = b1 & 0x08 != 0; // Menu
    s.buttons[5] = b1 & 0x10 != 0; // Xbox
    s.buttons[7] = b1 & 0x20 != 0; // LS (L3)
    s.buttons[8] = b1 & 0x40 != 0; // RS (R3)
    // LT / RT (ids 23 / 24) have no digital bit; the mapper derives them
    // from the analog value.
    Some(s)
}

/// u16 centred on 0x8000 → [-1, 1].
fn stick_axis(raw: u16) -> f32 {
    ((raw as f32 - 32768.0) / 32768.0).clamp(-1.0, 1.0)
}

fn trigger(raw: u16) -> f32 {
    raw.min(TRIGGER_MAX) as f32 / TRIGGER_MAX as f32
}

/// Hat 1..=8 clockwise from north → (up, right, down, left); 0 (and
/// anything else) is released.
fn decode_hat(hat: u8) -> (bool, bool, bool, bool) {
    match hat {
        1 => (true, false, false, false),
        2 => (true, true, false, false),
        3 => (false, true, false, false),
        4 => (false, true, true, false),
        5 => (false, false, true, false),
        6 => (false, false, true, true),
        7 => (false, false, false, true),
        8 => (true, false, false, true),
        _ => (false, false, false, false),
    }
}
