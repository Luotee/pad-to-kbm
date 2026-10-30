//! Switch 2 Pro Controller (NS2 Pro) BLE transport.
//!
//! Windows: `Ns2BleSource` spawns one std thread ("ns2-ble") that runs a
//! current-thread tokio runtime driving btleplug through
//! Scanning → Connecting → Initializing → Streaming, and back to Scanning
//! on any failure, lost link or manual disconnect. A scan with no matching
//! advert is restarted every 30 s, because btleplug never reports a WinRT
//! watcher that Windows aborted (Bluetooth off/on, sleep/wake). While the
//! suspend flag is set (another controller owns the session, see
//! `super::hub`) the worker neither scans nor connects. The hardware-proven
//! connect sequence (spec §12) is: `connect()`, *immediately*
//! `request_connection_parameters(ThroughputOptimized)`, 500 ms pause,
//! `discover_services()`, response notify, optional `0x15` pairing,
//! stick calibration, player LED, input notify. OS SMP pairing is never
//! triggered.
//!
//! Other platforms: a stub with the same API that never connects.
//!
//! The decisions (advert acceptance, watchdog, rescan pacing, the
//! Connected/Disconnected contract of iron rule 11) are pure functions
//! below so they are unit-tested on every platform; the btleplug glue in
//! `imp` stays thin.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use super::controller::ControllerKind;
use super::events::GamepadEvent;
use super::SourceCmd;
use super::ns2_protocol::{classify_advert, decode_05, parse_advert, AdvertMatch, Ns2StickCal, NS2_COMPANY_ID};
use super::pad::{emit_pad_diff, PadState, BUTTON_SLOTS};

/// No decoded report for this long → the link is considered lost.
pub(crate) const WATCHDOG_LIMIT: Duration = Duration::from_millis(1000);

/// Bound on one vibration frame write. The spike measured ~12 ms per frame;
/// 100 ms is about 6 connection intervals. A write stalled on a dying link
/// blocks the stream loop, so this must stay well inside `WATCHDOG_LIMIT`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const VIB_WRITE_TIMEOUT: Duration = Duration::from_millis(100);

/// Status warning for Windows builds older than 22000 (Windows 10), whose
/// BLE stack cannot reach the 15 ms connection interval.
pub const WARN_BLE_RATE_LIMITED_WIN10: &str = "ble_rate_limited_win10";

/// First Windows 11 build.
const FIRST_UNLIMITED_BUILD: u32 = 22000;

/// A scan with no matching advert for this long is stopped and restarted.
pub(crate) const SCAN_RESTART_AFTER: Duration = Duration::from_secs(30);

/// While suspended, the worker re-checks the suspend flag this often.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const SUSPEND_POLL: Duration = Duration::from_millis(200);

/// Turn Off, exactly as hardware-proven (spike round 2): after the 06/02
/// write, wait up to this long for a response (none is expected) ...
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const TURN_OFF_RESPONSE_WAIT: Duration = Duration::from_millis(1500);
/// ... then this long before dropping the BLE link.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const TURN_OFF_SETTLE: Duration = Duration::from_millis(1000);

/// Pads worth connecting to: an NS2 Pro advert (company `0x0553`) that is
/// bonded to this adapter or in pairing mode. `local_msb_first` is the
/// adapter address as btleplug reports it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn accept_advert(
    manufacturer_data: &HashMap<u16, Vec<u8>>,
    local_msb_first: [u8; 6],
) -> Option<AdvertMatch> {
    let ad = parse_advert(manufacturer_data.get(&NS2_COMPANY_ID)?)?;
    match classify_advert(&ad, local_msb_first) {
        AdvertMatch::Other => None,
        m => Some(m),
    }
}

/// Times the gap since the last decoded report.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct Watchdog {
    last: Instant,
    limit: Duration,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Watchdog {
    pub(crate) fn new(now: Instant, limit: Duration) -> Self {
        Watchdog { last: now, limit }
    }

    pub(crate) fn feed(&mut self, now: Instant) {
        self.last = now;
    }

    pub(crate) fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last) >= self.limit
    }

    pub(crate) fn remaining(&self, now: Instant) -> Duration {
        self.limit.saturating_sub(now.saturating_duration_since(self.last))
    }
}

/// Why a connection attempt or session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum SessionEnd {
    /// No adapter, scan error, or a connect / init step failed or timed out.
    Failed,
    /// Watchdog expired or btleplug reported `DeviceDisconnected`.
    LinkLost,
    /// Manual disconnect: the pad was told to turn off.
    TurnedOff,
    /// The source was dropped.
    Stopped,
    /// A scan saw no matching advert for `SCAN_RESTART_AFTER`.
    ScanRestart,
    /// The scan stopped because another source owns the session.
    Suspended,
}

/// True once a scan started at `started` should be stopped and started
/// again (the WinRT watcher may have been aborted silently).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn scan_should_restart(started: Instant, now: Instant) -> bool {
    now.saturating_duration_since(started) >= SCAN_RESTART_AFTER
}

/// What a running scan does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum ScanStep {
    /// Keep listening for adverts.
    Listen,
    /// Stop and start the watcher again (`scan_should_restart`).
    Restart,
    /// Stop scanning: another source owns the session.
    Suspend,
}

/// The scan decision for a scan started at `started`. Suspension wins.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn scan_step(suspended: bool, started: Instant, now: Instant) -> ScanStep {
    if suspended {
        ScanStep::Suspend
    } else if scan_should_restart(started, now) {
        ScanStep::Restart
    } else {
        ScanStep::Listen
    }
}

/// Pause before scanning again. Never zero after a failure, so a pad that
/// keeps dropping (e.g. paired in Windows settings, where SMP kicks it off)
/// costs one attempt every 2 s instead of a busy loop.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn rescan_delay(end: SessionEnd) -> Duration {
    match end {
        SessionEnd::Failed | SessionEnd::LinkLost => Duration::from_secs(2),
        // Long enough that the pad we just turned off is asleep before we
        // look again (it may still advertise for a moment).
        SessionEnd::TurnedOff => Duration::from_secs(3),
        // Suspended: the worker idles until resumed, then scans at once.
        SessionEnd::Stopped | SessionEnd::ScanRestart | SessionEnd::Suspended => Duration::ZERO,
    }
}

/// True when the next scan starts from a fresh Manager / Adapter / watcher.
/// The pad may come back long after a turn-off or a lost link. A pad turned
/// off from the header then reconnected only after an app restart (which is
/// a fresh adapter); the suspect is the one WinRT watcher stopped and
/// started in place every `SCAN_RESTART_AFTER`. Re-opening is cheap.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn reopen_adapter_after(end: SessionEnd) -> bool {
    matches!(end, SessionEnd::ScanRestart | SessionEnd::TurnedOff | SessionEnd::LinkLost)
}

