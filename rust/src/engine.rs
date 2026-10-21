//! Engine: long-running mapper loop with a thread-safe handle for the GUI.
//!
//! v0.1.x ran the loop directly from `app::run`. The GUI needs to mutate
//! config while the loop runs, pause it, suppress synth during key
//! capture, and shut it down. This module wraps that into:
//!
//!   * `Engine`      — owns the thread.
//!   * `Handle`      — what the GUI holds; clones cheaply (`Arc` inside).
//!   * `EngineEvent` — what the loop emits for the activity log.

use crate::config::Config;
use crate::event_log::{self, EventLog};
use crate::gamepad::lightbar::LightbarLive;
use crate::gamepad::stick_cal::{StickCalTable, StickPositions, StickProbe};
use crate::gamepad::{ControllerKind, CursorParams, GamepadSource, HubCmd, SourceCmd};
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
use std::time::Duration;

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
    /// The virtual Xbox pad. `Weak`: the engine thread owns it, so it is
    /// dropped (and unplugged) when that thread ends, even by unwinding.
    virtual_pad: Weak<Mutex<Box<dyn VirtualPad>>>,
    /// Folder for the `log_events` file, handed to the engine thread on its
    /// next tick (`Handle::set_log_dir`).
    pending_log_dir: Mutex<Option<PathBuf>>,
    /// One more virtual-pad plug attempt requested (`retry_virtual_pad`).
    retry_pad: AtomicBool,
}

// ─── Handle (GUI-side) ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Handle {
    inner: Arc<HandleInner>,
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
    /// installed). Plugs only if the owner still wants it (NS2 Pro, Xbox
    /// mode, not paused / off); clears `vigem_missing` until a plug fails again.
    pub fn retry_virtual_pad(&self) {
        self.inner.retry_pad.store(true, Ordering::SeqCst);
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
        let src = GamepadSource::new(cursor_params.clone(), lightbar.clone(), cmd_rx, warn_tx)?.with_stick_cal(cal.clone(), probe.clone());
        // Dry run synthesizes no OS input: that includes the virtual pad.
        let pad: Box<dyn VirtualPad> =
            if dry_run { Box::new(xinput_sink::NullPad) } else { xinput_sink::platform_pad() };
        Self::spawn_inner(cfg, dry_run, src, cursor_params, lightbar, cmd_tx, warn_rx, pad, cal, probe)
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
    ) -> Result<Self> {
        stick_cal.replace_from(&cfg);
        lightbar.set_config(cfg.lightbar);
        let key_state = safety::shared();
        let pad: SharedPad = Arc::new(Mutex::new(pad));
        let (event_tx, event_rx) = unbounded::<EngineEvent>();

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
        });

        let handle = Handle { inner: inner.clone() };

        let thread = thread::spawn(move || run_loop(inner, src, dry_run, pad));
        Ok(Engine { thread: Some(thread), handle })
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Signal shutdown and block until the engine thread exits.
    pub fn shutdown(mut self) {
        self.handle.inner.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
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

    // Per-controller mapping gate. `None` (no controller seen yet, or after
    // Disconnected) means mapping ON.
    let mut current_kind: Option<ControllerKind> = None;
    let mut mapping_on = true;
    // The owner is switched off (header 關): synthesize nothing, no virtual
    // pad. Polling goes on, so connects / disconnects are still seen.
    let mut pad_off = false;
    let mut last_off_pads: Vec<ControllerKind> = Vec::new();

    // Virtual Xbox pad (NS2 Pro, mapping OFF) and the status warnings.
    let mut pad = PadLink::new(pad);
    let mut worker_warnings: Vec<String> = Vec::new();
    // The connected pads last published in the status.
    let mut last_connected: Vec<ControllerKind> = Vec::new();
    // The `log_events` file (no folder until `Handle::set_log_dir`).
    let mut elog = EventLog::new(None);

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
            last_config_gen = current_gen;
        }

        // ── Mapping ON→OFF edge: release everything ──────────────────────────
        let now_on = mapping_enabled(&h, current_kind);
        if mapping_on && !now_on {
            drain_and_release(&mut macros, &macro_rx, &mut sink, &mut mouse_sink, mapper.config(), &h.event_tx, &mut elog);
            mapper.reset_input_state();
            // An OFF edge may retry a plug that failed earlier this session.
            pad.rearm();
        }
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

        // ── Virtual pad: plugged only for an NS2 Pro with mapping OFF ────────
        // After the OFF edge's release above (keys up, then the pad appears);
        // before polling on the ON edge (pad gone, then keys resume).
        warnings_changed |= pad.sync(wants_pad(paused || pad_off, mapping_on, current_kind));
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
                        let now_on = mapping_enabled(&h, current_kind);
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
                        pad.sync(wants_pad(paused || pad_off, mapping_on, current_kind));
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

                // The virtual pad mirrors every NS2 Pro event, whatever the
                // mapping state (it is pushed only while plugged), and is not
                // gated on key capture: it types nothing.
                if current_kind.map_or(false, ControllerKind::needs_virtual_pad) {
                    pad.feed(&ev);
                }

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

        // One pad report per tick at most.
        pad.flush();

        thread::sleep(Duration::from_millis(8));
    }

    // ── Graceful shutdown ────────────────────────────────────────────────────
    pad.sync(false);
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

