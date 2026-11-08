//! Read the Xbox Nano's battery once through the app's own GATT poller
//! (`gamepad::nano_battery`, Battery Service 0x180F / Battery Level 0x2A19).
//! Read-only: nothing is written to the pad.
//!
//! `nano_battery_probe FB3A7C7DF9A8` (the Nano's Bluetooth address, as in
//! 記住的手把's tooltip). Windows only; elsewhere it reads nothing.

use dualsense_mapper::gamepad::nano_battery;
use dualsense_mapper::gamepad::GamepadEvent;
use std::time::{Duration, Instant};

fn main() {
    tracing_subscriber::fmt().with_env_filter("debug").init();
    let arg = std::env::args().nth(1).unwrap_or_default();
    let Some(addr) = dualsense_mapper::hidhide::bt_addr_value(&arg) else {
        eprintln!("usage: nano_battery_probe <12 hex digit Bluetooth address>");
        std::process::exit(2);
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    let t0 = Instant::now();
    let guard = nano_battery::start(Some(addr), &tx);
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(GamepadEvent::Battery(b)) => println!("battery: {b:?} after {} ms", t0.elapsed().as_millis()),
        Ok(other) => println!("unexpected event: {other:?}"),
        Err(_) => println!("no reading within 20 s"),
    }
    drop(guard);
}