/// True for Windows builds whose BLE stack is rate limited (Windows 10).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn ble_rate_limited(build: u32) -> bool {
    build < FIRST_UNLIMITED_BUILD
}

/// The status warning to send when an NS2 Pro first connects, given the
/// Windows build (`None` = unknown → no warning).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn rate_limit_warning(build: Option<u32>) -> Option<&'static str> {
    build.filter(|&b| ble_rate_limited(b)).map(|_| WARN_BLE_RATE_LIMITED_WIN10)
}

/// Outcome of one input notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum Report {
    /// Too short to decode; ignored (does not feed the watchdog).
    Undecoded,
    /// Decoded; events emitted.
    Decoded,
    /// The first decoded report: `Connected(Ns2Pro)` was emitted with it.
    First,
}

/// Per-connection input state: decodes 0x05 reports, emits the event
/// diff, and keeps iron rule 11 — `Connected` on the first decoded report,
/// `Disconnected` only if `Connected` was sent.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct InputStream {
    cal: Ns2StickCal,
    announced: bool,
    /// `Disconnected` already emitted (manual disconnect finishes early).
    finished: bool,
    prev: Option<PadState>,
    prev_buttons: [bool; BUTTON_SLOTS],
    watchdog: Option<Watchdog>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl InputStream {
    pub(crate) fn new(cal: Ns2StickCal) -> Self {
        InputStream {
            cal,
            announced: false,
            finished: false,
            prev: None,
            prev_buttons: [false; BUTTON_SLOTS],
            watchdog: None,
        }
    }

    pub(crate) fn set_cal(&mut self, cal: Ns2StickCal) {
        self.cal = cal;
    }

    /// Start the watchdog (input notify just enabled).
    pub(crate) fn arm(&mut self, now: Instant) {
        self.watchdog = Some(Watchdog::new(now, WATCHDOG_LIMIT));
    }

    pub(crate) fn announced(&self) -> bool {
        self.announced
    }

    /// True once armed and no decoded report arrived for `WATCHDOG_LIMIT`.
    pub(crate) fn expired(&self, now: Instant) -> bool {
        self.watchdog.is_some_and(|w| w.expired(now))
    }

    /// Time until the watchdog fires (`None` while unarmed).
    pub(crate) fn remaining(&self, now: Instant) -> Option<Duration> {
        self.watchdog.map(|w| w.remaining(now))
    }

    /// Handle one input notification. Only a decoded report feeds the
    /// watchdog.
    pub(crate) fn on_report(&mut self, buf: &[u8], now: Instant, tx: &Sender<GamepadEvent>) -> Report {
        let Some(s) = decode_05(buf, &self.cal) else {
            return Report::Undecoded;
        };
        let first = !self.announced;
        if first {
            let _ = tx.send(GamepadEvent::Connected(ControllerKind::Ns2Pro));
            self.announced = true;
        }
        emit_pad_diff(self.prev.as_ref(), &s, &mut self.prev_buttons, tx);
        self.prev = Some(s);
        if let Some(w) = self.watchdog.as_mut() {
            w.feed(now);
        }
        if first {
            Report::First
        } else {
            Report::Decoded
        }
    }

    /// Session over: `Disconnected` iff `Connected` was sent, at most once.
    pub(crate) fn finish(&mut self, tx: &Sender<GamepadEvent>) {
        if self.announced && !self.finished {
            let _ = tx.send(GamepadEvent::Disconnected);
            self.finished = true;
        }
    }
}

/// How long the streaming select may sleep: until the watchdog fires, the
/// next rumble step is due (zero if overdue), or `tick`, whichever is first.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn next_wait(
    now: Instant,
    watchdog_left: Option<Duration>,
    rumble_due: Option<Instant>,
    tick: Duration,
) -> Duration {
    let rumble_left = rumble_due.map(|t| t.saturating_duration_since(now));
    [watchdog_left, rumble_left].into_iter().flatten().fold(tick, Duration::min)
}

// ─── Windows: btleplug worker ──────────────────────────────────────────────

#[cfg(windows)]
pub struct Ns2BleSource {
    rx: Receiver<GamepadEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    suspend: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(windows)]
impl Ns2BleSource {
    /// Spawn the BLE worker thread. It emits nothing until a pad's first
    /// decoded report. `cmd_rx` carries the hub's commands: manual disconnect
    /// requests (ignored while no pad is connected) and rumble requests; `warn_tx` receives status
    /// warnings (`WARN_BLE_RATE_LIMITED_WIN10`, at most once per process,
    /// when an NS2 Pro first connects on Windows 10).
    pub fn new(cmd_rx: Receiver<SourceCmd>, warn_tx: Sender<String>) -> anyhow::Result<Self> {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let (tx, rx) = crossbeam_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let suspend = Arc::new(AtomicBool::new(false));
        let ctx = imp::Ctx {
            tx,
            stop: stop.clone(),
            suspend: suspend.clone(),
            cmd_rx,
            warn_tx,
            build: None,
        };
        std::thread::Builder::new()
            .name("ns2-ble".into())
            .spawn(move || imp::run(ctx))
            .map_err(|e| anyhow::anyhow!("spawning NS2 BLE worker thread: {e}"))?;
        Ok(Self { rx, stop, suspend })
    }

    /// Drain pending events into `out`. Non-blocking.
    pub fn poll(&mut self, out: &mut Vec<GamepadEvent>) {
        out.extend(self.rx.try_iter());
    }

    /// While set, the worker neither scans nor connects (a pad already
    /// streaming is not affected). Set by `SourceHub` while another source
    /// owns the session.
    pub fn suspend_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.suspend.clone()
    }
}

#[cfg(windows)]
impl Drop for Ns2BleSource {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::gamepad::ns2_protocol::{
        command, expected_b2, is_response_to, ltk, pairing_addresses, pairing_key_cmd,
        pairing_response_key, parse_stick_cal, spi_read_cmd, spi_read_payload, PairingKeys, CMD_LED,
        CMD_PAIR, CMD_POWER, SPI_CAL_LEN, SPI_LEFT_STICK_CAL, SPI_RIGHT_STICK_CAL, SUB_LED_SET,
        SUB_PAIR_A1, SUB_PAIR_A2, SUB_PAIR_ADDR, SUB_PAIR_COMMIT, SUB_POWER_OFF,
    };
    use crate::gamepad::rumble::{ns2_vibration_frame, RumblePlan, Step, NS2_BUZZ_OP, NS2_STOP_OP};
    use anyhow::{anyhow, bail, Context, Result};
    use btleplug::api::{
        Central, CentralEvent, Characteristic, ConnectionParameterPreset, Manager as _,
        Peripheral as _, ScanFilter, ValueNotification, WriteType,
    };
    use btleplug::platform::{Adapter, Manager, Peripheral};
    use futures::{Stream, StreamExt};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Input report 0x05 notify.
    const UUID_INPUT: u128 = 0xab7de9be_89fe_49ad_828f_118f09df7fd2;
    /// Command write-without-response.
    const UUID_CMD: u128 = 0x649d4ac9_8eb7_4e6c_af44_1ea54fe5f005;
    /// Command response notify.
    const UUID_RESP: u128 = 0xc765a961_d9d8_4d36_a20a_5315b111836a;
    /// Vibration write-without-response (rumble frames).
    const UUID_VIB: u128 = 0xcc483f51_9258_427d_a939_630c31f72b05;

