//! Xbox Nano battery, charging and power-off (v3.10.0; the battery part
//! since v3.8.0).
//!
//! * Battery percentage: the standard Battery Service (0x180F), Battery
//!   Level (0x2A19), one byte 0..=100, read at once and then every
//!   `PERIOD` (it was right in every reading; the companion's own
//!   percentage read 0 for a minute after power-on, so it is not used).
//! * Charging and power-off: the Nano's vendor "companion" channel
//!   (`nano_companion_proto`): link hello, app hello, then a battery
//!   request every `Timing::charge`, whose flags carry charging; the
//!   power-off (`0x24 00`) on request. Only allow-listed messages can be
//!   sent. When the channel does not open (Microsoft's driver holds it, an
//!   old pad, no address) or is lost, the Nano reads as in v3.9.1: the
//!   percentage only, never charging, no power-off.
//! * One session per Nano worker session: `start` when it connects, the
//!   guard dropped before the worker's `Disconnected`. Dropping it never
//!   waits for a read or a write in flight (one already started may still
//!   finish), no new one starts and no event is sent after it.
//! * Display and command only: `Battery` and `PowerOff` never open or
//!   close a session (iron rule 11; the hub stores them, `hub.rs`).
//!
//! WinRT (`windows` crate) is Windows-only; elsewhere `start` reads nothing
//! (macOS: Phase 2 TBD, like the rest of the Nano).

use super::battery::Battery;
use super::events::GamepadEvent;
use super::nano_companion_proto::{
    caps_from_hello, charging_from_batt, hello_payload, hello_type, link_hello, parse, Frame, Inbound, Session,
    T_BATT, T_RST, T_ST_BATT,
};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the Battery Level is read while the session is open.
pub const PERIOD: Duration = Duration::from_secs(60);
/// The longest one WinRT step may take before it is cancelled.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Failed Battery Level reads in a row after which the reading is dropped.
const DROP_AFTER: u32 = 2;
/// Battery requests in a row without a reply after which the companion
/// channel counts as lost for the rest of the session.
const LOST_AFTER: u32 = 3;
/// How long the power-off waits for the pad's ACK.
const POWER_OFF_ACK: Duration = Duration::from_secs(1);

/// A Battery Level (0x2A19) value as a `Battery`: the first byte is the
/// percentage; 101..=255 are reserved and an empty value is no reading.
/// No charging flag in this characteristic: `charging` is false here.
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

/// The opened companion characteristic: frames out (write with response),
/// frames in (indications).
pub trait CompanionLink: Send {
    fn write(&mut self, frame: &[u8]) -> bool;
    fn recv(&mut self, timeout: Duration) -> Option<Vec<u8>>;
    /// Payload bytes per data fragment (ATT MTU − 8, at most 0xFA).
    fn frag_max(&self) -> usize;
}

/// Opens the companion channel once per session (`None`: not available).
pub trait CompanionOpener: Send {
    fn open(&mut self) -> Option<Box<dyn CompanionLink>>;
}

/// The session's clock.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Battery request (charging) period.
    pub charge: Duration,
    /// Battery Level (percentage) period.
    pub level: Duration,
    /// Link hello repeat period, and how many are sent.
    pub hello_retry: Duration,
    pub hello_tries: u32,
    /// How long a request waits for its reply.
    pub reply: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            charge: Duration::from_secs(5),
            level: PERIOD,
            hello_retry: Duration::from_secs(5),
            hello_tries: 3,
            reply: Duration::from_secs(5),
        }
    }
}

enum Cmd {
    PowerOff(Sender<bool>),
}

/// The running session of one Nano. Dropping it ends the session: nothing
/// is read, written or sent after that, and it never waits for a step in
/// flight.
pub struct NanoCompanion {
    alive: Arc<Mutex<bool>>,
    cmd: Option<Sender<Cmd>>,
}

impl NanoCompanion {
    fn inert() -> Self {
        Self { alive: Arc::new(Mutex::new(false)), cmd: None }
    }

