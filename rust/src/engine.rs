//! Engine: long-running mapper loop with a thread-safe handle for the GUI.
//!
//! v0.1.x ran the loop directly from `app::run`. The GUI needs to mutate
//! config while the loop runs, pause it, suppress synth during key
//! capture, and shut it down. This module wraps that into:
//!
//!   * `Engine`      — owns the thread.
//!   * `Handle`      — what the GUI holds; clones cheaply (`Arc` inside).
//!   * `EngineEvent` — what the loop emits for the activity log.

use crate::config::{Config, OutputMode};
use crate::event_log::{self, EventLog};
use crate::gamepad::lightbar::LightbarLive;
use crate::gamepad::stick_cal::{StickCalTable, StickPositions, StickProbe};
use crate::gamepad::{ControllerKind, CursorParams, GamepadSource, HubCmd, SourceCmd};
use crate::hider::HiderLink;
use crate::keyboard::KeyboardSink;
use crate::macro_engine::MacroEngine;
use crate::mapper::{KeyAction, Mapper, Source};
use crate::mouse::MouseSink;
use crate::safety::{self, SharedKeyState};
use crate::virtual_pad::XState;
use crate::xinput_sink::{self, PadError, SharedPad, VirtualPad};
use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// ─── Public event type ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A controller connected. Sent again, unchanged otherwise, whenever its
    /// status `warnings` or the `connected` pads change.
    ControllerConnected {
        name: String,
        transport: String,
        kind: ControllerKind,
        warnings: Vec<String>,
        /// Every connected pad: the owner (`kind`) first, then waiting ones.
        connected: Vec<ControllerKind>,
        /// Pads switched off for this session (`Handle::set_pad_off`).
        off_pads: Vec<ControllerKind>,
    },
    ControllerDisconnected,
    ButtonDown { id: u32 },
    ButtonUp   { id: u32 },
    /// `source`: the button or macro that sent the key (v3.2.0).
    KeyEmit    { ts_ms: u64, key: String, action: &'static str /* "down"|"up" */, source: Source },
    /// `button`: the pad button that started it.
    MacroStart { ts_ms: u64, name: String, button: u32 },
    MacroEnd   { ts_ms: u64, name: String, completed: bool },
    /// Diagnostic — emitted once per touchpad click rising edge with
    /// the raw finger position captured at that instant.
    TouchpadClick { raw_x: u16, raw_y: u16, quadrant: u32 },
    /// Diagnostic — emitted per HID frame on touchpad hover quadrant change.
    /// `quadrant = 255` is the sentinel for "finger lifted, clear UI hover".
    TouchpadHover { raw_x: u16, raw_y: u16, quadrant: u32 },
    /// The config was written outside the IPC (the hider remembered a pad,
    /// v3.6.0): the GUI emits `config-changed` (iron rule 10).
    ConfigChanged { source: &'static str },
}

/// Snapshot of the controller's current connection state. Polled by the GUI
/// on first load (since Tauri events don't replay missed emissions) and
/// mutated by the run loop on every Connected/Disconnected.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ControllerStatus {
    pub name: String,
    pub transport: String,
    pub kind: ControllerKind,
    pub warnings: Vec<String>,
    /// Every connected pad: the owner (`kind`) first, then the ones that
    /// wait for the session.
    pub connected: Vec<ControllerKind>,
    /// Pads switched off for this session (runtime only, `set_pad_off`).
    pub off_pads: Vec<ControllerKind>,
}

// ─── ConfigWriteGuard — bumps generation on drop ─────────────────────────────

/// An RAII wrapper around `RwLockWriteGuard<Config>` that increments
/// `config_generation` when dropped, so the run loop knows to rebuild
/// the `Mapper` on the next tick.
pub struct ConfigWriteGuard<'a> {
    guard: RwLockWriteGuard<'a, Config>,
    generation: &'a AtomicU64,
}

impl<'a> std::ops::Deref for ConfigWriteGuard<'a> {
    type Target = Config;
    fn deref(&self) -> &Config { &self.guard }
}

impl<'a> std::ops::DerefMut for ConfigWriteGuard<'a> {
    fn deref_mut(&mut self) -> &mut Config { &mut self.guard }
}

impl<'a> Drop for ConfigWriteGuard<'a> {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

// ─── Handle internals ────────────────────────────────────────────────────────

struct HandleInner {
    config: RwLock<Config>,
    /// Incremented by ConfigWriteGuard::drop; the run loop tracks the last
    /// seen value and rebuilds the Mapper when they differ.
    config_generation: AtomicU64,
    paused: AtomicBool,
    /// Pads switched off for this session (header 關), in `ControllerKind::ALL`
    /// order. Runtime only: never written to the config, cleared by restart.
    off_pads: RwLock<Vec<ControllerKind>>,
    capture_active: AtomicBool,
    shutdown: AtomicBool,
    event_tx: Sender<EngineEvent>,
    event_rx: Receiver<EngineEvent>,
    key_state: SharedKeyState,
    /// Current controller connection state. Set/cleared by the run loop on
    /// every Connected/Disconnected event. Exposed via `Handle::current_status`
    /// so the frontend can synchronously query "is a controller already
    /// connected?" on init without waiting to miss a startup-race event.
    current_status: RwLock<Option<ControllerStatus>>,
    /// Test-only: sender for the fake gamepad source. `None` in production.
    #[doc(hidden)]
    fake_tx: Mutex<Option<crossbeam_channel::Sender<crate::gamepad::GamepadEvent>>>,
    /// Test-only: commands the fake source received (routed by the hub to
    /// it as owner). `None` in production.
    #[doc(hidden)]
    fake_cmd_rx: Mutex<Option<Receiver<SourceCmd>>>,
    /// Test-only: commands drained from `fake_cmd_rx` and not yet counted.
    #[doc(hidden)]
    fake_cmd_buf: Mutex<Vec<SourceCmd>>,
    /// Live touchpad cursor parameters, shared with the HID worker.
    /// `set_settings` mutates these through the handle so changes take
    /// effect on the next decoded frame without rebuilding the engine.
    cursor_params: CursorParams,
    /// Lightbar state shared with the HID worker: the engine feeds it the
    /// config and whether the DualSense is on 鍵鼠.
    lightbar: LightbarLive,
    /// Commands for the session owner: the Tauri `disconnect_gamepad` IPC
    /// sends `Disconnect` through this (the worker turns its pad off and
    /// goes back to searching without exiting the thread); the engine sends
    /// `Rumble` on connect. The gamepad source forwards each to the worker
    /// that owns the session.
    source_cmd: crossbeam_channel::Sender<HubCmd>,
    /// Status warnings from the gamepad workers (e.g. the NS2 BLE worker's
    /// `ble_rate_limited_win10`). The run loop drains it every tick into
    /// `ControllerStatus.warnings` (kept for the process lifetime).
    warn_rx: Receiver<String>,
    /// Test-only: sender feeding `warn_rx`. `None` in production.
    #[doc(hidden)]
    fake_warn_tx: Mutex<Option<Sender<String>>>,
    /// User stick calibration shared with the hub; refreshed from the live
    /// config at start and on every config generation.
    stick_cal: StickCalTable,
    /// The session owner's latest stick positions (raw + corrected).
    stick_probe: StickProbe,
    /// The virtual Xbox pad. `Weak`: the engine thread owns it (a plug in
    /// flight holds a second reference until it returns), so it is dropped
    /// (and unplugged) when that thread ends, even by unwinding.
    virtual_pad: Weak<Mutex<Box<dyn VirtualPad>>>,
    /// Folder for the `log_events` file, handed to the engine thread on its
    /// next tick (`Handle::set_log_dir`).
    pending_log_dir: Mutex<Option<PathBuf>>,
    /// One more virtual-pad plug attempt requested (`retry_virtual_pad`).
    retry_pad: AtomicBool,
    /// The HidHide worker (`hider.rs`), when one runs (Windows, not dry run).
    hider: RwLock<Option<HiderLink>>,
    /// A worker was just attached: the engine sends it the current inputs.
    hider_fresh: AtomicBool,
    /// Powers Bluetooth links off by address (the automatic power-off).
    power_off: PowerOffFn,
    /// The automatic power-offs' results (links dropped per power-off).
    power_log: PowerOffLog,
    /// Set by `spawn_with_fake_gamepad`: the power-off records instead.
    fake_power_off: Arc<FakePowerOff>,
    /// Pads the app powered off this launch because they needed a replug,
    /// not back since (`get_xbox_status.auto_off`, per kind: any of them).
    auto_off: RwLock<Vec<AutoOff>>,
}

/// A pad the app powered off (`HandleInner::auto_off`), by Bluetooth address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AutoOff {
    kind: ControllerKind,
    addr: u64,
    /// The hider has seen it gone; seen present again, the entry goes.
    away: bool,
}