    /// Upper bound on any wait before the stop flag is re-checked.
    const TICK: Duration = Duration::from_millis(100);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);
    const GATT_OP_TIMEOUT: Duration = Duration::from_secs(5);
    const RESPONSE_TIMEOUT: Duration = Duration::from_millis(1500);
    /// After the connection-parameter request (hardware-proven value).
    const PARAMS_SETTLE: Duration = Duration::from_millis(500);
    const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
    const ADAPTER_RETRY: Duration = Duration::from_secs(2);
    const LED_PLAYER_1: [u8; 8] = [0x01, 0, 0, 0, 0, 0, 0, 0];

    type Events = Pin<Box<dyn Stream<Item = CentralEvent> + Send>>;
    type Notes = Pin<Box<dyn Stream<Item = ValueNotification> + Send>>;

    pub(super) struct Ctx {
        pub(super) tx: Sender<GamepadEvent>,
        pub(super) stop: Arc<AtomicBool>,
        pub(super) suspend: Arc<AtomicBool>,
        pub(super) cmd_rx: Receiver<SourceCmd>,
        pub(super) warn_tx: Sender<String>,
        /// Windows build, read once at thread start.
        pub(super) build: Option<u32>,
    }

    /// The Win10 warning goes out at most once per process.
    static RATE_WARNED: AtomicBool = AtomicBool::new(false);

    impl Ctx {
        fn stopped(&self) -> bool {
            self.stop.load(Ordering::SeqCst)
        }

        fn suspended(&self) -> bool {
            self.suspend.load(Ordering::SeqCst)
        }

        /// An NS2 Pro just connected: warn if this Windows rate-limits BLE.
        fn on_first_connect(&self) {
            if let Some(w) = rate_limit_warning(self.build) {
                if !RATE_WARNED.swap(true, Ordering::SeqCst) {
                    tracing::warn!(
                        build = ?self.build,
                        "Windows 10 BLE stack: Switch 2 Pro report rate is limited"
                    );
                    let _ = self.warn_tx.try_send(w.to_string());
                }
            }
        }

        /// A command (manual disconnect or rumble) while no pad is
        /// connected is a no-op.
        fn drain_commands(&self) {
            while self.cmd_rx.try_recv().is_ok() {}
        }
    }

    /// Windows build number via `RtlGetVersion` (not subject to the
    /// manifest-based version lie of `GetVersionEx`).
    pub(super) fn windows_build() -> Option<u32> {
        use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
        use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
        // SAFETY: OSVERSIONINFOW is plain data; RtlGetVersion fills the
        // struct whose size field we set, and always succeeds for it.
        let mut v: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        v.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        let status = unsafe { RtlGetVersion(&mut v) };
        (status == 0).then_some(v.dwBuildNumber)
    }

    /// Thread body.
    pub(super) fn run(mut ctx: Ctx) {
        ctx.build = windows_build();
        if ctx.build.is_none() {
            tracing::warn!("could not read the Windows build number");
        }
        let rt = match tokio::runtime::Builder::new_current_thread().enable_time().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "NS2 BLE: tokio runtime init failed");
                return;
            }
        };
        rt.block_on(worker(&ctx));
        tracing::info!("NS2 BLE worker stopped");
    }

    /// Adapter handle kept across sessions; re-opened after any error.
    struct Ble {
        adapter: Adapter,
        events: Events,
        /// Adapter address, MSB-first (btleplug order).
        local: [u8; 6],
    }

    /// `quiet`: the routine re-open after a scan restart logs at debug level.
    async fn open_ble(quiet: bool) -> Result<Ble> {
        let manager = Manager::new().await.context("BLE manager")?;
        let adapter = manager
            .adapters()
            .await
            .context("listing BLE adapters")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no Bluetooth adapter"))?;
        let addr = adapter
            .adapter_address()
            .await
            .context("adapter address")?
            .ok_or_else(|| anyhow!("adapter address unavailable"))?;
        let events = adapter.events().await.context("adapter events")?;
        if quiet {
            tracing::debug!(adapter = %addr, "NS2 BLE: adapter re-opened; scanning");
        } else {
            tracing::info!(adapter = %addr, "NS2 BLE: scanning for a Switch 2 Pro");
        }
        Ok(Ble { adapter, events, local: addr.into_inner() })
    }

    async fn worker(ctx: &Ctx) {
        let mut ble: Option<Ble> = None;
        // Repeated identical failures (Bluetooth off, a pad that keeps
        // dropping) are logged once, then at debug level.
        let mut last_err = String::new();
        let mut quiet = false;
        while !ctx.stopped() {
            if ctx.suspended() {
                // Another controller owns the session: leave the radio alone.
                pause(ctx, SUSPEND_POLL).await;
                continue;
            }
            if ble.is_none() {
                match open_ble(quiet).await {
                    Ok(b) => ble = Some(b),
                    Err(e) => {
                        note_failure(&mut last_err, &e);
                        pause(ctx, ADAPTER_RETRY).await;
                        continue;
                    }
                }
            }
            let b = ble.as_mut().expect("opened above");
            let end = match session(ctx, b).await {
                Ok(end) => {
                    // A pad streamed: the next failure is news again.
                    if matches!(end, SessionEnd::LinkLost | SessionEnd::TurnedOff) {
                        last_err.clear();
                    }
                    end
                }
                Err(f) if ctx.stopped() => {
                    tracing::debug!(error = %format!("{:#}", f.err), "NS2 BLE: stopped mid-step");
                    SessionEnd::Stopped
                }
                Err(f) => {
                    note_failure(&mut last_err, &f.err);
                    if f.adapter {
                        ble = None;
                    }
                    SessionEnd::Failed
                }
            };
            tracing::debug!(?end, "NS2 BLE session ended");
            if reopen_adapter_after(end) {
                ble = None;
            }
            quiet = end == SessionEnd::ScanRestart;
            pause(ctx, rescan_delay(end)).await;
        }
    }

    fn note_failure(last: &mut String, e: &anyhow::Error) {
        let msg = format!("{e:#}");
        if *last != msg {
            tracing::warn!(error = %msg, "NS2 BLE: attempt failed; retrying every 2 s");
            *last = msg;
        } else {
            tracing::debug!(error = %msg, "NS2 BLE: attempt failed again");
        }
    }

    /// Sleep `d`, waking every `TICK` for the stop flag. No announced pad
    /// is streaming meanwhile, so manual disconnects are drained.
    async fn pause(ctx: &Ctx, d: Duration) {
        let end = Instant::now() + d;
        while !ctx.stopped() {
            ctx.drain_commands();
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(left.min(TICK)).await;
        }
    }

    /// Await `fut` for at most `limit`, giving up early when stopped.
    async fn bounded<F: Future>(ctx: &Ctx, limit: Duration, what: &str, fut: F) -> Result<F::Output> {
        tokio::pin!(fut);
        let end = Instant::now() + limit;
        loop {
            if ctx.stopped() {
                bail!("{what}: stopped");
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("{what}: timed out after {limit:?}");
            }
            if let Ok(v) = tokio::time::timeout(left.min(TICK), &mut fut).await {
                return Ok(v);
            }
        }
    }

    /// A session that failed before its pad was announced.
    struct Fault {
        /// Scanning failed: re-open the adapter before the next attempt.
        adapter: bool,
        err: anyhow::Error,
    }

    /// One scan → connect → stream cycle.
    async fn session(ctx: &Ctx, ble: &mut Ble) -> std::result::Result<SessionEnd, Fault> {
        let (p, m) = match scan(ctx, ble).await.map_err(|err| Fault { adapter: true, err })? {
            Scan::Found(p, m) => (p, m),
            Scan::Stopped => return Ok(SessionEnd::Stopped),
            Scan::Suspended => {
                tracing::debug!("NS2 BLE: another controller owns the session; scan suspended");
                return Ok(SessionEnd::Suspended);
            }
            Scan::Restart => {
                tracing::debug!("NS2 BLE: no advert for {SCAN_RESTART_AFTER:?}; restarting the scan");
                return Ok(SessionEnd::ScanRestart);
            }
        };
        tracing::info!(addr = %p.address(), mode = ?m, "NS2 Pro advert accepted; connecting");
        let mut input = InputStream::new(Ns2StickCal::default());
        let result = drive(ctx, ble, &p, m, &mut input).await;
        if !matches!(result, Ok(SessionEnd::TurnedOff)) {
            // Every other exit leaves the link up (or half up): drop it.
            if !matches!(tokio::time::timeout(CLEANUP_TIMEOUT, p.disconnect()).await, Ok(Ok(()))) {
                tracing::debug!("NS2 BLE: disconnect after session end failed or timed out");
            }
        }
        input.finish(&ctx.tx);
        match &result {
            Ok(end) => tracing::info!(?end, connected = input.announced(), "NS2 Pro session ended"),
            Err(e) if input.announced() => {
                tracing::warn!(error = %format!("{e:#}"), "NS2 Pro link failed")
            }
            Err(_) => {}
        }
        // A pad that streamed and then failed is a lost link, not a
        // connect failure.
        match result {
            Ok(end) => Ok(end),
            Err(_) if input.announced() => Ok(SessionEnd::LinkLost),
            Err(err) => Err(Fault { adapter: false, err }),
        }
    }

    enum Scan {
        Found(Peripheral, AdvertMatch),
        Stopped,
        /// `SCAN_RESTART_AFTER` without a matching advert.
        Restart,
        /// The suspend flag was set (checked again after a match, so a
        /// pad found in the same instant is not connected).
        Suspended,
    }

    /// One bounded scan: until an acceptable advert, a stop, a suspend, or
    /// `SCAN_RESTART_AFTER`. The watcher is always stopped on exit, so a
    /// restart is a fresh `start_scan` (btleplug never surfaces a watcher
    /// that WinRT aborted on Bluetooth off/on or sleep/wake).
    async fn scan(ctx: &Ctx, ble: &mut Ble) -> Result<Scan> {
        ble.adapter.start_scan(ScanFilter::default()).await.context("start_scan")?;
        let found = scan_until_match(ctx, ble).await;
        let _ = ble.adapter.stop_scan().await;
        found
    }

    async fn scan_until_match(ctx: &Ctx, ble: &mut Ble) -> Result<Scan> {
        let started = Instant::now();
        loop {
            // Checked every iteration: adverts from nearby devices can
            // arrive faster than `TICK`.
            if ctx.stopped() {
                return Ok(Scan::Stopped);
            }
            match scan_step(ctx.suspended(), started, Instant::now()) {
                ScanStep::Listen => {}
                ScanStep::Restart => return Ok(Scan::Restart),
                ScanStep::Suspend => return Ok(Scan::Suspended),
            }
            ctx.drain_commands();
            let ev = match tokio::time::timeout(TICK, ble.events.next()).await {
                Err(_) => continue,
                Ok(None) => bail!("adapter event stream closed"),
                Ok(Some(ev)) => ev,
            };
            if let CentralEvent::ManufacturerDataAdvertisement { id, manufacturer_data } = ev {
                if let Some(m) = accept_advert(&manufacturer_data, ble.local) {
                    if ctx.suspended() {
                        return Ok(Scan::Suspended);
                    }
                    let p = ble.adapter.peripheral(&id).await.context("peripheral lookup")?;
                    return Ok(Scan::Found(p, m));
                }
            }
        }
    }

    struct Link<'a> {
        p: &'a Peripheral,
        cmd: Characteristic,
        input: Characteristic,
        /// Vibration characteristic; `None` → no rumble this session.
        vib: Option<Characteristic>,
        notes: Notes,
    }

    impl Link<'_> {
        /// Write a command (without response) and wait for the response
        /// notification `done` accepts. Other notifications are skipped.
        async fn request(
            &mut self,
            ctx: &Ctx,
            pkt: &[u8],
            done: impl Fn(&[u8]) -> bool,
        ) -> Result<Vec<u8>> {
            self.request_within(ctx, pkt, RESPONSE_TIMEOUT, done).await
        }

        async fn request_within(
            &mut self,
            ctx: &Ctx,
            pkt: &[u8],
            wait: Duration,
            done: impl Fn(&[u8]) -> bool,
        ) -> Result<Vec<u8>> {
            let (cmd, sub) = (pkt[0], pkt[3]);
            self.send(ctx, pkt).await?;
            let end = Instant::now() + wait;
            loop {
                if ctx.stopped() {
                    bail!("command {cmd:#04x}/{sub:#04x}: stopped");
                }
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    bail!("no response to command {cmd:#04x}/{sub:#04x}");
                }
                match tokio::time::timeout(left.min(TICK), self.notes.next()).await {
                    Ok(Some(n)) if n.uuid.as_u128() == UUID_RESP && done(&n.value) => {
                        return Ok(n.value)
                    }
                    Ok(Some(_)) | Err(_) => {}
                    Ok(None) => bail!("notification stream closed"),
                }
            }
        }

        async fn send(&self, ctx: &Ctx, pkt: &[u8]) -> Result<()> {
            let write = self.p.write(&self.cmd, pkt, WriteType::WithoutResponse);
            bounded(ctx, GATT_OP_TIMEOUT, "command write", write).await?.context("command write")
        }

        /// Write one vibration frame (without response). Never a request:
        /// no response is expected and input must keep flowing.
        async fn vibrate(&self, ctx: &Ctx, frame: &[u8]) -> Result<()> {
            let Some(vib) = &self.vib else { bail!("no vibration characteristic") };
            let write = self.p.write(vib, frame, WriteType::WithoutResponse);
            bounded(ctx, VIB_WRITE_TIMEOUT, "vibration write", write).await?.context("vibration write")
        }

        /// The session is ending on purpose (manual disconnect, app stop):
        /// a pad mid-buzz gets one stop frame, best effort. Not cut short
        /// by the stop flag (unlike `vibrate`), or an app stop could never
        /// send it; bounded by `VIB_WRITE_TIMEOUT` like every frame.
        async fn stop_rumble(&self, plan: &mut Option<RumblePlan>, seq: &mut u8) {
            let (Some(Step::Stop), Some(vib)) = (plan.take().and_then(|mut p| p.abort()), &self.vib)
            else {
                return;
            };
            let frame = ns2_vibration_frame(*seq, NS2_STOP_OP);
            *seq = seq.wrapping_add(1);
            let write = self.p.write(vib, &frame, WriteType::WithoutResponse);
            let sent = matches!(tokio::time::timeout(VIB_WRITE_TIMEOUT, write).await, Ok(Ok(())));
            tracing::info!(kind = ?ControllerKind::Ns2Pro, sent, "rumble: stopped early");
        }
    }

    /// Connect, initialize, stream. Returns how the session ended.
    async fn drive(
        ctx: &Ctx,
        ble: &mut Ble,
        p: &Peripheral,
        m: AdvertMatch,
        input: &mut InputStream,
    ) -> Result<SessionEnd> {
        // ── Connecting (order is hardware-proven, spec §12) ──
        bounded(ctx, CONNECT_TIMEOUT, "connect", p.connect()).await?.context("connect")?;
        let preset = ConnectionParameterPreset::ThroughputOptimized;
        if let Err(e) = p.request_connection_parameters(preset).await {
            tracing::warn!(error = %e, "NS2 BLE: connection-parameter request failed");
        }
        pause(ctx, PARAMS_SETTLE).await;
        bounded(ctx, DISCOVER_TIMEOUT, "discover_services", p.discover_services())
            .await?
            .context("discover_services")?;
        let chars = p.characteristics();
        let find = |u: u128| {
            chars
                .iter()
                .find(|c| c.uuid.as_u128() == u)
                .cloned()
                .ok_or_else(|| anyhow!("characteristic {u:032x} missing"))
        };
        let (cmd, input_char, resp) = (find(UUID_CMD)?, find(UUID_INPUT)?, find(UUID_RESP)?);
        let vib = chars.iter().find(|c| c.uuid.as_u128() == UUID_VIB).cloned();
        if vib.is_none() {
            tracing::info!("NS2 BLE: vibration characteristic not found; rumble off");
        }
        // Take the notification stream before enabling notify so no
        // response is missed.
        let notes = p.notifications().await.context("notification stream")?;
        let mut link = Link { p, cmd, input: input_char, vib, notes };
        bounded(ctx, GATT_OP_TIMEOUT, "subscribe response", p.subscribe(&resp))
            .await?
            .context("subscribe response")?;

        // ── Initializing ──
        if m == AdvertMatch::PairingMode {
            pair(ctx, &mut link, ble.local).await?;
        }
        input.set_cal(read_stick_cal(ctx, &mut link).await);
        let led = command(CMD_LED, SUB_LED_SET, &LED_PLAYER_1);
        if let Err(e) = link.request(ctx, &led, |r| is_response_to(r, CMD_LED, SUB_LED_SET)).await {
            if ctx.stopped() {
                return Err(e);
            }
            tracing::warn!(error = %format!("{e:#}"), "NS2 BLE: player LED not acknowledged");
        }
        bounded(ctx, GATT_OP_TIMEOUT, "subscribe input", p.subscribe(&link.input))
            .await?
            .context("subscribe input")?;

        // ── Streaming ──
        // A request made before this pad was announced is not for it.
        ctx.drain_commands();
        input.arm(Instant::now());
        stream(ctx, ble, &mut link, input).await
    }

    async fn stream(
        ctx: &Ctx,
        ble: &mut Ble,
        link: &mut Link<'_>,
        input: &mut InputStream,
    ) -> Result<SessionEnd> {
        let id = link.p.id();
        let mut plan: Option<RumblePlan> = None;
        // Vibration frame sequence; wraps (only the low nibble is sent).
        let mut seq: u8 = 0;
        loop {
            // Checked every iteration: reports arrive every ~15 ms.
            if ctx.stopped() {
                link.stop_rumble(&mut plan, &mut seq).await;
                return Ok(SessionEnd::Stopped);
            }
            // One command per iteration. A `Rumble` (re)starts the buzz and
            // never ends the session; a `Disconnect` before the pad
            // announced is dropped.
            match ctx.cmd_rx.try_recv() {
                Ok(SourceCmd::Disconnect) if input.announced() => {
                    // Release held keys now, before the stop frame and the
                    // ~2.5 s turn-off.
                    input.finish(&ctx.tx);
                    link.stop_rumble(&mut plan, &mut seq).await;
                    turn_off(ctx, link).await;
                    return Ok(SessionEnd::TurnedOff);
                }
                Ok(SourceCmd::Rumble) if link.vib.is_some() => {
                    let now = Instant::now();
                    plan = Some(match plan.take() {
                        Some(old) => old.restarted(now),
                        None => RumblePlan::new(ControllerKind::Ns2Pro, now),
                    });
                    tracing::info!(kind = ?ControllerKind::Ns2Pro, "rumble: start");
                }
                Ok(_) | Err(_) => {}
            }
            let now = Instant::now();
            if input.expired(now) {
                tracing::info!("NS2 Pro: no report for {WATCHDOG_LIMIT:?}");
                return Ok(SessionEnd::LinkLost);
            }
            // One frame per iteration, so notifications are drained between
            // frames even when writes are slow (an overdue step makes the
            // wait zero).
            play_rumble(ctx, link, &mut plan, &mut seq).await;
            // Wake for the next rumble step too, never later than the watchdog.
            let now = Instant::now();
            let rumble_due = plan.as_ref().and_then(RumblePlan::next_due);
            let wait = next_wait(now, input.remaining(now), rumble_due, TICK);
            tokio::select! {
                n = link.notes.next() => match n {
                    Some(n) if n.uuid.as_u128() == UUID_INPUT => {
                        if input.on_report(&n.value, Instant::now(), &ctx.tx) == Report::First {
                            ctx.on_first_connect();
                        }
                    }
                    Some(_) => {}
                    None => bail!("notification stream closed"),
                },
                ev = ble.events.next() => match ev {
                    Some(CentralEvent::DeviceDisconnected(d)) if d == id => {
                        tracing::info!("NS2 Pro: device disconnected");
                        return Ok(SessionEnd::LinkLost);
                    }
                    Some(_) => {}
                    None => bail!("adapter event stream closed"),
                },
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Write at most one due rumble step. A write error skips the rest of
    /// the buzz; the stops are still tried. While the app is stopping the
    /// plan is kept as is, so the stop check can still send its stop frame.
    async fn play_rumble(ctx: &Ctx, link: &Link<'_>, plan: &mut Option<RumblePlan>, seq: &mut u8) {
        let Some(p) = plan.as_mut() else { return };
        if let Some(step) = p.take_due(Instant::now()) {
            let op = if step == Step::Buzz { NS2_BUZZ_OP } else { NS2_STOP_OP };
            let frame = ns2_vibration_frame(*seq, op);
            *seq = seq.wrapping_add(1);
            if let Err(e) = link.vibrate(ctx, &frame).await {
                p.on_write_failed();
                if ctx.stopped() {
                    return;
                }
                tracing::warn!(error = %format!("{e:#}"), ?step, "rumble write failed; dropping it");
            }
        }
        if p.finished() {
            tracing::info!(kind = ?ControllerKind::Ns2Pro, "rumble: done");
            *plan = None;
        }
    }

    /// Manual disconnect, replicating the hardware-proven spike exactly:
    /// write 0x06/0x02, wait up to `TURN_OFF_RESPONSE_WAIT` for a response
    /// (none is the expected outcome), sleep `TURN_OFF_SETTLE`, then drop
    /// the BLE link. Best effort; the link goes down either way.
    async fn turn_off(ctx: &Ctx, link: &mut Link<'_>) {
        tracing::info!("NS2 Pro: manual disconnect — turning the pad off");
        let off = command(CMD_POWER, SUB_POWER_OFF, &[0u8; 12]);
        let done = |r: &[u8]| is_response_to(r, CMD_POWER, SUB_POWER_OFF);
        match link.request_within(ctx, &off, TURN_OFF_RESPONSE_WAIT, done).await {
            Ok(_) => tracing::debug!("NS2 Pro: turn-off acknowledged"),
            Err(e) => {
                tracing::debug!(error = %format!("{e:#}"), "NS2 Pro: turn-off not acknowledged (expected)")
            }
        }
        pause(ctx, TURN_OFF_SETTLE).await;
        match tokio::time::timeout(CLEANUP_TIMEOUT, link.p.disconnect()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "NS2 BLE: disconnect failed"),
            Err(_) => tracing::warn!("NS2 BLE: disconnect timed out"),
        }
    }

    /// The `0x15` pairing (pairing-mode adverts only). A B2 that does not
    /// match our computation is logged and the pairing still committed.
    async fn pair(ctx: &Ctx, link: &mut Link<'_>, local: [u8; 6]) -> Result<()> {
        tracing::info!("NS2 Pro in pairing mode: pairing with this adapter");
        let addr = command(CMD_PAIR, SUB_PAIR_ADDR, &pairing_addresses(local));
        link.request(ctx, &addr, |r| is_response_to(r, CMD_PAIR, SUB_PAIR_ADDR)).await?;
        let keys = PairingKeys::random();
        let a1 = pairing_key_cmd(SUB_PAIR_A1, &keys.a1);
        let r = link.request(ctx, &a1, |r| is_response_to(r, CMD_PAIR, SUB_PAIR_A1)).await?;
        let b1 = pairing_response_key(&r, SUB_PAIR_A1);
        let a2 = pairing_key_cmd(SUB_PAIR_A2, &keys.a2);
        let r = link.request(ctx, &a2, |r| is_response_to(r, CMD_PAIR, SUB_PAIR_A2)).await?;
        let b2 = pairing_response_key(&r, SUB_PAIR_A2);
        match (b1, b2) {
            (Some(b1), Some(b2)) if expected_b2(&ltk(&keys.a1, &b1), &keys.a2) == b2 => {
                tracing::info!("NS2 Pro pairing: B2 verified")
            }
            _ => tracing::warn!("NS2 Pro pairing: B2 mismatch or missing; committing anyway"),
        }
        let commit = command(CMD_PAIR, SUB_PAIR_COMMIT, &[0x00]);
        link.request(ctx, &commit, |r| is_response_to(r, CMD_PAIR, SUB_PAIR_COMMIT)).await?;
        tracing::info!("NS2 Pro pairing committed");
        Ok(())
    }

    /// Factory stick calibration. A missing or malformed block keeps the
    /// defaults (centre 2048, span 1400) for that stick.
    async fn read_stick_cal(ctx: &Ctx, link: &mut Link<'_>) -> Ns2StickCal {
        let mut blocks: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
        for (i, addr) in [SPI_LEFT_STICK_CAL, SPI_RIGHT_STICK_CAL].into_iter().enumerate() {
            let pkt = spi_read_cmd(addr, SPI_CAL_LEN);
            match link.request(ctx, &pkt, |r| spi_read_payload(r, addr).is_some()).await {
                Ok(r) => {
                    blocks[i] = spi_read_payload(&r, addr).map(<[u8]>::to_vec).unwrap_or_default()
                }
                Err(_) if ctx.stopped() => break,
                Err(e) => tracing::warn!(
                    addr = format_args!("{addr:#x}"),
                    error = %format!("{e:#}"),
                    "NS2 Pro: stick calibration unreadable; using defaults"
                ),
            }
        }
        parse_stick_cal(&blocks[0], &blocks[1])
    }
}

// ─── Other platforms: stub ─────────────────────────────────────────────────

/// Non-Windows stand-in: never connects, never warns. Commands (manual
/// disconnect, rumble) are drained so they do not pile up.
#[cfg(not(windows))]
pub struct Ns2BleSource {
    cmd_rx: Receiver<SourceCmd>,
    suspend: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(windows))]