    /// Turn the Nano off through the companion channel. False when nothing
    /// was sent (no working channel, or the pad did not ACK it); true once
    /// the pad ACKed it, and also when the session did not answer within
    /// one step in flight plus the ACK wait (`READ_TIMEOUT` +
    /// `POWER_OFF_ACK`): the power-off may still go out, so the caller
    /// treats the pad as going off.
    pub fn power_off(&self) -> bool {
        let Some(cmd) = &self.cmd else { return false };
        let (rtx, rrx) = bounded(1);
        if cmd.try_send(Cmd::PowerOff(rtx)).is_err() {
            return false;
        }
        match rrx.recv_timeout(READ_TIMEOUT + POWER_OFF_ACK) {
            Ok(acked) => acked,
            Err(RecvTimeoutError::Timeout) => {
                tracing::info!("Xbox Nano companion: power-off not answered in time; treating the pad as going off");
                true
            }
            Err(RecvTimeoutError::Disconnected) => false,
        }
    }
}

impl Drop for NanoCompanion {
    fn drop(&mut self) {
        // Held only around a send, never across a read or a write.
        if let Ok(mut alive) = self.alive.lock() {
            *alive = false;
        }
        self.cmd.take();
    }
}

/// Start the session for the Nano at Bluetooth address `addr` (from its
/// HID path, `crate::hidhide::bt_addr_from_hid_path`). Without an address,
/// or off Windows, nothing is read.
pub fn start(addr: Option<u64>, tx: &Sender<GamepadEvent>) -> NanoCompanion {
    #[cfg(windows)]
    if let Some(a) = addr {
        return start_with(winrt::GattOpener::new(a), winrt::GattLevel::new(a), Timing::default(), tx);
    }
    let _ = (addr, tx);
    NanoCompanion::inert()
}

/// Start a session on `opener` (the companion channel) and `level` (Battery
/// Level).
pub fn start_with<O: CompanionOpener + 'static, R: LevelReader + Send + 'static>(
    opener: O,
    level: R,
    timing: Timing,
    tx: &Sender<GamepadEvent>,
) -> NanoCompanion {
    let alive = Arc::new(Mutex::new(true));
    let (cmd_tx, cmd_rx) = bounded::<Cmd>(1);
    let (alive_t, tx) = (alive.clone(), tx.clone());
    let spawned = thread::Builder::new().name("nano-companion".into()).spawn(move || {
        Run { alive: alive_t, tx, cmd: cmd_rx, t: timing, tracker: Tracker::default(), last_level: None, charging: false }
            .run(opener, level)
    });
    match spawned {
        Ok(_) => NanoCompanion { alive, cmd: Some(cmd_tx) },
        Err(e) => {
            tracing::warn!(error = %e, "Xbox Nano companion: no thread; no battery shown");
            NanoCompanion::inert()
        }
    }
}

/// The companion channel once both hellos went through.
struct Ready {
    link: Box<dyn CompanionLink>,
    sess: Session,
    power_off: bool,
    charge: bool,
    unanswered: u32,
    next_charge: Instant,
}

struct Run {
    alive: Arc<Mutex<bool>>,
    tx: Sender<GamepadEvent>,
    cmd: Receiver<Cmd>,
    t: Timing,
    tracker: Tracker,
    last_level: Option<Battery>,
    charging: bool,
}

impl Run {
    fn alive(&self) -> bool {
        self.alive.lock().map(|a| *a).unwrap_or(false)
    }

    /// Send while the session is alive (the lock keeps `Drop` waiting until
    /// the send is done). False once it ended.
    fn send(&self, ev: GamepadEvent) -> bool {
        match self.alive.lock() {
            Ok(a) if *a => {
                let _ = self.tx.send(ev);
                true
            }
            _ => false,
        }
    }

    /// Write while the session is alive. The lock is not held across the
    /// write (it can take `READ_TIMEOUT`): dropping the guard must never
    /// wait for it.
    fn write(&self, link: &mut dyn CompanionLink, frame: &[u8]) -> bool {
        self.alive() && link.write(frame)
    }

    fn emit_battery(&mut self) {
        if let Some(l) = self.last_level {
            let b = Battery { pct: l.pct, charging: self.charging };
            if let Some(ev) = self.tracker.next(Some(b)) {
                self.send(GamepadEvent::Battery(ev));
            }
        }
    }

    fn read_level<R: LevelReader>(&mut self, level: &mut R) {
        let reading = level.read().as_deref().and_then(battery_from_level);
        self.last_level = reading;
        match reading {
            Some(_) => self.emit_battery(),
            None => {
                if let Some(ev) = self.tracker.next(None) {
                    self.send(GamepadEvent::Battery(ev));
                }
            }
        }
    }

