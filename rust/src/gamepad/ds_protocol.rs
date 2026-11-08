//! DualSense BT 0x31 input report decoder + output handshake helper.
//!
//! Pure functions: no I/O, no allocations on the hot path. Drives the
//! main difference between v1.x (gilrs) and v2.x (raw HID) — every
//! decoded byte is observable and testable from synthesised buffers.

use super::pad::PadState;

/// Length of the BT 0x31 input report.
pub const REPORT_LEN_BT: usize = 78;

/// Length of the feature report 0x05 (calibration request) used to
/// trigger BT 0x31 mode. Sending this as a feature `get_report` causes
/// the pad to start emitting 0x31 input reports.
pub const HANDSHAKE_FEATURE_LEN: usize = 41;

/// Build the buffer to pass to `HidDevice::get_feature_report` to
/// trigger 0x31 mode. Byte 0 = `0x05`, rest zeroed.
pub fn build_handshake_buffer() -> [u8; HANDSHAKE_FEATURE_LEN] {
    let mut buf = [0u8; HANDSHAKE_FEATURE_LEN];
    buf[0] = 0x05;
    buf
}

/// Decode a 78-byte BT 0x31 report. Returns None on short buffer or
/// wrong report id.
pub fn decode_31(buf: &[u8]) -> Option<PadState> {
    if buf.len() < REPORT_LEN_BT || buf[0] != 0x31 {
        return None;
    }
    let mut s = PadState::default();
    s.stick_lx = stick_axis(buf[2]);
    s.stick_ly = -stick_axis(buf[3]);
    s.stick_rx = stick_axis(buf[4]);
    s.stick_ry = -stick_axis(buf[5]);
    s.trigger_l2 = buf[6] as f32 / 255.0;
    s.trigger_r2 = buf[7] as f32 / 255.0;

    let b0 = buf[9];
    let b1 = buf[10];
    let b2 = buf[11];

    s.buttons[0]  = (b0 >> 5) & 1 == 1;  // Cross
    s.buttons[1]  = (b0 >> 6) & 1 == 1;  // Circle
    s.buttons[2]  = (b0 >> 4) & 1 == 1;  // Square
    s.buttons[3]  = (b0 >> 7) & 1 == 1;  // Triangle
    s.buttons[4]  = (b1 >> 4) & 1 == 1;  // Share
    s.buttons[5]  =  b2       & 1 == 1;  // PS
    s.buttons[6]  = (b1 >> 5) & 1 == 1;  // Options
    s.buttons[7]  = (b1 >> 6) & 1 == 1;  // L3
    s.buttons[8]  = (b1 >> 7) & 1 == 1;  // R3
    s.buttons[9]  =  b1       & 1 == 1;  // L1
    s.buttons[10] = (b1 >> 1) & 1 == 1;  // R1
    s.buttons[23] = (b1 >> 2) & 1 == 1;  // L2 digital
    s.buttons[24] = (b1 >> 3) & 1 == 1;  // R2 digital

    let hat = b0 & 0x0F;
    let (up, down, left, right) = decode_hat(hat);
    s.buttons[11] = up;
    s.buttons[12] = down;
    s.buttons[13] = left;
    s.buttons[14] = right;

    // Touchpad whole-pad click switch — buttons[2] bit 1.
    s.touchpad_btn = (b2 >> 1) & 1 == 1;

    // Touchpad finger 0 — bytes 34..=37 in the BT 0x31 report.
    //
    // USB report 0x01 lays finger 0 at bytes 33..=36; BT 0x31 inserts
    // a sub-id byte at offset 1 so every payload byte shifts +1 (same
    // shift that puts stick_lx at buf[2] / buttons0 at buf[9]). v2.1.0
    // initially used the USB offsets here — the result was cursor
    // jitter from random gyro / timestamp data and wrong quadrant
    // ids at click-down. Always use the BT-relative offsets.
    //
    //   byte 34: high bit = "no contact" flag; low 7 bits = touch id
    //   byte 35: x[7..0]
    //   byte 36: low nibble = x[11..8], high nibble = y[3..0]
    //   byte 37: y[11..4]
    let f0 = &buf[34..=37];
    s.finger0_active = (f0[0] & 0x80) == 0;
    let x_lo = f0[1] as u16;
    let x_hi = (f0[2] & 0x0F) as u16;
    let y_lo = ((f0[2] & 0xF0) >> 4) as u16;
    let y_hi = f0[3] as u16;
    s.finger0_x = x_lo | (x_hi << 8);
    s.finger0_y = y_lo | (y_hi << 4);

    // Status byte: report byte 52 in `hid-playstation`, +2 here (report id
    // and the BT sub-id byte).
    s.battery = super::battery::from_dualsense_status(buf[54]);

    Some(s)
}

fn stick_axis(raw: u8) -> f32 {
    (raw as f32 - 128.0) / 128.0
}