impl Ns2BleSource {
    pub fn new(cmd_rx: Receiver<SourceCmd>, _warn_tx: Sender<String>) -> anyhow::Result<Self> {
        Ok(Self { cmd_rx, suspend: Default::default() })
    }

    pub fn poll(&mut self, _out: &mut Vec<GamepadEvent>) {
        while self.cmd_rx.try_recv().is_ok() {}
    }

    /// Stored only; the stub never scans.
    pub fn suspend_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.suspend.clone()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::events::GamepadEvent;
    use crate::gamepad::ns2_protocol::{AdvertMatch, Ns2StickCal, NS2_COMPANY_ID};
    use crate::gamepad::ControllerKind;
    use crossbeam_channel::unbounded;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// Local adapter of the spike runs (34:FD:70:A3:21:85).
    const LOCAL: [u8; 6] = [0x34, 0xFD, 0x70, 0xA3, 0x21, 0x85];
    const ADV_PAIRING: &str = "01 00 03 7e 05 69 20 00 01 00 00 00 00 00 00 00 0f 00 00 00 00 00 00 00";
    const ADV_BONDED: &str = "01 00 03 7e 05 69 20 00 01 00 85 21 a3 70 fd 34 0f 00 00 00 00 00 00 00";
    const R2_FIRST_REPORT: &str = "69 0d 00 00 00 00 00 00 00 00 ef 27 86 9d 88 7d 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 1d 0e 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00";

