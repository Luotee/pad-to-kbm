//! Pad battery as read from the input reports (v3.8.0). Display only: the
//! battery never affects the connection state (iron rule 11).

use serde::Serialize;

/// One battery reading.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub struct Battery {
    /// Charge, 0..=100. The Switch Pro reports steps of 25.
    pub pct: u8,
    pub charging: bool,
}

/// Bars the board shows for `pct`: 0..=3.
pub fn level(pct: u8) -> u8 {
    match pct {
        67.. => 3,
        34..=66 => 2,
        11..=33 => 1,
        _ => 0,
    }
}

/// DualSense status byte (BT 0x31 `buf[54]`; `hid-playstation`'s `status`):
/// low nibble = charge in tenths, high nibble = 0 discharging, 1 charging,
/// 2 full. Any other high nibble (voltage / temperature / charging error,
/// unknown) is no reading.
pub(crate) fn from_dualsense_status(status: u8) -> Option<Battery> {
    let pct = ((status & 0x0F) * 10).min(100);
    match status >> 4 {
        0x0 => Some(Battery { pct, charging: false }),
        0x1 => Some(Battery { pct, charging: true }),
        0x2 => Some(Battery { pct: 100, charging: false }),
        _ => None,
    }
}

/// Switch Pro 0x30 byte 2: high nibble = level (8 full, 6, 4, 2, 0 empty)
/// with bit 4 = charging; the low nibble is connection info.
pub(crate) fn from_switch_pro(b: u8) -> Battery {
    let steps = (b >> 5) & 0x07; // level / 2
    Battery { pct: (steps * 25).min(100), charging: b & 0x10 != 0 }
}
