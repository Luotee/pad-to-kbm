//! Xbox Nano battery over Bluetooth GATT (v3.8.0).
//!
//! The Nano's battery is in neither its HID report nor XInput; it is the
//! standard Battery Service (0x180F), Battery Level characteristic (0x2A19):
//! one byte, 0..=100 % (0x46 = 70 % on the maintainer's Nano, 2026-11-09,
//! read the same with and without Microsoft's Xbox driver). Battery Level
//! has no charging flag, so the Nano never shows charging.
//!
//! * The Nano worker starts one poller per session (`start`, from the Nano's
//!   Bluetooth address in its HID path) and drops the guard when the session
//!   ends, before it sends `Disconnected`.
//! * The poller runs on its own thread: it reads at once, then every
//!   `PERIOD`. Each WinRT step waits at most `READ_TIMEOUT` (then it is
//!   cancelled), so a slow or stuck read never touches the worker.
//! * It sends `GamepadEvent::Battery` on the Nano worker's channel, only
//!   for a new reading (`Tracker`); a reading that is gone for two reads in
//!   a row is sent as `None` (the tab goes back to the lamp). Once the guard
//!   is dropped nothing more is sent, so no reading lands after the
//!   worker's `Disconnected`.
//! * Display only: a reading never opens or closes a session (iron rule 11;
//!   the hub stores it, `hub.rs`).
//!
//! WinRT (`windows` crate) is Windows-only; elsewhere `start` reads nothing
//! (macOS: Phase 2 TBD, like the rest of the Nano).

use super::battery::Battery;
use super::events::GamepadEvent;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How often the Nano's battery is read while its session is open.
pub const PERIOD: Duration = Duration::from_secs(60);
/// The longest one WinRT step (open the device, find the service or the
/// characteristic, read the value) may take before it is cancelled.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Failed reads in a row after which the last reading is dropped.
const DROP_AFTER: u32 = 2;

/// A Battery Level (0x2A19) value as a `Battery`: the first byte is the
/// percentage; 101..=255 are reserved and an empty value is no reading.
/// No charging flag in this characteristic: `charging` is always false.
pub fn battery_from_level(value: &[u8]) -> Option<Battery> {
    let pct = *value.first()?;
    (pct <= 100).then_some(Battery { pct, charging: false })
}

/// Which readings are sent: the first one, each change, and `None` once
/// `DROP_AFTER` reads in a row failed after a reading was sent.
#[derive(Debug, Default)]
pub struct Tracker {
    last: Option<Battery>,
    failures: u32,
}

impl Tracker {
    /// The event to send for this read, if any (`Some(None)` = drop it).
    pub fn next(&mut self, reading: Option<Battery>) -> Option<Option<Battery>> {
        match reading {
            Some(b) => {
                self.failures = 0;
                (self.last != Some(b)).then(|| {
                    self.last = Some(b);
                    Some(b)
                })
            }
            None => {
                self.failures = self.failures.saturating_add(1);
                (self.last.is_some() && self.failures >= DROP_AFTER).then(|| {
                    self.last = None;
                    None
                })
            }
        }
    }
}

/// One Battery Level read; `None` when it fails or times out.
pub trait LevelReader {
    fn read(&mut self) -> Option<Vec<u8>>;
}

/// The running poller of one Nano session. Dropping it ends the poller:
/// no read starts and no reading is sent after that. It never waits for
/// a read in flight.
pub struct NanoBattery {
    alive: Arc<Mutex<bool>>,
    stop: Option<Sender<()>>,
}

impl NanoBattery {
    fn inert() -> Self {
        Self { alive: Arc::new(Mutex::new(false)), stop: None }
    }
}

impl Drop for NanoBattery {
    fn drop(&mut self) {
        // Held only around a send (never across a read), so this is quick.
        if let Ok(mut alive) = self.alive.lock() {
            *alive = false;
        }
        // Dropping the sender wakes the poller's wait.
        self.stop.take();
    }
}

/// Start the poller for the Nano at Bluetooth address `addr` (its HID
/// path, `crate::hidhide::bt_addr_from_hid_path`). Without an address, or
/// off Windows, nothing is read.
pub fn start(addr: Option<u64>, tx: &Sender<GamepadEvent>) -> NanoBattery {
    #[cfg(windows)]
    if let Some(a) = addr {
        return start_with(winrt::GattLevel::new(a), PERIOD, tx);
    }
    let _ = (addr, tx);
    NanoBattery::inert()
}

/// Start a poller on `reader`: a read at once, then every `period`.
pub fn start_with<R: LevelReader + Send + 'static>(
    reader: R,
    period: Duration,
    tx: &Sender<GamepadEvent>,
) -> NanoBattery {
    let alive = Arc::new(Mutex::new(true));
    let (stop_tx, stop_rx) = bounded::<()>(0);
    let (alive_t, tx) = (alive.clone(), tx.clone());
    let spawned = thread::Builder::new()
        .name("nano-battery".into())
        .spawn(move || poll(reader, period, &stop_rx, &alive_t, &tx));
    match spawned {
        Ok(_) => NanoBattery { alive, stop: Some(stop_tx) },
        Err(e) => {
            tracing::warn!(error = %e, "Xbox Nano battery: no thread; no battery shown");
            NanoBattery::inert()
        }
    }
}