    fn mfr(company: u16, data: &str) -> HashMap<u16, Vec<u8>> {
        HashMap::from([(company, hex(data))])
    }

    #[test]
    fn advert_accepts_pairing_mode_and_bonded_to_us() {
        assert_eq!(
            accept_advert(&mfr(NS2_COMPANY_ID, ADV_PAIRING), LOCAL),
            Some(AdvertMatch::PairingMode)
        );
        assert_eq!(accept_advert(&mfr(NS2_COMPANY_ID, ADV_BONDED), LOCAL), Some(AdvertMatch::Bonded));
        // Other manufacturers' entries alongside ours do not matter.
        let mut both = mfr(NS2_COMPANY_ID, ADV_BONDED);
        both.insert(0x004C, vec![0x02, 0x15]);
        assert_eq!(accept_advert(&both, LOCAL), Some(AdvertMatch::Bonded));
    }

    #[test]
    fn advert_rejects_everything_else() {
        // Bonded to another host.
        let mut other = LOCAL;
        other[5] = 0x86;
        assert_eq!(accept_advert(&mfr(NS2_COMPANY_ID, ADV_BONDED), other), None);
        // Same bytes under another company id.
        assert_eq!(accept_advert(&mfr(0x004C, ADV_PAIRING), LOCAL), None);
        // No manufacturer data at all.
        assert_eq!(accept_advert(&HashMap::new(), LOCAL), None);
        // Truncated before the host field.
        let short = hex(ADV_PAIRING)[..15].to_vec();
        assert_eq!(accept_advert(&HashMap::from([(NS2_COMPANY_ID, short)]), LOCAL), None);
        // Joy-Con 2 R (PID 0x2067) in pairing mode.
        let mut jc = hex(ADV_PAIRING);
        jc[5] = 0x67;
        assert_eq!(accept_advert(&HashMap::from([(NS2_COMPANY_ID, jc)]), LOCAL), None);
    }

