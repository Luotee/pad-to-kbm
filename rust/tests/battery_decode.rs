//! Battery read from the input reports (v3.8.0): the DualSense BT 0x31
//! status byte and the Switch Pro 0x30 battery nibble; the board's level
//! (0..=3 bars). The Switch 2 Pro and the Xbox Nano carry none in the
//! reports we decode.

use dualsense_mapper::gamepad::battery::{level, Battery};
use dualsense_mapper::gamepad::ds_protocol::{decode_31, REPORT_LEN_BT};
use dualsense_mapper::gamepad::ns_protocol::{decode_30, StickCal};
use dualsense_mapper::gamepad::xbox_protocol::decode_xbox_bt;

/// A neutral BT 0x31 report with status byte `status` (`buf[54]`: the
/// input report starts at `buf[2]`, `hid-playstation`'s `status` is its
/// byte 52).
fn ds_report(status: u8) -> [u8; REPORT_LEN_BT] {
    let mut buf = [0u8; REPORT_LEN_BT];
    buf[0] = 0x31;
    buf[2..6].copy_from_slice(&[128; 4]);
    buf[9] = 0x08;
    buf[34] = 0x80;
    buf[38] = 0x80;
    buf[54] = status;
    buf
}

/// A neutral 0x30 report with byte 2 (battery high nibble, connection low
/// nibble) set to `b2`.
fn ns_report(b2: u8) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[0] = 0x30;
    b[2] = b2;
    // Sticks centred (2048, 2048).
    b[6..9].copy_from_slice(&[0x00, 0x08, 0x80]);
    b[9..12].copy_from_slice(&[0x00, 0x08, 0x80]);
    b
}

fn ds(status: u8) -> Option<Battery> {
    decode_31(&ds_report(status)).expect("decodes").battery
}

fn ns(b2: u8) -> Option<Battery> {
    decode_30(&ns_report(b2), &StickCal::default()).expect("decodes").battery
}

#[test]
fn dualsense_discharging_80() {
    assert_eq!(ds(0x08), Some(Battery { pct: 80, charging: false }));
}

#[test]
fn dualsense_charging() {
    assert_eq!(ds(0x18), Some(Battery { pct: 80, charging: true }));
}

#[test]
fn dualsense_full_is_100_not_charging() {
    assert_eq!(ds(0x28), Some(Battery { pct: 100, charging: false }));
    assert_eq!(ds(0x20), Some(Battery { pct: 100, charging: false }));
}

#[test]
fn dualsense_level_caps_at_100() {
    assert_eq!(ds(0x0A), Some(Battery { pct: 100, charging: false }));
    assert_eq!(ds(0x0F), Some(Battery { pct: 100, charging: false }));
}

#[test]
fn dualsense_error_status_reads_as_unknown() {
    // 0xA voltage / 0xB temperature / 0xF charging error: no reading.
    assert_eq!(ds(0xA5), None);
    assert_eq!(ds(0xB5), None);
    assert_eq!(ds(0xF5), None);
}

#[test]
fn switch_pro_levels() {
    assert_eq!(ns(0x80), Some(Battery { pct: 100, charging: false }));
    assert_eq!(ns(0x60), Some(Battery { pct: 75, charging: false }));
    assert_eq!(ns(0x40), Some(Battery { pct: 50, charging: false }));
    assert_eq!(ns(0x20), Some(Battery { pct: 25, charging: false }));
    assert_eq!(ns(0x00), Some(Battery { pct: 0, charging: false }));
}

#[test]
fn switch_pro_charging_bit() {
    assert_eq!(ns(0x10), Some(Battery { pct: 0, charging: true }));
    assert_eq!(ns(0x50), Some(Battery { pct: 50, charging: true }));
    assert_eq!(ns(0x90), Some(Battery { pct: 100, charging: true }));
}

#[test]
fn switch_pro_low_nibble_is_not_battery() {
    // The low nibble is connection info.
    assert_eq!(ns(0x8E), Some(Battery { pct: 100, charging: false }));
}

#[test]
fn xbox_nano_report_has_no_battery() {
    let mut buf = [0u8; 16];
    buf[0] = 0x01;
    buf[1..9].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
    assert_eq!(decode_xbox_bt(&buf).expect("decodes").battery, None);
}

#[test]
fn level_bars() {
    for (pct, bars) in [(100, 3), (67, 3), (66, 2), (34, 2), (33, 1), (11, 1), (10, 0), (0, 0)] {
        assert_eq!(level(pct), bars, "pct {pct}");
    }
}

#[test]
fn battery_serializes_as_pct_and_charging() {
    let v = serde_json::to_value(Battery { pct: 75, charging: true }).unwrap();
    assert_eq!(v, serde_json::json!({ "pct": 75, "charging": true }));
}