    /// Commands while no power-off is possible: refuse them. False when
    /// the guard is gone.
    fn refuse_cmds(&self) -> bool {
        loop {
            match self.cmd.try_recv() {
                Ok(Cmd::PowerOff(r)) => {
                    let _ = r.send(false);
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn run<O: CompanionOpener, R: LevelReader>(mut self, mut opener: O, mut level: R) {
        self.read_level(&mut level);
        let mut next_level = Instant::now() + self.t.level;
        let mut ready = if self.alive() { opener.open().and_then(|l| self.handshake(l)) } else { None };
        if let Some(r) = &ready {
            if r.power_off && !self.send(GamepadEvent::PowerOff(true)) {
                return;
            }
        }
        loop {
            if !self.alive() {
                return;
            }
            match self.cmd.try_recv() {
                Ok(Cmd::PowerOff(reply)) => {
                    let ok = match ready.as_mut() {
                        Some(r) if r.power_off => self.power_off(r),
                        _ => false,
                    };
                    let _ = reply.send(ok);
                    if ok {
                        // The pad is going down: nothing more to ask it.
                        ready = None;
                    }
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
            if Instant::now() >= next_level {
                self.read_level(&mut level);
                next_level = Instant::now() + self.t.level;
            }
            match ready.as_mut() {
                Some(r) => {
                    if !self.poll(r) {
                        tracing::info!("Xbox Nano companion: channel lost; percentage only from now on");
                        let power_off = r.power_off;
                        ready = None;
                        self.charging = false;
                        self.emit_battery();
                        if power_off {
                            self.send(GamepadEvent::PowerOff(false));
                        }
                    }
                }
                None => {
                    let wait = next_level.saturating_duration_since(Instant::now()).min(Duration::from_millis(50));
                    match self.cmd.recv_timeout(wait) {
                        Ok(Cmd::PowerOff(r)) => {
                            let _ = r.send(false);
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            }
        }
    }

    /// One steady-state step: the battery request when due, then the
    /// frames that came in. False when the channel is lost.
    fn poll(&mut self, r: &mut Ready) -> bool {
        if r.charge && Instant::now() >= r.next_charge {
            if r.unanswered >= LOST_AFTER {
                return false;
            }
            let Ok(frames) = r.sess.request(T_BATT, &[]) else { return false };
            for f in frames {
                if !self.write(r.link.as_mut(), &f) {
                    return !self.alive();
                }
            }
            r.unanswered += 1;
            r.next_charge = Instant::now() + self.t.charge;
        }
        let Some(f) = r.link.recv(Duration::from_millis(20)) else { return true };
        if !self.alive() {
            return true;
        }
        match r.sess.on_frame(&f) {
            Inbound::Message { ty, payload, ack, .. } => {
                if !self.write(r.link.as_mut(), &ack) {
                    return !self.alive();
                }
                if ty == T_BATT || ty == T_ST_BATT {
                    if let Some(c) = charging_from_batt(&payload, r.sess.ver()) {
                        r.unanswered = 0;
                        if c != self.charging {
                            self.charging = c;
                            self.emit_battery();
                        }
                    }
                }
            }
            Inbound::Fragment { ack } => {
                self.write(r.link.as_mut(), &ack);
            }
            _ => {}
        }
        true
    }

    /// Link hello (repeated), then the app hello. `None`: no channel.
    fn handshake(&mut self, mut link: Box<dyn CompanionLink>) -> Option<Ready> {
        let id = fastrand::u32(1..);
        let mut sess = Session::new(id, link.frag_max());
        let mut linked = None;
        'tries: for _ in 0..self.t.hello_tries {
            if !self.write(link.as_mut(), &link_hello(id)) {
                return None;
            }
            let until = Instant::now() + self.t.hello_retry;
            while Instant::now() < until {
                if !self.refuse_cmds() || !self.alive() {
                    return None;
                }
                if let Some(f) = link.recv(Duration::from_millis(20)) {
                    if let Inbound::Linked { ch, ver } = sess.on_frame(&f) {
                        linked = Some((ch, ver));
                        break 'tries;
                    }
                }
            }
        }
        let Some((ch, ver)) = linked else {
            tracing::info!("Xbox Nano companion: no link hello answer; percentage only");
            return None;
        };
        let ty = hello_type(ver);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        for f in sess.request(ty, &hello_payload(now)).ok()? {
            if !self.write(link.as_mut(), &f) {
                return None;
            }
        }
        let until = Instant::now() + self.t.reply;
        while Instant::now() < until {
            if !self.refuse_cmds() || !self.alive() {
                return None;
            }
            let Some(f) = link.recv(Duration::from_millis(20)) else { continue };
            match sess.on_frame(&f) {
                Inbound::Message { ty: got, payload, ack, .. } => {
                    self.write(link.as_mut(), &ack);
                    if got == ty {
                        let caps = caps_from_hello(&payload)?;
                        tracing::info!(ch, ?ver, caps = format_args!("{:#x}", caps.0), "Xbox Nano companion: ready");
                        return Some(Ready {
                            link,
                            sess,
                            power_off: caps.shutdown(),
                            charge: caps.battery(),
                            unanswered: 0,
                            next_charge: Instant::now(),
                        });
                    }
                }
                Inbound::Fragment { ack } => {
                    self.write(link.as_mut(), &ack);
                }
                _ => {}
            }
        }
        tracing::info!("Xbox Nano companion: no app hello answer; percentage only");
        None
    }

    /// Send the power-off and wait for its ACK.
    fn power_off(&mut self, r: &mut Ready) -> bool {
        let Ok(frames) = r.sess.request(T_RST, &[0]) else { return false };
        let Some(ch) = r.sess.channel() else { return false };
        for f in frames {
            if !self.write(r.link.as_mut(), &f) {
                return false;
            }
        }
        let until = Instant::now() + POWER_OFF_ACK;
        while Instant::now() < until && self.alive() {
            let Some(f) = r.link.recv(Duration::from_millis(20)) else { continue };
            if let Frame::Ack { ch: c, ty: T_RST, .. } = parse(&f) {
                if c == ch {
                    tracing::info!("Xbox Nano companion: power-off acknowledged");
                    return true;
                }
            }
            if let Inbound::Message { ack, .. } | Inbound::Fragment { ack } = r.sess.on_frame(&f) {
                self.write(r.link.as_mut(), &ack);
            }
        }
        false
    }
}

#[cfg(windows)]
mod winrt {
    //! Battery Level and the companion characteristic through WinRT.

    use super::{CompanionLink, CompanionOpener, LevelReader, READ_TIMEOUT};
    use crossbeam_channel::{unbounded, Receiver};
    use std::time::Duration;
    use windows::core::{RuntimeType, GUID};
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristic, GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
        GattDeviceService, GattOpenStatus, GattSession, GattSharingMode, GattValueChangedEventArgs, GattWriteOption,
    };
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
    use windows::Foundation::TypedEventHandler;
    use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};
    use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

    /// Battery Service, Battery Level (Bluetooth base UUID).
    const BATTERY_SERVICE: GUID = GUID::from_u128(0x0000180f_0000_1000_8000_00805f9b34fb);
    const BATTERY_LEVEL: GUID = GUID::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);
    /// The companion service and characteristic.
    const COMPANION_SERVICE: GUID = GUID::from_u128(0x00000000_16e2_469d_8013_a85ccd1a3f11);
    const COMPANION_CHAR: GUID = GUID::from_u128(0x00000004_16e2_469d_8013_a85ccd1a3f11);

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
        bytes(&r.Value().ok()?)
    }

    fn bytes(buf: &IBuffer) -> Option<Vec<u8>> {
        let n = buf.Length().ok()? as usize;
        let mut v = vec![0u8; n];
        if n > 0 {
            DataReader::FromBuffer(buf).ok()?.ReadBytes(&mut v).ok()?;
        }
        Some(v)
    }

    /// Opens the companion characteristic of the Nano at `addr`: service
    /// access + `OpenAsync(SharedReadAndWrite)` (without it the
    /// characteristics read AccessDenied), indications on.
    pub struct GattOpener {
        addr: u64,
    }

    impl GattOpener {
        pub fn new(addr: u64) -> Self {
            Self { addr }
        }
    }

    impl CompanionOpener for GattOpener {
        fn open(&mut self) -> Option<Box<dyn CompanionLink>> {
            let link = GattLink::open(self.addr);
            if link.is_none() {
                tracing::info!("Xbox Nano companion: channel not available; percentage only");
            }
            link.map(|l| Box::new(l) as Box<dyn CompanionLink>)
        }
    }

    struct GattLink {
        dev: BluetoothLEDevice,
        service: GattDeviceService,
        ch: GattCharacteristic,
        token: i64,
        rx: Receiver<Vec<u8>>,
        frag_max: usize,
    }

    // SAFETY-free: the WinRT objects are agile (free-threaded) COM objects;
    // the link is used by the companion thread only.
    unsafe impl Send for GattLink {}

    impl GattLink {
        fn open(addr: u64) -> Option<Self> {
            let dev = wait("device", BluetoothLEDevice::FromBluetoothAddressAsync(addr).ok()?)?;
            let services = wait(
                "service",
                dev.GetGattServicesForUuidWithCacheModeAsync(COMPANION_SERVICE, BluetoothCacheMode::Uncached).ok()?,
            )?;
            if services.Status().ok()? != GattCommunicationStatus::Success {
                tracing::debug!(status = ?services.Status(), "Xbox Nano companion: service lookup failed");
                return None;
            }
            let service = services.Services().ok()?.GetAt(0).ok()?;
            let access = wait("access", service.RequestAccessAsync().ok()?);
            let open = wait("open", service.OpenAsync(GattSharingMode::SharedReadAndWrite).ok()?)?;
            if open != GattOpenStatus::Success && open != GattOpenStatus::AlreadyOpened {
                tracing::debug!(?access, ?open, "Xbox Nano companion: service not opened");
                return None;
            }
            let chars = wait(
                "characteristic",
                service.GetCharacteristicsForUuidWithCacheModeAsync(COMPANION_CHAR, BluetoothCacheMode::Uncached).ok()?,
            )?;
            if chars.Status().ok()? != GattCommunicationStatus::Success {
                tracing::debug!(status = ?chars.Status(), "Xbox Nano companion: characteristic lookup failed");
                return None;
            }
            let ch = chars.Characteristics().ok()?.GetAt(0).ok()?;
            let (tx, rx) = unbounded::<Vec<u8>>();
            let token = ch
                .ValueChanged(&TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(move |_, a| {
                    if let Some(a) = a.as_ref() {
                        if let Some(v) = bytes(&a.CharacteristicValue()?) {
                            let _ = tx.send(v);
                        }
                    }
                    Ok(())
                }))
                .ok()?;
            let cccd = wait(
                "indicate",
                ch.WriteClientCharacteristicConfigurationDescriptorWithResultAsync(
                    GattClientCharacteristicConfigurationDescriptorValue::Indicate,
                )
                .ok()?,
            )?;
            if cccd.Status().ok()? != GattCommunicationStatus::Success {
                tracing::debug!(status = ?cccd.Status(), "Xbox Nano companion: indications not enabled");
                let _ = ch.RemoveValueChanged(token);
                return None;
            }
            let mtu = dev
                .BluetoothDeviceId()
                .ok()
                .and_then(|id| GattSession::FromDeviceIdAsync(&id).ok())
                .and_then(|op| wait("session", op))
                .and_then(|s| s.MaxPduSize().ok())
                .unwrap_or(23) as usize;
            Some(GattLink { dev, service, ch, token, rx, frag_max: mtu.saturating_sub(8).clamp(1, 0xfa) })
        }
    }

    impl CompanionLink for GattLink {
        fn write(&mut self, frame: &[u8]) -> bool {
            let buf = (|| {
                let w = DataWriter::new().ok()?;
                w.WriteBytes(frame).ok()?;
                w.DetachBuffer().ok()
            })();
            let Some(buf) = buf else { return false };
            let Some(op) = self.ch.WriteValueWithResultAndOptionAsync(&buf, GattWriteOption::WriteWithResponse).ok()
            else {
                return false;
            };
            wait("write", op).and_then(|r| r.Status().ok()) == Some(GattCommunicationStatus::Success)
        }

        fn recv(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            self.rx.recv_timeout(timeout).ok()
        }

        fn frag_max(&self) -> usize {
            self.frag_max
        }
    }

    impl Drop for GattLink {
        fn drop(&mut self) {
            let _ = self.ch.RemoveValueChanged(self.token);
            let _ = self.service.Close();
            let _ = self.dev.Close();
        }
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
            tracing::debug!(step, "Xbox Nano: WinRT call timed out");
            let _ = op.Cancel();
            return None;
        }
        op.GetResults().map_err(|e| tracing::debug!(step, error = %e, "Xbox Nano: WinRT call failed")).ok()
    }
}