/// The link-level power-off by Bluetooth address
/// (`platform::power_off_addrs`; a recorder in tests).
type PowerOffFn = Arc<dyn Fn(&[u64]) -> anyhow::Result<usize> + Send + Sync>;

/// What each automatic power-off did: the pad (kind, address) and the
/// number of Bluetooth links it dropped (0: none went down, or the call
/// failed). Written by the power-off's own thread, drained by the engine.
#[derive(Debug, Clone, Default)]
struct PowerOffLog(Arc<Mutex<Vec<(ControllerKind, u64, usize)>>>);

impl PowerOffLog {
    fn record(&self, kind: ControllerKind, addr: u64, links: usize) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push((kind, addr, links));
    }

    /// Every result since the last call, oldest first.
    fn take(&self) -> Vec<(ControllerKind, u64, usize)> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// The fake power-off's record (`Engine::spawn_with_fake_gamepad`).
#[derive(Default)]
struct FakePowerOff {
    /// The addresses of each call, oldest first.
    calls: Mutex<Vec<Vec<u64>>>,
    /// Links each call reports dropped; `None`: one per address.
    links: Mutex<Option<usize>>,
}

// ─── Handle (GUI-side) ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Handle {
    inner: Arc<HandleInner>,
}

/// [`Handle::downgrade`]: for workers that must not keep the engine alive.
#[derive(Clone)]
pub struct WeakHandle(std::sync::Weak<HandleInner>);

impl WeakHandle {
    pub fn upgrade(&self) -> Option<Handle> {
        self.0.upgrade().map(|inner| Handle { inner })
    }
}

impl Handle {
    /// Live touchpad cursor parameters. The HID worker reads through
    /// this every frame; `set_settings` writes through it on the
    /// engine thread so changes take effect without a config rebuild.
    pub fn cursor_params(&self) -> CursorParams {
        self.inner.cursor_params.clone()
    }

    /// Whether the lightbar currently counts as 鍵鼠. Test hook.
    #[doc(hidden)]
    pub fn lightbar_kbm(&self) -> bool {
        self.inner.lightbar.kbm()
    }

    /// The lightbar config the engine-owned live state currently holds.
    /// Test hook.
    #[doc(hidden)]
    pub fn lightbar_config(&self) -> crate::gamepad::lightbar::LightbarConfig {
        self.inner.lightbar.snapshot().0
    }

    /// The session owner's latest stick positions, raw and corrected
    /// (`None`: no controller connected, or no stick report yet), with the
    /// raw extents seen since the previous call.
    pub fn stick_positions(&self) -> Option<StickPositions> {
        self.inner.stick_probe.take()
    }