    #[test]
    fn watchdog_expires_at_limit_and_feed_resets() {
        let t0 = Instant::now();
        let mut w = Watchdog::new(t0, WATCHDOG_LIMIT);
        assert_eq!(WATCHDOG_LIMIT, Duration::from_millis(1000));
        assert!(!w.expired(t0));
        assert!(!w.expired(t0 + Duration::from_millis(999)));
        assert!(w.expired(t0 + Duration::from_millis(1000)));
        assert_eq!(w.remaining(t0 + Duration::from_millis(400)), Duration::from_millis(600));
        assert_eq!(w.remaining(t0 + Duration::from_millis(1500)), Duration::ZERO);
        w.feed(t0 + Duration::from_millis(900));
        assert!(!w.expired(t0 + Duration::from_millis(1800)));
        assert!(w.expired(t0 + Duration::from_millis(1900)));
    }

    #[test]
    fn vibration_write_bound_stays_well_inside_the_watchdog() {
        // A stalled frame write blocks the stream loop; it must never
        // stretch past the 1000 ms liveness bound (iron rule 11).
        assert_eq!(VIB_WRITE_TIMEOUT, Duration::from_millis(100));
        assert!(VIB_WRITE_TIMEOUT * 5 < WATCHDOG_LIMIT);
    }

