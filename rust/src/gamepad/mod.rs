//! Gamepad event source abstraction.
//!
//! v2.0.0 onwards: DualSense BT raw HID only via `hidapi`. The v1.x
//! gilrs path has been removed. v2.6.0 adds the Switch 2 Pro BLE worker;
//! `GamepadSource` is the `hub::SourceHub` that lets one of the two
//! workers own the session at a time. See
//! `docs/superpowers/specs/2026-05-17-v2.0.0-raw-hid-dualsense-design.md`
//! for the design.

pub mod controller;
pub mod cursor_params;
pub mod events;
pub mod ds_protocol;
pub mod hid_source;
pub mod hub;
pub mod lightbar;
pub mod ns_handshake;
pub mod ns_protocol;
pub mod ns2_ble;
pub mod ns2_protocol;
pub mod pad;
pub mod raw_share;
pub mod rumble;
pub mod stick_cal;
pub mod xbox_protocol;

pub use controller::ControllerKind;
pub use cursor_params::CursorParams;
pub use events::GamepadEvent;
pub use hub::SourceHub as GamepadSource;
pub use pad::{PadState, BUTTON_SLOTS};

/// A request from the engine to the worker that owns the session.
/// Only `Disconnect` ends a session; `Rumble` never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCmd {
    /// Manual disconnect (Turn Off): the worker drops its pad.
    Disconnect,
    /// One short, gentle rumble on the connected pad.
    Rumble,
}

/// What each Turn Off's link-level power-off did
/// (`crate::platform::power_off_connected`). The HID worker records the kind
/// and the number of Bluetooth links it dropped (0: USB, no match, or the
/// call failed) before it reports the pad gone, so whoever sees the pad
/// leave finds the result already here. The engine drains it.
#[derive(Debug, Clone, Default)]
pub struct PowerOffLog(std::sync::Arc<std::sync::Mutex<Vec<(ControllerKind, usize)>>>);

impl PowerOffLog {
    pub fn record(&self, kind: ControllerKind, links: usize) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push((kind, links));
    }

    /// Every result since the last call, oldest first.
    pub fn take(&self) -> Vec<(ControllerKind, usize)> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// A command for the hub: `to: None` goes to the session owner; `Some(k)`
/// goes to the source whose open session is pad `k` (owner or waiting),
/// and is dropped if no such pad is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubCmd {
    pub to: Option<ControllerKind>,
    pub cmd: SourceCmd,
}

impl HubCmd {
    /// For the session owner.
    pub fn owner(cmd: SourceCmd) -> Self {
        HubCmd { to: None, cmd }
    }

    /// For pad `kind`, whether it owns the session or waits.
    pub fn to(kind: ControllerKind, cmd: SourceCmd) -> Self {
        HubCmd { to: Some(kind), cmd }
    }
}