    /// Where the `log_events` file goes (the config's folder). Until set,
    /// nothing is written even with `log_events` on.
    pub fn set_log_dir(&self, dir: PathBuf) {
        *self.inner.pending_log_dir.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir);
    }

    /// Switch pad `kind` off (`true`) or back on for this session. While the
    /// session owner is off the engine synthesizes nothing for it and keeps
    /// its virtual pad unplugged; the on→off edge releases every held key and
    /// stops its macros. Keyed by kind: it survives reconnects and may be set
    /// while the pad is away. Not a config change (no `config-changed`).
    pub fn set_pad_off(&self, kind: ControllerKind, off: bool) {
        let mut v = self.inner.off_pads.write().unwrap_or_else(|p| p.into_inner());
        let mut set: Vec<ControllerKind> = v.iter().copied().filter(|k| *k != kind).collect();
        if off {
            set.push(kind);
        }
        *v = ControllerKind::ALL.into_iter().filter(|k| set.contains(k)).collect();
    }

    /// Pads switched off for this session, in `ControllerKind::ALL` order.
    pub fn off_pads(&self) -> Vec<ControllerKind> {
        self.inner.off_pads.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Allow one more plug of the virtual Xbox pad (after ViGEmBus was
    /// installed). Plugs only if the owner still wants it (Xbox mode, not
    /// paused / off); clears `vigem_missing` until a plug fails again.
    pub fn retry_virtual_pad(&self) {
        self.inner.retry_pad.store(true, Ordering::SeqCst);
    }

    /// Feed the HidHide worker from now on: the engine sends it the kinds
    /// wanted hidden and the connected kinds whenever either changes.
    pub fn attach_hider(&self, link: HiderLink) {
        *self.inner.hider.write().unwrap_or_else(|p| p.into_inner()) = Some(link);
        self.inner.hider_fresh.store(true, Ordering::SeqCst);
    }

    /// The HidHide worker's link (`None`: no worker, e.g. dry run or not
    /// Windows).
    pub fn hider(&self) -> Option<HiderLink> {
        self.inner.hider.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The app powered pad `kind` off this launch because it needed a
    /// replug (its power-off dropped a Bluetooth link), and it has not
    /// reconnected since.
    pub fn auto_off(&self, kind: ControllerKind) -> bool {
        self.inner.auto_off.read().unwrap_or_else(|p| p.into_inner()).iter().any(|a| a.kind == kind)
    }

    /// One more HidHide round now (after HidHide was installed).
    pub fn retry_hiding(&self) {
        if let Some(link) = self.hider() {
            link.retry();
        }
    }

    /// Tell the GUI the config changed (`EngineEvent::ConfigChanged`), for
    /// a write that did not come through an IPC command.
    pub fn announce_config_change(&self, source: &'static str) {
        let _ = self.inner.event_tx.send(EngineEvent::ConfigChanged { source });
    }

    /// A handle that does not keep the engine alive.
    pub fn downgrade(&self) -> WeakHandle {
        WeakHandle(Arc::downgrade(&self.inner))
    }

    /// A callback that re-sends the current controller status unchanged
    /// (the frontend then re-probes `get_xbox_status`). Holds the engine
    /// weakly; a no-op once it is gone or while no controller is connected.
    pub fn status_notifier(&self) -> impl Fn() + Send + Sync + 'static {
        let weak = Arc::downgrade(&self.inner);
        move || {
            if let Some(h) = weak.upgrade() {
                republish_status(&h, |_| {});
            }
        }
    }

    pub fn set_paused(&self, v: bool) {
        self.inner.paused.store(v, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }
    pub fn set_capture_active(&self, v: bool) {
        self.inner.capture_active.store(v, Ordering::SeqCst);
    }
    pub fn is_capture_active(&self) -> bool {
        self.inner.capture_active.load(Ordering::SeqCst)
    }

    pub fn config_read(&self) -> RwLockReadGuard<'_, Config> {
        self.inner.config.read().unwrap()
    }

    pub fn config_write(&self) -> ConfigWriteGuard<'_> {
        ConfigWriteGuard {
            guard: self.inner.config.write().unwrap(),
            generation: &self.inner.config_generation,
        }
    }

    /// Current controller connection state — `Some` if a gamepad is plugged in
    /// (or was at startup), `None` otherwise. Frontend uses this on first load
    /// to avoid the Tauri-event startup race (events don't replay).
    pub fn current_status(&self) -> Option<ControllerStatus> {
        self.inner.current_status.read().unwrap().clone()
    }

    pub fn drain_events(&self) -> Vec<EngineEvent> {
        let mut v = Vec::new();
        while let Ok(e) = self.inner.event_rx.try_recv() {
            v.push(e);
        }
        v
    }

    pub fn held_keys_count(&self) -> usize {
        self.inner.key_state.lock().unwrap_or_else(|p| p.into_inner()).len_held()
    }

    /// Return a clone of the engine's key-state Arc. Used by `app::run` to
    /// register the global panic-hook state after the engine is spawned.
    pub fn key_state(&self) -> crate::safety::SharedKeyState {
        self.inner.key_state.clone()
    }

    /// Inject a fake ButtonDown event. Works only when spawned via
    /// `Engine::spawn_with_fake_gamepad`; no-op otherwise.
    #[doc(hidden)]
    pub fn fake_button_down(&self, id: u32) {
        if let Ok(guard) = self.inner.fake_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(crate::gamepad::GamepadEvent::ButtonDown(id));
            }
        }
    }

    /// Inject an arbitrary fake gamepad event. Works only when spawned via
    /// `Engine::spawn_with_fake_gamepad`; no-op otherwise.
    #[doc(hidden)]
    pub fn fake_event(&self, ev: crate::gamepad::GamepadEvent) {
        if let Ok(guard) = self.inner.fake_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(ev);
            }
        }
    }

    /// How many manual disconnects reached the fake source since the last
    /// call. Works only when spawned via `Engine::spawn_with_fake_gamepad`;
    /// 0 otherwise.
    #[doc(hidden)]
    pub fn fake_disconnects_received(&self) -> usize {
        self.fake_cmds_received(SourceCmd::Disconnect)
    }

    /// How many rumble requests reached the fake source since the last
    /// call. Works only when spawned via `Engine::spawn_with_fake_gamepad`;
    /// 0 otherwise.
    #[doc(hidden)]
    pub fn fake_rumbles_received(&self) -> usize {
        self.fake_cmds_received(SourceCmd::Rumble)
    }

    /// Drain the fake source's command receiver into the shared buffer,
    /// then remove and count the entries of `kind` (other kinds stay).
    fn fake_cmds_received(&self, kind: SourceCmd) -> usize {
        let (Ok(rx), Ok(mut buf)) = (self.inner.fake_cmd_rx.lock(), self.inner.fake_cmd_buf.lock())
        else {
            return 0;
        };
        if let Some(rx) = rx.as_ref() {
            buf.extend(rx.try_iter());
        }
        let before = buf.len();
        buf.retain(|c| *c != kind);
        before - buf.len()
    }

    /// The addresses of each automatic power-off since the last call. Works
    /// only when spawned via `Engine::spawn_with_fake_gamepad`; empty
    /// otherwise.
    #[doc(hidden)]
    pub fn fake_power_offs(&self) -> Vec<Vec<u64>> {
        std::mem::take(&mut *self.inner.fake_power_off.calls.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// How many links the fake power-off reports dropped per call (`None`:
    /// one per address, the default). Test hook.
    #[doc(hidden)]
    pub fn set_fake_power_off_links(&self, links: Option<usize>) {
        *self.inner.fake_power_off.links.lock().unwrap_or_else(|p| p.into_inner()) = links;
    }

    /// Inject a status warning as a gamepad worker would. Works only when
    /// spawned via `Engine::spawn_with_fake_gamepad*`; no-op otherwise.
    #[doc(hidden)]
    pub fn fake_warning(&self, w: &str) {
        if let Ok(guard) = self.inner.fake_warn_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.try_send(w.to_string());
            }
        }
    }

    /// The engine's virtual pad, for the panic hook
    /// (`xinput_sink::register_global`).
    pub(crate) fn virtual_pad(&self) -> Weak<Mutex<Box<dyn VirtualPad>>> {
        self.inner.virtual_pad.clone()
    }

    /// Inject a fake ButtonUp event. Works only when spawned via
    /// `Engine::spawn_with_fake_gamepad`; no-op otherwise.
    #[doc(hidden)]
    pub fn fake_button_up(&self, id: u32) {
        if let Ok(guard) = self.inner.fake_tx.lock() {
            if let Some(ref tx) = *guard {
                let _ = tx.send(crate::gamepad::GamepadEvent::ButtonUp(id));
            }
        }
    }

    /// Manual disconnect of pad `kind` (`None`: the session owner). The
    /// gamepad source forwards it to the worker whose session is that pad
    /// (not connected: ignored), which drops its device and returns to
    /// searching. The thread does NOT exit; the pad reconnects via the
    /// normal handshake path on its next button press.
    pub fn disconnect(&self, kind: Option<ControllerKind>) {
        let _ = self.inner.source_cmd.send(HubCmd { to: kind, cmd: SourceCmd::Disconnect });
    }
}

// ─── Engine (owns the thread) ────────────────────────────────────────────────

pub struct Engine {
    thread: Option<JoinHandle<Result<()>>>,
    handle: Handle,
}

impl Engine {
    /// Spawn with a real gamepad source.
    pub fn spawn(cfg: Config, dry_run: bool) -> Result<Self> {
        let cursor_params = CursorParams::with_midpoints(
            cfg.touchpad_cursor_sensitivity,
            cfg.touchpad_cursor_enabled,
            cfg.touchpad_midpoint_x,
            cfg.touchpad_midpoint_y,
        );
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<HubCmd>(4);
        // Bounded: workers `try_send`, and each warning is sent at most once.
        let (warn_tx, warn_rx) = crossbeam_channel::bounded::<String>(8);
        let (cal, probe) = (StickCalTable::default(), StickProbe::default());
        let lightbar = LightbarLive::default();
        // Before the HID worker starts: it must never play the default
        // (自訂) settings to a 原廠燈 / 關閉 user.
        lightbar.set_config(cfg.lightbar);
        let src = GamepadSource::new(cursor_params.clone(), lightbar.clone(), cmd_rx, warn_tx)?
            .with_stick_cal(cal.clone(), probe.clone());
        // Dry run synthesizes no OS input: that includes the virtual pad.
        let pad: Box<dyn VirtualPad> =
            if dry_run { Box::new(xinput_sink::NullPad) } else { xinput_sink::platform_pad() };
        Self::spawn_inner(cfg, dry_run, src, cursor_params, lightbar, cmd_tx, warn_rx, pad, cal, probe, false)
    }