    #[test]
    fn next_wait_takes_the_earliest_deadline() {
        let now = Instant::now();
        let tick = Duration::from_millis(100);
        assert_eq!(next_wait(now, None, None, tick), tick);
        assert_eq!(next_wait(now, Some(Duration::from_millis(40)), None, tick), Duration::from_millis(40));
        assert_eq!(
            next_wait(now, Some(Duration::from_millis(40)), Some(now + Duration::from_millis(12)), tick),
            Duration::from_millis(12)
        );
        assert_eq!(next_wait(now + Duration::from_millis(5), None, Some(now), tick), Duration::ZERO); // overdue
    }

    #[test]
    fn rescan_delays() {
        assert_eq!(rescan_delay(SessionEnd::Failed), Duration::from_secs(2));
        assert_eq!(rescan_delay(SessionEnd::LinkLost), Duration::from_secs(2));
        // A turned-off pad must not be re-grabbed while it goes to sleep.
        assert_eq!(rescan_delay(SessionEnd::TurnedOff), Duration::from_secs(3));
        assert_eq!(rescan_delay(SessionEnd::Stopped), Duration::ZERO);
        // A routine scan restart is not a failure: no pause.
        assert_eq!(rescan_delay(SessionEnd::ScanRestart), Duration::ZERO);
    }

    #[test]
    fn long_waits_reopen_the_adapter() {
        // The pad may come back long after a turn-off or a lost link: these
        // start from a fresh adapter, as an app restart does.
        assert!(reopen_adapter_after(SessionEnd::ScanRestart));
        assert!(reopen_adapter_after(SessionEnd::TurnedOff));
        assert!(reopen_adapter_after(SessionEnd::LinkLost));
        // Connect failures keep the adapter (a scan error already drops it),
        // and so do the ends that do not scan again at once.
        assert!(!reopen_adapter_after(SessionEnd::Failed));
        assert!(!reopen_adapter_after(SessionEnd::Stopped));
        assert!(!reopen_adapter_after(SessionEnd::Suspended));
    }

