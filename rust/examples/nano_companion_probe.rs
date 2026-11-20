//! Run the app's Xbox Nano companion session (`gamepad::nano_companion`)
//! for 15 s and print what it reports: the battery (percentage from
//! Battery Level 0x2A19, charging from the companion channel) and whether
//! the power-off is available. With `--off`, then turn the Nano off.
//!
//! Writes to the pad only what the session itself sends (link hello, app
//! hello, battery requests; the power-off with `--off`).
//!
//! `nano_companion_probe FB3A7C7DF9A8 [--off]` (the Nano's Bluetooth
//! address, as in 記住的手把's tooltip). Windows only; elsewhere it reads
//! nothing.

use dualsense_mapper::gamepad::nano_companion;
use std::time::{Duration, Instant};

fn main() {
    tracing_subscriber::fmt().with_env_filter("debug").init();
    let arg = std::env::args().nth(1).unwrap_or_default();
    let off = std::env::args().any(|a| a == "--off");
    let Some(addr) = dualsense_mapper::hidhide::bt_addr_value(&arg) else {
        eprintln!("usage: nano_companion_probe <12 hex digit Bluetooth address> [--off]");
        std::process::exit(2);
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    let t0 = Instant::now();
    let guard = nano_companion::start(Some(addr), &tx);
    while t0.elapsed() < Duration::from_secs(15) {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
            println!("{:>6} ms  {ev:?}", t0.elapsed().as_millis());
        }
    }
    if off {
        println!("power-off: {}", if guard.power_off() { "acknowledged" } else { "not available / no ACK" });
    }
    drop(guard);
}