    /// Spawn with a fake gamepad source. Events are injected via
    /// `Handle::fake_button_down` / `Handle::fake_button_up`.
    /// Not part of the stable public API — used by integration tests.
    ///
    /// The virtual pad is `NullPad`: tests never plug a real one.
    #[doc(hidden)]
    pub fn spawn_with_fake_gamepad(cfg: Config) -> Result<Self> {
        Self::spawn_with_fake_gamepad_and_pad(cfg, Box::new(xinput_sink::NullPad))
    }

    /// `spawn_with_fake_gamepad` with a caller-supplied virtual pad.
    /// Not part of the stable public API — used by integration tests.
    #[doc(hidden)]
    pub fn spawn_with_fake_gamepad_and_pad(cfg: Config, pad: Box<dyn VirtualPad>) -> Result<Self> {
        let cursor_params = CursorParams::with_midpoints(
            cfg.touchpad_cursor_sensitivity,
            cfg.touchpad_cursor_enabled,
            cfg.touchpad_midpoint_x,
            cfg.touchpad_midpoint_y,
        );
        let (fake_tx, fake_rx) = unbounded::<crate::gamepad::GamepadEvent>();
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<HubCmd>(4);
        let (source_cmd_tx, source_cmd_rx) = unbounded::<SourceCmd>();
        let (cal, probe) = (StickCalTable::default(), StickProbe::default());
        let src = GamepadSource::fake_with_disconnect(fake_rx, cmd_rx, source_cmd_tx).with_stick_cal(cal.clone(), probe.clone());
        let (warn_tx, warn_rx) = crossbeam_channel::bounded::<String>(8);
        let engine = Self::spawn_inner(
            cfg, /*dry_run=*/true, src, cursor_params, LightbarLive::default(), cmd_tx, warn_rx, pad, cal, probe,
            /*fake_power_off=*/true,
        )?;
        // Install the sender into HandleInner.
        *engine.handle.inner.fake_tx.lock().unwrap() = Some(fake_tx);
        *engine.handle.inner.fake_warn_tx.lock().unwrap() = Some(warn_tx);
        *engine.handle.inner.fake_cmd_rx.lock().unwrap() = Some(source_cmd_rx);
        Ok(engine)
    }