/// 4-bit hat → (up, down, left, right). 8 = released.
fn decode_hat(hat: u8) -> (bool, bool, bool, bool) {
    match hat {
        0 => (true,  false, false, false),
        1 => (true,  false, false, true),
        2 => (false, false, false, true),
        3 => (false, true,  false, true),
        4 => (false, true,  false, false),
        5 => (false, true,  true,  false),
        6 => (false, false, true,  false),
        7 => (true,  false, true,  false),
        _ => (false, false, false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::pad::BUTTON_SLOTS;

    fn neutral_report() -> [u8; REPORT_LEN_BT] {
        let mut buf = [0u8; REPORT_LEN_BT];
        buf[0] = 0x31;
        buf[2] = 128;
        buf[3] = 128;
        buf[4] = 128;
        buf[5] = 128;
        buf[9] = 0x08;  // hat = released, no buttons in byte 9 upper nibble
        buf[34] = 0x80; // touchpad finger 0 inactive (high bit = "no contact")
        buf[38] = 0x80; // touchpad finger 1 inactive (decoded but ignored in v2.1)
        buf
    }

    #[test]
    fn decode_31_never_sets_slots_beyond_28() {
        let mut buf = neutral_report();
        buf[9] = 0xFF;
        buf[10] = 0xFF;
        buf[11] = 0xFF;
        let s = decode_31(&buf).unwrap();
        assert_eq!(s.buttons.len(), BUTTON_SLOTS);
        assert!(!s.buttons[29]);
    }

    #[test]
    fn handshake_buffer_is_41_bytes_starting_with_05() {
        let buf = build_handshake_buffer();
        assert_eq!(buf.len(), 41);
        assert_eq!(buf[0], 0x05);
        assert!(buf[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn decode_rejects_short_buffer() {
        let buf = [0u8; 10];
        assert!(decode_31(&buf).is_none());
    }

    #[test]
    fn decode_rejects_wrong_report_id() {
        let mut buf = [0u8; REPORT_LEN_BT];
        buf[0] = 0x01;
        assert!(decode_31(&buf).is_none());
    }

    #[test]
    fn decode_neutral_state_has_no_buttons_and_centered_sticks() {
        let buf = neutral_report();
        let s = decode_31(&buf).unwrap();
        assert_eq!(s.stick_lx, 0.0);
        assert_eq!(s.stick_ly, 0.0);
        assert_eq!(s.stick_rx, 0.0);
        assert_eq!(s.stick_ry, 0.0);
        assert_eq!(s.trigger_l2, 0.0);
        assert_eq!(s.trigger_r2, 0.0);
        for (i, b) in s.buttons.iter().enumerate() {
            assert!(!b, "button {i} should be unpressed in neutral state");
        }
    }

    #[test]
    fn decode_cross_pressed_emits_only_id_0() {
        let mut buf = neutral_report();
        buf[9] |= 1 << 5;
        let s = decode_31(&buf).unwrap();
        assert!(s.buttons[0]);
        for (i, b) in s.buttons.iter().enumerate() {
            if i == 0 { continue; }
            assert!(!b, "button {i} unexpectedly set");
        }
    }

    #[test]
    fn decode_l2_analog_range() {
        let mut buf = neutral_report();
        buf[6] = 0;
        assert!((decode_31(&buf).unwrap().trigger_l2 - 0.0).abs() < 1e-6);
        buf[6] = 127;
        assert!((decode_31(&buf).unwrap().trigger_l2 - 127.0 / 255.0).abs() < 1e-6);
        buf[6] = 255;
        assert!((decode_31(&buf).unwrap().trigger_l2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn decode_stick_ly_inverted() {
        let mut buf = neutral_report();
        buf[3] = 0;  // hardware "up"
        let s = decode_31(&buf).unwrap();
        assert!((s.stick_ly - 1.0).abs() < 1e-6, "got {}", s.stick_ly);
    }

    #[test]
    fn decode_touchpad_finger0_inactive_by_default() {
        let buf = neutral_report();
        let s = decode_31(&buf).unwrap();
        assert!(!s.finger0_active, "neutral report should have no finger contact");
    }

    #[test]
    fn decode_touchpad_finger0_active_position() {
        let mut buf = neutral_report();
        // active=1 (high bit clear), id=0
        buf[34] = 0x00;
        // x = 1500 = 0x5DC → x_low_8 = 0xDC, x_high_4 = 0x5
        // y = 800  = 0x320 → y_low_4 = 0x0, y_high_8 = 0x32
        buf[35] = 0xDC;
        buf[36] = 0x05 | (0x0 << 4);
        buf[37] = 0x32;
        let s = decode_31(&buf).unwrap();
        assert!(s.finger0_active);
        assert_eq!(s.finger0_x, 1500);
        assert_eq!(s.finger0_y, 800);
    }

    #[test]
    fn decode_touchpad_click_button() {
        let mut buf = neutral_report();
        buf[11] |= 1 << 1;
        let s = decode_31(&buf).unwrap();
        assert!(s.touchpad_btn);
    }

    #[test]
    fn decode_hat_to_dpad_buttons() {
        let mut buf = neutral_report();
        // hat = 0 → up only
        buf[9] = (buf[9] & 0xF0) | 0;
        let s = decode_31(&buf).unwrap();
        assert!(s.buttons[11]);
        assert!(!s.buttons[12]);
        assert!(!s.buttons[13]);
        assert!(!s.buttons[14]);
        // hat = 8 → released, all clear
        buf[9] = (buf[9] & 0xF0) | 8;
        let s = decode_31(&buf).unwrap();
        assert!(!s.buttons[11]);
        // hat = 1 → up-right
        buf[9] = (buf[9] & 0xF0) | 1;
        let s = decode_31(&buf).unwrap();
        assert!(s.buttons[11]);
        assert!(s.buttons[14]);
    }
}