fn poll<R: LevelReader>(
    mut reader: R,
    period: Duration,
    stop: &Receiver<()>,
    alive: &Mutex<bool>,
    tx: &Sender<GamepadEvent>,
) {
    let mut tracker = Tracker::default();
    loop {
        if !alive.lock().map(|a| *a).unwrap_or(false) {
            return;
        }
        let reading = reader.read().as_deref().and_then(battery_from_level);
        if let Some(ev) = tracker.next(reading) {
            tracing::debug!(?ev, "Xbox Nano battery");
            match alive.lock() {
                Ok(a) if *a => {
                    let _ = tx.send(GamepadEvent::Battery(ev));
                }
                _ => return,
            }
        }
        match stop.recv_timeout(period) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
    }
}

#[cfg(windows)]
mod winrt {
    //! Battery Level through WinRT: `BluetoothLEDevice` from the address,
    //! its Battery Service and Battery Level characteristic (from Windows'
    //! GATT cache), `ReadValue` (uncached: from the pad). The device and characteristic are kept for the session
    //! and looked up again after a failed read.

    use super::{LevelReader, READ_TIMEOUT};
    use windows::core::{RuntimeType, GUID};
    use windows::Devices::Bluetooth::GenericAttributeProfile::{GattCharacteristic, GattCommunicationStatus};
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
    use windows::Storage::Streams::DataReader;
    use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

    /// Battery Service, Battery Level (Bluetooth base UUID).
    const BATTERY_SERVICE: GUID = GUID::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb);
    const BATTERY_LEVEL: GUID = GUID::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);

    pub struct GattLevel {
        addr: u64,
        open: Option<(BluetoothLEDevice, GattCharacteristic)>,
    }

    impl GattLevel {
        pub fn new(addr: u64) -> Self {
            Self { addr, open: None }
        }

        fn open(&self) -> Option<(BluetoothLEDevice, GattCharacteristic)> {
            let dev = wait("device", BluetoothLEDevice::FromBluetoothAddressAsync(self.addr).ok()?)?;
            // The GATT table from Windows' cache (filled at pairing); only the
            // value itself is read from the pad.
            let services = wait(
                "service",
                dev.GetGattServicesForUuidWithCacheModeAsync(BATTERY_SERVICE, BluetoothCacheMode::Cached).ok()?,
            )?;
            if services.Status().ok()? != GattCommunicationStatus::Success {
                tracing::debug!(status = ?services.Status(), "Xbox Nano battery: service lookup failed");
                return None;
            }
            let service = services.Services().ok()?.GetAt(0).ok()?;
            let chars = wait(
                "characteristic",
                service.GetCharacteristicsForUuidWithCacheModeAsync(BATTERY_LEVEL, BluetoothCacheMode::Cached).ok()?,
            )?;
            if chars.Status().ok()? != GattCommunicationStatus::Success {
                tracing::debug!(status = ?chars.Status(), "Xbox Nano battery: characteristic lookup failed");
                return None;
            }
            let ch = chars.Characteristics().ok()?.GetAt(0).ok()?;
            Some((dev, ch))
        }
    }

    impl LevelReader for GattLevel {
        fn read(&mut self) -> Option<Vec<u8>> {
            if self.open.is_none() {
                self.open = self.open();
            }
            let value = self.open.as_ref().and_then(|(_, ch)| read_value(ch));
            if value.is_none() {
                // Look everything up again next time.
                if let Some((dev, _)) = self.open.take() {
                    let _ = dev.Close();
                }
            }
            value
        }
    }

    impl Drop for GattLevel {
        fn drop(&mut self) {
            if let Some((dev, _)) = self.open.take() {
                let _ = dev.Close();
            }
        }
    }

    fn read_value(ch: &GattCharacteristic) -> Option<Vec<u8>> {
        let r = wait("read", ch.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).ok()?)?;
        if r.Status().ok()? != GattCommunicationStatus::Success {
            tracing::debug!(status = ?r.Status(), "Xbox Nano battery: read failed");
            return None;
        }
        let buf = r.Value().ok()?;
        let n = buf.Length().ok()? as usize;
        let mut v = vec![0u8; n];
        DataReader::FromBuffer(&buf).ok()?.ReadBytes(&mut v).ok()?;
        Some(v)
    }

    /// The operation's result, or `None` when it fails or takes longer
    /// than `READ_TIMEOUT` (then it is cancelled).
    fn wait<T: RuntimeType + 'static>(step: &str, op: IAsyncOperation<T>) -> Option<T> {
        let (tx, rx) = crossbeam_channel::bounded::<()>(1);
        op.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
            let _ = tx.try_send(());
            Ok(())
        }))
        .ok()?;
        if rx.recv_timeout(READ_TIMEOUT).is_err() {
            tracing::debug!(step, "Xbox Nano battery: WinRT call timed out");
            let _ = op.Cancel();
            return None;
        }
        op.GetResults().map_err(|e| tracing::debug!(step, error = %e, "Xbox Nano battery: WinRT call failed")).ok()
    }
}