    fn spawn_inner(
        cfg: Config,
        dry_run: bool,
        src: GamepadSource,
        cursor_params: CursorParams,
        lightbar: LightbarLive,
        source_cmd: crossbeam_channel::Sender<HubCmd>,
        warn_rx: Receiver<String>,
        pad: Box<dyn VirtualPad>,
        stick_cal: StickCalTable,
        stick_probe: StickProbe,
        fake_power_off: bool,
    ) -> Result<Self> {
        stick_cal.replace_from(&cfg);
        lightbar.set_config(cfg.lightbar);
        let key_state = safety::shared();
        let pad: SharedPad = Arc::new(Mutex::new(pad));
        let (event_tx, event_rx) = unbounded::<EngineEvent>();
        let fake = Arc::new(FakePowerOff::default());
        let power_off: PowerOffFn = if fake_power_off {
            let f = fake.clone();
            Arc::new(move |addrs: &[u64]| {
                f.calls.lock().unwrap_or_else(|p| p.into_inner()).push(addrs.to_vec());
                Ok(f.links.lock().unwrap_or_else(|p| p.into_inner()).unwrap_or(addrs.len()))
            })
        } else {
            Arc::new(crate::platform::power_off_addrs)
        };

        let inner = Arc::new(HandleInner {
            config: RwLock::new(cfg),
            config_generation: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            off_pads: RwLock::new(Vec::new()),
            capture_active: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            event_tx,
            event_rx,
            key_state: key_state.clone(),
            fake_tx: Mutex::new(None),
            fake_cmd_rx: Mutex::new(None),
            fake_cmd_buf: Mutex::new(Vec::new()),
            current_status: RwLock::new(None),
            cursor_params,
            lightbar,
            source_cmd,
            warn_rx,
            fake_warn_tx: Mutex::new(None),
            stick_cal,
            stick_probe,
            virtual_pad: Arc::downgrade(&pad),
            pending_log_dir: Mutex::new(None),
            retry_pad: AtomicBool::new(false),
            hider: RwLock::new(None),
            hider_fresh: AtomicBool::new(false),
            power_off,
            power_log: PowerOffLog::default(),
            fake_power_off: fake,
            auto_off: RwLock::new(Vec::new()),
        });

        let handle = Handle { inner: inner.clone() };

        let thread = thread::spawn(move || run_loop(inner, src, dry_run, pad));
        Ok(Engine { thread: Some(thread), handle })
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Signal shutdown and block until the engine thread exits. The closing
    /// log line is what `scripts/verify.sh` (gui-smoke) looks for after a
    /// graceful window close.
    pub fn shutdown(mut self) {
        self.handle.inner.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        tracing::info!("engine shut down");
    }
}

// ─── Run loop (engine-thread) ─────────────────────────────────────────────────

fn run_loop(h: Arc<HandleInner>, mut source: GamepadSource, dry_run: bool, pad: SharedPad) -> Result<()> {
    // Read initial config values before the loop.
    let (tick_jitter_ms, min_press_ms) = {
        let cfg = h.config.read().unwrap();
        (cfg.tick_jitter_ms, cfg.min_press_ms)
    };

    let mut sink = KeyboardSink::new(h.key_state.clone(), tick_jitter_ms, min_press_ms, dry_run)
        .expect("KeyboardSink::new failed");
    let mut mouse_sink = MouseSink::new(h.key_state.clone(), dry_run)
        .expect("MouseSink::new failed");

    // Set up macro → keyboard channel (std::sync::mpsc, matching MacroEngine).
    let (macro_tx, macro_rx) = std::sync::mpsc::channel::<KeyAction>();
    let mut macros = MacroEngine::new(macro_tx);

    // Build the initial mapper.
    let mut mapper = {
        let cfg = h.config.read().unwrap();
        Mapper::new(cfg.clone())
    };
    let mut last_config_gen = h.config_generation.load(Ordering::SeqCst);

    let mut last_paused = false;
    let mut gp_events: Vec<crate::gamepad::GamepadEvent> = Vec::with_capacity(32);

    // Per-controller output mode; mapping (key synth) is ON only in Kbm.
    // `None` (no controller seen yet, or after Disconnected) means Kbm.
    let mut current_kind: Option<ControllerKind> = None;
    let mut mode = OutputMode::Kbm;
    let mut mapping_on = true;
    // The owner is switched off (header 關): synthesize nothing, no virtual
    // pad. Polling goes on, so connects / disconnects are still seen.
    let mut pad_off = false;
    let mut last_off_pads: Vec<ControllerKind> = Vec::new();

    // Virtual Xbox pad (Xbox output mode) and the status warnings.
    let mut pad = PadLink::new(pad);
    let mut worker_warnings: Vec<String> = Vec::new();
    // The connected pads last published in the status.
    let mut last_connected: Vec<ControllerKind> = Vec::new();
    // The `log_events` file (no folder until `Handle::set_log_dir`).
    let mut elog = EventLog::new(None);
    // HidHide inputs: the kinds wanted hidden (from the live config) and the
    // connected kinds last sent to the worker (`None`: not sent yet).
    let mut hide_wanted = hide_wanted_kinds(&h.config.read().unwrap());
    let mut hider_sent: Option<(Vec<ControllerKind>, Vec<ControllerKind>)> = None;
    // The remembered pads' ids (`hidhide_known`) and those last sent.
    let mut hide_known = hide_known_ids(&h.config.read().unwrap());
    let mut known_sent: Option<Vec<String>> = None;
    // A pad hidden while connected is powered off once per launch, by its
    // Bluetooth address: the addresses tried, the kinds logged as having
    // none, and the connected kinds at the previous tick.
    let mut auto_tried: Vec<u64> = Vec::new();
    let mut auto_no_addr: Vec<ControllerKind> = Vec::new();
    let mut prev_connected: Vec<ControllerKind> = Vec::new();

    while !h.shutdown.load(Ordering::SeqCst) {
        if let Some(dir) = h.pending_log_dir.lock().unwrap_or_else(|p| p.into_inner()).take() {
            elog.set_dir(Some(dir));
        }

        // ── Source control, paused or not ────────────────────────────────────
        // Turn Off stays clickable while paused; its request must reach the
        // owning worker now, not queue until unpause (`poll` is skipped then).
        source.service();

        // ── Worker warnings: kept for the process lifetime, deduplicated ─────
        let mut warnings_changed = false;
        for w in h.warn_rx.try_iter() {
            if !worker_warnings.contains(&w) {
                worker_warnings.push(w);
                warnings_changed = true;
            }
        }

        // ── Pause / unpause edge ─────────────────────────────────────────────
        let paused = h.paused.load(Ordering::SeqCst);
        if paused && !last_paused {
            // Drain all macros and held keys when pausing.
            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
            // No events are polled while paused, so their releases are lost.
            mapper.reset_input_state();
        }
        last_paused = paused;

        // ── Config change: swap the mapper's config ──────────────────────────
        // `replace_config`, not a new Mapper: held bindings and trigger
        // state survive, so a key held across a rebind still releases the
        // key it pressed (iron rule 2).
        let current_gen = h.config_generation.load(Ordering::SeqCst);
        if current_gen != last_config_gen {
            let cfg = h.config.read().unwrap();
            h.stick_cal.replace_from(&cfg);
            h.lightbar.set_config(cfg.lightbar);
            mapper.replace_config(cfg.clone());
            hide_wanted = hide_wanted_kinds(&cfg);
            hide_known = hide_known_ids(&cfg);
            last_config_gen = current_gen;
        }

        // ── Mapping ON→OFF edge: release everything ──────────────────────────
        let now_mode = output_mode(&h, current_kind);
        let now_on = now_mode == OutputMode::Kbm;
        if mapping_on && !now_on {
            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
            mapper.reset_input_state();
        }
        if now_mode == OutputMode::Xbox && mode != OutputMode::Xbox {
            // An edge into Xbox may retry a plug that failed earlier this session.
            pad.rearm();
        }
        mode = now_mode;
        mapping_on = now_on;

        // ── Per-pad 關: on→off edge releases everything ──────────────────────
        let off_now = h.off_pads.read().unwrap_or_else(|p| p.into_inner()).clone();
        if off_now != last_off_pads {
            last_off_pads = off_now.clone();
            republish_status(&h, |s| s.off_pads = off_now.clone());
        }
        let now_off = is_off(&last_off_pads, current_kind);
        if now_off && !pad_off {
            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
            mapper.reset_input_state();
        }
        if !now_off && pad_off {
            // Back on: one more plug attempt if Xbox output had failed.
            pad.rearm();
        }
        pad_off = now_off;

        if h.retry_pad.swap(false, Ordering::SeqCst) {
            pad.rearm();
            if pad.bus_missing {
                pad.bus_missing = false;
                warnings_changed = true;
            }
        }

        // ── Virtual pad: plugged only in Xbox mode ───────────────────────────
        // After the OFF edge's release above (keys up, then the pad appears);
        // before polling on the ON edge (pad gone, then keys resume).
        warnings_changed |= pad.sync(wants_pad(paused || pad_off, mode, current_kind));
        if warnings_changed {
            publish_warnings(&h, status_warnings(&worker_warnings, &pad));
        }

        // ── Lightbar: 鍵鼠 only for a live DualSense under mapping ───────────
        // Before the `paused` gate so it keeps following while paused.
        h.lightbar.set_kbm(
            current_kind == Some(ControllerKind::DualSense) && mapping_on && !pad_off && !paused,
        );

        if !paused {
            // ── Poll gamepad ─────────────────────────────────────────────────
            gp_events.clear();
            source.poll(&mut gp_events);
            // Before the events, so a result that came before its pad's
            // `Disconnected` is set before the frontend hears of the leave;
            // the status is re-sent either way, for it to re-probe.
            for (kind, addr, links) in h.power_log.take() {
                if links > 0 {
                    tracing::info!(?kind, "HidHide: powered off; it comes back hidden");
                    let mut v = h.auto_off.write().unwrap_or_else(|p| p.into_inner());
                    if !v.iter().any(|a| a.addr == addr) {
                        v.push(AutoOff { kind, addr, away: false });
                    }
                    drop(v);
                    republish_status(&h, |_| {});
                } else {
                    // No link went down: the pad stays, a game may still hold it.
                    tracing::info!(?kind, "HidHide: the power-off dropped no link; one reconnect still needed");
                }
            }

            for ev in gp_events.drain(..) {
                // Always emit raw gamepad events (even when capture_active).
                match &ev {
                    crate::gamepad::GamepadEvent::ButtonDown(id) => {
                        let _ = h.event_tx.send(EngineEvent::ButtonDown { id: *id });
                    }
                    crate::gamepad::GamepadEvent::ButtonUp(id) => {
                        let _ = h.event_tx.send(EngineEvent::ButtonUp { id: *id });
                    }
                    crate::gamepad::GamepadEvent::Connected(kind) => {
                        let kind = *kind;
                        current_kind = Some(kind);
                        // From now on presses use this pad's table.
                        mapper.set_pad(Some(kind));
                        mode = output_mode(&h, current_kind);
                        let now_on = mode == OutputMode::Kbm;
                        if mapping_on && !now_on {
                            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
                            mapper.reset_input_state();
                        }
                        mapping_on = now_on;
                        let now_off = is_off(&last_off_pads, current_kind);
                        if now_off && !pad_off {
                            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
                            mapper.reset_input_state();
                        }
                        pad_off = now_off;
                        // A fresh session: neutral pad state, one plug attempt.
                        pad.reset_session();
                        pad.sync(wants_pad(paused || pad_off, mode, current_kind));
                        last_connected = source.connected_kinds();
                        let status = ControllerStatus {
                            name: kind.display_name().to_string(),
                            transport: "BT".to_string(),
                            kind,
                            warnings: status_warnings(&worker_warnings, &pad),
                            connected: last_connected.clone(),
                            off_pads: last_off_pads.clone(),
                        };
                        *h.current_status.write().unwrap() = Some(status.clone());
                        let _ = h.event_tx.send(connected_event(&status));
                        // Ask the owner for one short rumble. `try_send`: a full
                        // channel must never stall the engine tick.
                        if h.config.read().unwrap().rumble_on_connect {
                            tracing::info!(?kind, "rumble on connect: requested");
                            if h.source_cmd.try_send(HubCmd::owner(SourceCmd::Rumble)).is_err() {
                                tracing::debug!("rumble request dropped: command channel full");
                            }
                        }
                    }
                    crate::gamepad::GamepadEvent::Disconnected => {
                        current_kind = None;
                        pad_off = false;
                        last_connected.clear();
                        // The pad is gone: so is its virtual twin.
                        pad.sync(false);
                        pad.reset_session();
                        *h.current_status.write().unwrap() = None;
                        let _ = h.event_tx.send(EngineEvent::ControllerDisconnected);
                        // The pad is gone, so no ButtonUp / stick-neutral will
                        // ever arrive for what it was holding: release it all
                        // now (iron rules 2–4), as on pause / mapping ON→OFF.
                        drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
                        // Forget the stick / trigger virtual state too: the
                        // next session's first frame reports every axis, and a
                        // stale `virt_active` would swallow that press.
                        mapper = Mapper::new(mapper.config().clone());
                        mapper.set_pad(None);
                    }
                    _ => {}
                }

                // The virtual pad mirrors every event, whatever the mode (it
                // is pushed only while plugged), and is not gated on key
                // capture: it types nothing.
                pad.feed(&ev);

                // Gate synth on capture_active.
                if h.capture_active.load(Ordering::SeqCst) {
                    continue;
                }

                let cfg = h.config.read().unwrap();
                let actions = mapper.handle(ev);
                drop(cfg); // release lock before execute calls into sink
                for (id, pressed) in mapper.take_visual_transitions() {
                    let _ = if pressed {
                        h.event_tx.send(EngineEvent::ButtonDown { id })
                    } else {
                        h.event_tx.send(EngineEvent::ButtonUp { id })
                    };
                }
                for action in &actions {
                    // Mapping OFF or pad off: keep the touchpad diagnostics,
                    // synthesize nothing.
                    if (!mapping_on || pad_off) && !is_diagnostic(action) {
                        continue;
                    }
                    execute_action(action, &mut sink, &mut mouse_sink, &mut macros, mapper.config(), &h.event_tx, &mut elog);
                }
            }

            // ── Drain macro-emitted key actions ──────────────────────────────
            while let Ok(action) = macro_rx.try_recv() {
                // Mapping OFF: a macro cancelled on the ON→OFF edge may still
                // race a late Press/Release in; drop it (sink was already reset).
                if (!mapping_on || pad_off) && !is_diagnostic(&action) {
                    continue;
                }
                execute_action(&action, &mut sink, &mut mouse_sink, &mut macros, mapper.config(), &h.event_tx, &mut elog);
            }
        }

        // ── Connected pads: re-publish the status when the list changes ─────
        // (a second pad connects or leaves while the owner keeps the session).
        let now_connected = source.connected_kinds();
        if now_connected != last_connected {
            last_connected = now_connected;
            republish_status(&h, |s| s.connected = last_connected.clone());
        }

        // ── HidHide worker: send its inputs when they change ────────────────
        // Unbounded channel: never blocks the tick.
        // The remembered list goes first, so the first round already has it.
        if h.hider_fresh.swap(false, Ordering::SeqCst) {
            hider_sent = None;
            known_sent = None;
        }
        // A pad the app powered off is back: its auto_off goes. Back = the
        // hider saw it gone and then present again, or its kind joined the
        // engine (a pad turned back on before the hider looked).
        let joined: Vec<ControllerKind> =
            last_connected.iter().copied().filter(|k| !prev_connected.contains(k)).collect();
        prev_connected = last_connected.clone();
        let present_addrs = h.hider.read().unwrap_or_else(|p| p.into_inner()).as_ref().map(|l| l.present_addrs());
        let changed = {
            let mut v = h.auto_off.write().unwrap_or_else(|p| p.into_inner());
            let before = v.len();
            v.retain(|a| !joined.contains(&a.kind));
            if let Some(present) = &present_addrs {
                for a in v.iter_mut() {
                    a.away |= !present.iter().any(|(_, p)| *p == a.addr);
                }
                v.retain(|a| !(a.away && present.iter().any(|(_, p)| *p == a.addr)));
            }
            v.len() != before
        };
        if changed {
            republish_status(&h, |_| {});
        }
        if let Some(link) = h.hider.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
            // Hidden while connected: drop the Bluetooth link of each pad that
            // was hidden while present (not every pad of the kind, as the ⏻
            // does: another one may be in use), so it comes back hidden. Once
            // per pad per launch, on its own thread (the call may block). The
            // HID session ends by the link drop alone (iron rule 11). A pad
            // with no address to drop (USB, or a Bluetooth LE pad such as
            // the Xbox Nano) keeps the reconnect hint.
            let addrs = link.replug_addrs();
            for kind in link.needs_replug() {
                if !last_connected.contains(&kind) {
                    continue;
                }
                let of_kind: Vec<u64> = addrs.iter().filter(|(k, _)| *k == kind).map(|(_, a)| *a).collect();
                if of_kind.is_empty() {
                    if !auto_no_addr.contains(&kind) {
                        auto_no_addr.push(kind);
                        tracing::info!(?kind, "HidHide: hidden while connected, no classic Bluetooth address; one reconnect needed");
                    }
                    continue;
                }
                let fresh: Vec<u64> = of_kind.into_iter().filter(|a| !auto_tried.contains(a)).collect();
                if fresh.is_empty() {
                    continue;
                }
                auto_tried.extend(&fresh);
                tracing::info!(?kind, pads = fresh.len(), "HidHide: hidden while connected; powering it off once");
                let (power_off, log) = (h.power_off.clone(), h.power_log.clone());
                let spawned = thread::Builder::new().name("auto-power-off".into()).spawn(move || {
                    for addr in fresh {
                        let links = power_off(&[addr]).unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "HidHide: power-off failed");
                            0
                        });
                        log.record(kind, addr, links);
                    }
                });
                if let Err(e) = spawned {
                    tracing::warn!("HidHide: could not start the power-off: {e}");
                }
            }
            if known_sent.as_ref() != Some(&hide_known) {
                link.set_known(hide_known.clone());
                known_sent = Some(hide_known.clone());
            }
            if hider_sent.as_ref().map_or(true, |(w, c)| *w != hide_wanted || *c != last_connected) {
                link.update(hide_wanted.clone());
                hider_sent = Some((hide_wanted.clone(), last_connected.clone()));
            }
        }

        // One pad report per tick at most.
        pad.flush();

        thread::sleep(Duration::from_millis(8));
    }

    // ── Graceful shutdown ────────────────────────────────────────────────────
    pad.shutdown();
    macros.stop_all();
    // Drain any final macro releases.
    while let Ok(action) = macro_rx.try_recv() {
        execute_action(&action, &mut sink, &mut mouse_sink, &mut macros, mapper.config(), &h.event_tx, &mut elog);
    }
    // sink.drop() will call release_all_held() — that is the iron-rule guarantee.
    // We explicitly drop to make the intent clear.
    drop(sink);
    drop(mouse_sink);
    Ok(())
}