/// Whether mapping is ON for the connected controller. Reads the live config
/// (not the mapper's snapshot) so an IPC toggle takes effect on the next tick.
/// No known controller → ON.
fn mapping_enabled(h: &HandleInner, kind: Option<ControllerKind>) -> bool {
    kind.map_or(true, |k| h.config.read().unwrap().mapping_enabled_for(k))
}

/// Whether the session owner `kind` is switched off.
fn is_off(off: &[ControllerKind], kind: Option<ControllerKind>) -> bool {
    kind.map_or(false, |k| off.contains(&k))
}

/// The virtual Xbox pad is plugged only for a connected NS2 Pro with mapping
/// OFF, and not while paused (no input is forwarded then, so a plugged pad
/// would freeze whatever it last reported).
fn wants_pad(paused: bool, mapping_on: bool, kind: Option<ControllerKind>) -> bool {
    !paused && !mapping_on && kind.map_or(false, ControllerKind::needs_virtual_pad)
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

/// Status warning: ViGEmBus is not installed, so the NS2 Pro gets no virtual
/// Xbox pad (mapping ON still works).
pub const WARN_VIGEM_MISSING: &str = "vigem_missing";

/// The virtual Xbox pad's lifecycle (NS2 Pro with mapping OFF). Owned by the
/// engine thread; the `SharedPad` is shared only with the panic hook.
struct PadLink {
    pad: SharedPad,
    plugged: bool,
    /// A plug failed this session. No retry until a fresh `Connected` or a
    /// mapping ON→OFF edge: a missing bus must not be retried every tick.
    plug_failed: bool,
    /// Session warning: ViGEmBus is not installed.
    bus_missing: bool,
    state: XState,
    /// `state` changed since the pad last accepted it.
    dirty: bool,
}

impl PadLink {
    fn new(pad: SharedPad) -> Self {
        PadLink { pad, plugged: false, plug_failed: false, bus_missing: false, state: XState::default(), dirty: false }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn VirtualPad>> {
        self.pad.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Plug or unplug to match `want`. Returns whether the session warnings
    /// changed.
    fn sync(&mut self, want: bool) -> bool {
        if want && !self.plugged && !self.plug_failed {
            let result = self.lock().plug();
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
        } else if !want && self.plugged {
            self.lock().unplug();
            self.plugged = false;
        }
        false
    }

    /// Allow one more plug attempt after a failure.
    fn rearm(&mut self) {
        self.plug_failed = false;
    }

    /// A controller session starts or ends: neutral state, no failed plug,
    /// no session warning.
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
    /// cannot leave a button pressed in the game.
    fn flush(&mut self) {
        if self.plugged && self.dirty {
            let s = self.state;
            let accepted = self.lock().update(&s).is_ok();
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