    #[test]
    fn scan_restarts_every_30_s() {
        assert_eq!(SCAN_RESTART_AFTER, Duration::from_secs(30));
        let t0 = Instant::now();
        assert!(!scan_should_restart(t0, t0));
        assert!(!scan_should_restart(t0, t0 + Duration::from_millis(29_999)));
        assert!(scan_should_restart(t0, t0 + Duration::from_secs(30)));
        assert!(scan_should_restart(t0, t0 + Duration::from_secs(300)));
    }

    #[test]
    fn scan_step_suspend_wins_over_restart() {
        let t0 = Instant::now();
        let late = t0 + SCAN_RESTART_AFTER;
        assert_eq!(scan_step(false, t0, t0), ScanStep::Listen);
        assert_eq!(scan_step(false, t0, late), ScanStep::Restart);
        // Another source owns the session: stop scanning at once.
        assert_eq!(scan_step(true, t0, t0), ScanStep::Suspend);
        assert_eq!(scan_step(true, t0, late), ScanStep::Suspend);
    }

    #[test]
    fn suspended_scan_resumes_without_pause() {
        // The worker idles in SUSPEND_POLL steps until resumed; once
        // resumed it scans again at once.
        assert_eq!(SUSPEND_POLL, Duration::from_millis(200));
        assert_eq!(rescan_delay(SessionEnd::Suspended), Duration::ZERO);
    }

    #[test]
    fn turn_off_timing_matches_spike() {
        // Spike round 2: 06/02 TX, "no response" after 1.5 s, 1000 ms, disconnect.
        assert_eq!(TURN_OFF_RESPONSE_WAIT, Duration::from_millis(1500));
        assert_eq!(TURN_OFF_SETTLE, Duration::from_millis(1000));
    }

    #[test]
    fn rate_limit_warning_only_for_known_win10_builds() {
        assert_eq!(rate_limit_warning(Some(19045)), Some(WARN_BLE_RATE_LIMITED_WIN10));
        assert_eq!(rate_limit_warning(Some(22631)), None);
        assert_eq!(rate_limit_warning(None), None, "unknown build: no warning");
    }

    #[test]
    fn win10_builds_are_rate_limited() {
        assert!(ble_rate_limited(19045)); // Windows 10 22H2
        assert!(ble_rate_limited(21999));
        assert!(!ble_rate_limited(22000)); // Windows 11 21H2
        assert!(!ble_rate_limited(26200)); // the spike machine
    }

    #[test]
    fn input_stream_connects_on_first_decoded_report() {
        let (tx, rx) = unbounded();
        let t0 = Instant::now();
        let mut s = InputStream::new(Ns2StickCal::default());
        s.arm(t0);
        assert!(!s.announced());

        let mut report = hex(R2_FIRST_REPORT);
        assert_eq!(s.on_report(&report, t0 + Duration::from_millis(15), &tx), Report::First);
        let evs: Vec<GamepadEvent> = rx.try_iter().collect();
        assert_eq!(evs[0], GamepadEvent::Connected(ControllerKind::Ns2Pro));
        assert_eq!(evs.iter().filter(|e| matches!(e, GamepadEvent::Connected(_))).count(), 1);
        // The first frame carries the full state (4 sticks + 2 triggers).
        assert_eq!(evs.len(), 1 + 6);
        assert!(s.announced());

        // B pressed: only the diff, no second Connected.
        report[4] = 0x04;
        assert_eq!(s.on_report(&report, t0 + Duration::from_millis(30), &tx), Report::Decoded);
        let evs: Vec<GamepadEvent> = rx.try_iter().collect();
        assert_eq!(evs, vec![GamepadEvent::ButtonDown(0)]);

        // The watchdog runs from the last decoded report.
        assert!(!s.expired(t0 + Duration::from_millis(1029)));
        assert!(s.expired(t0 + Duration::from_millis(1030)));
    }

    #[test]
    fn input_stream_ignores_undecodable_reports() {
        let (tx, rx) = unbounded();
        let t0 = Instant::now();
        let mut s = InputStream::new(Ns2StickCal::default());
        s.arm(t0);
        assert_eq!(s.on_report(&[0u8; 15], t0 + Duration::from_millis(500), &tx), Report::Undecoded);
        assert_eq!(rx.try_iter().count(), 0, "no Connected without a decoded report");
        assert!(!s.announced());
        // An undecodable report does not feed the watchdog.
        assert!(s.expired(t0 + Duration::from_millis(1000)));
    }

    #[test]
    fn input_stream_disconnected_only_after_connected() {
        let (tx, rx) = unbounded();
        let t0 = Instant::now();
        let mut s = InputStream::new(Ns2StickCal::default());
        s.finish(&tx);
        assert_eq!(rx.try_iter().count(), 0, "never connected → no Disconnected");

        let mut s = InputStream::new(Ns2StickCal::default());
        s.arm(t0);
        s.on_report(&hex(R2_FIRST_REPORT), t0, &tx);
        let _ = rx.try_iter().count();
        s.finish(&tx);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![GamepadEvent::Disconnected]);
        // Manual disconnect finishes early (before the turn-off sequence);
        // the session-end finish must not emit a second Disconnected.
        s.finish(&tx);
        assert_eq!(rx.try_iter().count(), 0, "Disconnected is emitted once");
    }

    #[test]
    #[cfg(not(windows))]
    fn stub_never_connects() {
        let (dtx, drx) = unbounded::<SourceCmd>();
        let (wtx, wrx) = unbounded::<String>();
        let mut src = Ns2BleSource::new(drx, wtx).unwrap();
        let _ = dtx.send(SourceCmd::Disconnect);
        let _ = dtx.send(SourceCmd::Rumble);
        let mut out = Vec::new();
        for _ in 0..5 {
            src.poll(&mut out);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(out.is_empty(), "stub emitted {out:?}");
        assert!(wrx.try_recv().is_err(), "stub sends no warnings");
        assert!(dtx.is_empty(), "stub drains commands");
    }

    #[test]
    #[cfg(not(windows))]
    fn stub_suspend_flag_is_shared() {
        let (_dtx, drx) = unbounded::<SourceCmd>();
        let (wtx, _wrx) = unbounded::<String>();
        let src = Ns2BleSource::new(drx, wtx).unwrap();
        let flag = src.suspend_flag();
        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst), "not suspended at start");
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(src.suspend_flag().load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    #[cfg(windows)]
    fn windows_build_is_known() {
        let b = imp::windows_build().expect("RtlGetVersion");
        assert!(b >= 10240, "build {b}");
    }
}