/// The connected controller's output mode. Reads the live config (not the
/// mapper's snapshot) so an IPC toggle takes effect on the next tick. No
/// known controller → Kbm.
fn output_mode(h: &HandleInner, kind: Option<ControllerKind>) -> OutputMode {
    kind.map_or(OutputMode::Kbm, |k| h.config.read().unwrap().output_mode_for(k))
}

/// The kinds HidHide should hide: in 鍵鼠 or Xbox output mode (whether
/// connected or not, so a reconnect arrives hidden; 手把 mode never hides) and driven natively by Windows (the
/// Switch 2 Pro is not, so a game cannot see it anyway).
fn hide_wanted_kinds(cfg: &Config) -> Vec<ControllerKind> {
    ControllerKind::ALL
        .into_iter()
        .filter(|k| k.has_native_driver() && matches!(cfg.output_mode_for(*k), OutputMode::Kbm | OutputMode::Xbox))
        .collect()
}

/// The remembered pads' instance ids (`hidhide_known`): hidden while their
/// kind is wanted, even when absent.
fn hide_known_ids(cfg: &Config) -> Vec<String> {
    cfg.hidhide_known.iter().map(|k| k.id.clone()).collect()
}

/// Whether the session owner `kind` is switched off.
fn is_off(off: &[ControllerKind], kind: Option<ControllerKind>) -> bool {
    kind.map_or(false, |k| off.contains(&k))
}

/// The virtual Xbox pad is plugged only for a connected pad (any kind) in
/// Xbox mode, and not while paused or off (no input is forwarded then, so a
/// plugged pad would freeze whatever it last reported).
fn wants_pad(paused_or_off: bool, mode: OutputMode, kind: Option<ControllerKind>) -> bool {
    !paused_or_off && mode == OutputMode::Xbox && kind.is_some()
}

/// Warnings for `ControllerStatus`: the workers' (process lifetime), then
/// the session's `vigem_missing`.
fn status_warnings(worker: &[String], pad: &PadLink) -> Vec<String> {
    let mut w = worker.to_vec();
    if pad.bus_missing {
        w.push(WARN_VIGEM_MISSING.to_string());
    }
    w
}

/// Store new warnings in the current status and re-send it. No controller
/// connected: nothing to show (the next `Connected` carries them).
fn publish_warnings(h: &HandleInner, warnings: Vec<String>) {
    republish_status(h, |s| s.warnings = warnings);
}

/// Patch the current status and re-send it. No controller connected:
/// nothing to show.
fn republish_status(h: &HandleInner, patch: impl FnOnce(&mut ControllerStatus)) {
    let mut cur = h.current_status.write().unwrap();
    if let Some(status) = cur.as_mut() {
        patch(status);
        let _ = h.event_tx.send(connected_event(status));
    }
}

fn connected_event(status: &ControllerStatus) -> EngineEvent {
    EngineEvent::ControllerConnected {
        name: status.name.clone(),
        transport: status.transport.clone(),
        kind: status.kind,
        warnings: status.warnings.clone(),
        connected: status.connected.clone(),
        off_pads: status.off_pads.clone(),
    }
}

/// Status warning: ViGEmBus is not installed, so a pad in Xbox mode gets no
/// virtual Xbox pad (鍵鼠 still works).
pub const WARN_VIGEM_MISSING: &str = "vigem_missing";

/// A plug still running after this long is logged (once).
const PLUG_SLOW_WARN: Duration = Duration::from_secs(5);
/// Shutdown waits at most this long for a plug in flight.
const PLUG_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// A `plug()` running on its own thread.
struct Plugging {
    rx: Receiver<Result<(), PadError>>,
    since: Instant,
    warned: bool,
}

/// The virtual Xbox pad's lifecycle (Xbox output mode). Owned by the engine
/// thread; the `SharedPad` is shared only with the panic hook and a plug in
/// flight.
///
/// `plug()` runs on a short-lived thread: vigem-client's `wait_ready` can
/// block (forever, in the worst case), and the engine thread must keep
/// polling and publishing the status meanwhile. `update` / `unplug` use
/// `try_lock`, so they never wait on it either.
struct PadLink {
    pad: SharedPad,
    plugged: bool,
    /// A plug failed this session. No retry until a fresh `Connected` or an
    /// edge into Xbox mode: a missing bus must not be retried every tick.
    plug_failed: bool,
    /// Session warning: ViGEmBus is not installed.
    bus_missing: bool,
    state: XState,
    /// `state` changed since the pad last accepted it.
    dirty: bool,
    /// The plug in flight, if any. Never more than one.
    plugging: Option<Plugging>,
}

impl PadLink {
    fn new(pad: SharedPad) -> Self {
        PadLink {
            pad,
            plugged: false,
            plug_failed: false,
            bus_missing: false,
            state: XState::default(),
            dirty: false,
            plugging: None,
        }
    }

    /// The pad's lock if free now (a poisoned one is taken anyway).
    fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, Box<dyn VirtualPad>>> {
        match self.pad.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// Plug or unplug to match `want`. Returns whether the session warnings
    /// changed.
    fn sync(&mut self, want: bool) -> bool {
        let changed = self.poll_plug();
        if want && !self.plugged && !self.plug_failed && self.plugging.is_none() {
            self.start_plug();
        } else if !want && self.plugged {
            // A plug that returned after `want` turned false lands here too.
            self.unplug();
        }
        changed
    }

    fn start_plug(&mut self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let pad = self.pad.clone();
        let spawned = thread::Builder::new().name("virtual-pad-plug".into()).spawn(move || {
            let result = pad.lock().unwrap_or_else(|p| p.into_inner()).plug();
            let _ = tx.send(result);
        });
        match spawned {
            Ok(_) => self.plugging = Some(Plugging { rx, since: Instant::now(), warned: false }),
            Err(e) => {
                tracing::warn!(error = %e, "virtual pad: could not start the plug thread");
                self.plug_failed = true;
            }
        }
    }

    /// Take the result of a plug in flight, if it has returned. Returns
    /// whether the session warnings changed.
    fn poll_plug(&mut self) -> bool {
        let Some(p) = self.plugging.as_mut() else { return false };
        match p.rx.try_recv() {
            Ok(result) => {
                self.plugging = None;
                self.plug_done(result)
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                if !p.warned && p.since.elapsed() >= PLUG_SLOW_WARN {
                    p.warned = true;
                    tracing::warn!("virtual pad: plug still running after 5 s (ViGEmBus not answering?)");
                }
                false
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                // The plug thread panicked inside the backend.
                self.plugging = None;
                self.plug_done(Err(PadError::Other("plug thread ended without a result".into())))
            }
        }
    }

    fn plug_done(&mut self, result: Result<(), PadError>) -> bool {
        match result {
            Ok(()) => {
                self.plugged = true;
                self.dirty = true; // report the current state right away
            }
            Err(e) => {
                self.plug_failed = true;
                match e {
                    PadError::BusNotFound => {
                        tracing::warn!("ViGEmBus is not installed: no virtual Xbox pad");
                        let changed = !self.bus_missing;
                        self.bus_missing = true;
                        return changed;
                    }
                    PadError::Unsupported => tracing::debug!("no virtual pad on this platform"),
                    PadError::Other(msg) => tracing::warn!(error = %msg, "virtual pad plug failed"),
                }
            }
        }
        false
    }

    /// Unplug now if the lock is free; otherwise stay plugged and let the
    /// next `sync` retry.
    fn unplug(&mut self) {
        let done = self.try_lock().map(|mut p| p.unplug()).is_some();
        if done {
            self.plugged = false;
        }
    }

    /// Engine shutdown: wait a little for a plug in flight (a stuck one is
    /// left behind: the process exit removes its target), then unplug.
    fn shutdown(&mut self) {
        if let Some(p) = self.plugging.take() {
            match p.rx.recv_timeout(PLUG_SHUTDOWN_WAIT) {
                Ok(result) => {
                    self.plug_done(result);
                }
                Err(_) => tracing::warn!("virtual pad: plug still running at shutdown; not waiting"),
            }
        }
        if self.plugged {
            // The plug thread, if any, has returned and released the lock.
            self.pad.lock().unwrap_or_else(|p| p.into_inner()).unplug();
            self.plugged = false;
        }
    }

    /// Allow one more plug attempt after a failure.
    fn rearm(&mut self) {
        self.plug_failed = false;
    }

    /// A controller session starts or ends: neutral state, no failed plug,
    /// no session warning. A plug in flight keeps running (no second one).
    fn reset_session(&mut self) {
        self.state = XState::default();
        self.dirty = true;
        self.plug_failed = false;
        self.bus_missing = false;
    }

    fn feed(&mut self, ev: &crate::gamepad::GamepadEvent) {
        let before = self.state;
        self.state.apply(ev);
        self.dirty |= self.state != before;
    }

    /// Push the state if it changed. A refused report stays dirty and is
    /// retried (with the latest state) next tick, so a transient bus refusal
    /// cannot leave a button pressed in the game. A busy lock is skipped the
    /// same way.
    fn flush(&mut self) {
        if self.plugged && self.dirty {
            let s = self.state;
            let accepted = self.try_lock().is_some_and(|mut p| p.update(&s).is_ok());
            self.dirty = !accepted;
        }
    }
}

/// Actions that synthesize no input — the only ones executed while mapping
/// is OFF.
fn is_diagnostic(action: &KeyAction) -> bool {
    matches!(action, KeyAction::TouchpadClick { .. } | KeyAction::TouchpadHover { .. })
}

/// Stop every macro, run the releases they drain, then release every held
/// key and mouse button (iron rules 2–4). Shared by the pause edge and the
/// mapping ON→OFF edge.
fn drain_and_release(
    macros: &mut MacroEngine,
    macro_rx: &std::sync::mpsc::Receiver<KeyAction>,
    sink: &mut KeyboardSink,
    mouse_sink: &mut MouseSink,
    cfg: &Config,
    event_tx: &Sender<EngineEvent>,
    elog: &mut EventLog,
) {
    macros.stop_all();
    // Drain macro-emitted releases so sink's refcounts are balanced.
    while let Ok(action) = macro_rx.try_recv() {
        execute_action(&action, sink, mouse_sink, macros, cfg, event_tx, elog);
    }
    sink.release_all_held();
    mouse_sink.release_all_held();
}

fn execute_action(
    action: &KeyAction,
    sink: &mut KeyboardSink,
    mouse_sink: &mut MouseSink,
    macros: &mut MacroEngine,
    cfg: &Config,
    event_tx: &Sender<EngineEvent>,
    elog: &mut EventLog,
) {
    let now_ms = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    };
    match action {
        KeyAction::Press(k, src) => {
            let _ = sink.press(k);
            let ts_ms = now_ms();
            elog.append(cfg.log_events, &event_log::format_line(ts_ms, "按下", &format!("送出 {k} 按下"), &src.tag()));
            let _ = event_tx.send(EngineEvent::KeyEmit {
                ts_ms,
                key: k.clone(),
                action: "down",
                source: src.clone(),
            });
        }
        KeyAction::Release(k, src) => {
            let _ = sink.release(k);
            let ts_ms = now_ms();
            elog.append(cfg.log_events, &event_log::format_line(ts_ms, "放開", &format!("送出 {k} 放開"), &src.tag()));
            let _ = event_tx.send(EngineEvent::KeyEmit {
                ts_ms,
                key: k.clone(),
                action: "up",
                source: src.clone(),
            });
        }
        KeyAction::MacroStart { name, source_id } => {
            if let Some(def) = cfg.macros.get(name) {
                macros.start(*source_id, name, def.clone());
                let ts_ms = now_ms();
                let src = Source::Button(*source_id).tag();
                elog.append(cfg.log_events, &event_log::format_line(ts_ms, "巨集", &format!("巨集「{name}」開始"), &src));
                let _ = event_tx.send(EngineEvent::MacroStart {
                    ts_ms,
                    name: name.clone(),
                    button: *source_id,
                });
            } else {
                tracing::error!(name = %name, "macro not found in config");
            }
        }
        KeyAction::MacroStop { source_id } => {
            macros.stop(*source_id);
            // MacroEnd with completed=false (interrupted by button release).
            // We don't track name per source_id here; emit a best-effort event.
        }
        KeyAction::MousePress(b) => {
            let _ = mouse_sink.click_press(*b);
        }
        KeyAction::MouseRelease(b) => {
            let _ = mouse_sink.click_release(*b);
        }
        KeyAction::MouseMove { dx, dy } => {
            let _ = mouse_sink.move_rel(*dx, *dy);
        }
        KeyAction::TouchpadClick { raw_x, raw_y, quadrant } => {
            let _ = event_tx.send(EngineEvent::TouchpadClick {
                raw_x: *raw_x,
                raw_y: *raw_y,
                quadrant: *quadrant,
            });
        }
        KeyAction::TouchpadHover { raw_x, raw_y, quadrant } => {
            let _ = event_tx.send(EngineEvent::TouchpadHover {
                raw_x: *raw_x,
                raw_y: *raw_y,
                quadrant: *quadrant,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OutputMode;

    #[test]
    fn nano_pad_mode_is_not_hidden() {
        let mut cfg: Config = serde_json::from_str(include_str!("../config.example.json")).unwrap();
        cfg.output_mode.clear();
        cfg.mapping_enabled.clear();
        assert!(hide_wanted_kinds(&cfg).contains(&ControllerKind::XboxNano), "鍵鼠 hides the Nano");
        cfg.output_mode.insert("xbox_nano".into(), OutputMode::Pad);
        assert!(!hide_wanted_kinds(&cfg).contains(&ControllerKind::XboxNano));
        cfg.output_mode.insert("xbox_nano".into(), OutputMode::Xbox);
        assert!(!hide_wanted_kinds(&cfg).contains(&ControllerKind::XboxNano), "a stray xbox is 手把 for the Nano");
    }

    #[test]
    fn hide_wanted_kinds_are_native_pads_in_kbm_or_xbox() {
        let mut cfg: Config = serde_json::from_str(include_str!("../config.example.json")).unwrap();
        cfg.output_mode.clear();
        cfg.mapping_enabled.clear();
        assert_eq!(hide_wanted_kinds(&cfg), vec![ControllerKind::DualSense, ControllerKind::NsPro, ControllerKind::XboxNano], "default 鍵鼠 is hidden");
        for k in ControllerKind::ALL {
            cfg.output_mode.insert(k.slug().into(), OutputMode::Pad);
        }
        assert!(hide_wanted_kinds(&cfg).is_empty(), "手把 is never hidden");
        cfg.output_mode.insert("dualsense".into(), OutputMode::Xbox);
        cfg.output_mode.insert("ns2_pro".into(), OutputMode::Kbm);
        assert_eq!(hide_wanted_kinds(&cfg), vec![ControllerKind::DualSense], "Switch 2 Pro has no native driver");
    }

    #[test]
    fn wants_pad_truth_table() {
        let modes = [OutputMode::Kbm, OutputMode::Xbox, OutputMode::Pad];
        for kind in ControllerKind::ALL {
            for mode in modes {
                for paused_or_off in [false, true] {
                    let want = !paused_or_off && mode == OutputMode::Xbox;
                    assert_eq!(wants_pad(paused_or_off, mode, Some(kind)), want, "{kind:?} {mode:?} {paused_or_off}");
                }
            }
        }
        for mode in modes {
            assert!(!wants_pad(false, mode, None), "no controller, no pad ({mode:?})");
        }
    }
}
