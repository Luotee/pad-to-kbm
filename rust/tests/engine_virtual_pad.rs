//! Virtual Xbox pad for any pad in Xbox output mode: the engine plugs it
//! only for a session whose `output_mode` is `xbox`, feeds it every event,
//! unplugs it on another mode / Disconnected / pause / shutdown, never
//! retries a failed plug in a loop, plugs off the engine thread (a slow or
//! stuck plug never stalls the loop), and surfaces `vigem_missing` and the
//! BLE worker's warnings in the controller status.

#[allow(dead_code)] // only load_example() is used here
mod test_helpers;

use dualsense_mapper::config::{Config, OutputMode};
use dualsense_mapper::engine::{Engine, EngineEvent, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use dualsense_mapper::safety::SharedKeyState;
use dualsense_mapper::virtual_pad::{xbutton, XState};
use dualsense_mapper::xinput_sink::{PadError, VirtualPad};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const STEP: Duration = Duration::from_millis(100);

/// The example config with `kind` set to Xbox output. Every pad defaults
/// to 鍵鼠, so these tests say Xbox explicitly.
fn xbox(kind: ControllerKind) -> Config {
    let mut cfg = test_helpers::load_example();
    cfg.output_mode.insert(kind.slug().into(), OutputMode::Xbox);
    cfg
}

fn ns2_xbox() -> Config {
    xbox(ControllerKind::Ns2Pro)
}

#[derive(Debug, Clone, PartialEq)]
enum Op {
    /// `held` = keys the engine still held when the pad was plugged.
    Plug { held: usize },
    Update(XState),
    Unplug,
}

#[derive(Default)]
struct Shared {
    ops: Vec<Op>,
    key_state: Option<SharedKeyState>,
    /// The next this-many `update` calls fail (not recorded as `Op::Update`).
    failing_updates: usize,
}

/// Records every call. `fail` makes every `plug` return that error.
/// `gate`: each `plug` blocks until a `()` arrives or the sender is dropped
/// (a slow or stuck ViGEmBus `wait_ready`).
struct FakePad {
    shared: Arc<Mutex<Shared>>,
    fail: Option<PadError>,
    gate: Option<mpsc::Receiver<()>>,
}

impl VirtualPad for FakePad {
    fn plug(&mut self) -> Result<(), PadError> {
        let mut s = self.shared.lock().unwrap();
        let held = s
            .key_state
            .as_ref()
            .map_or(0, |k| k.lock().unwrap_or_else(|p| p.into_inner()).len_held());
        s.ops.push(Op::Plug { held });
        drop(s);
        if let Some(g) = &self.gate {
            let _ = g.recv();
        }
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
    fn update(&mut self, st: &XState) -> Result<(), PadError> {
        let mut s = self.shared.lock().unwrap();
        if s.failing_updates > 0 {
            s.failing_updates -= 1;
            return Err(PadError::Other("WinError(259)".into()));
        }
        s.ops.push(Op::Update(*st));
        Ok(())
    }
    fn unplug(&mut self) {
        self.shared.lock().unwrap().ops.push(Op::Unplug);
    }
}

struct Rig {
    engine: Engine,
    h: Handle,
    shared: Arc<Mutex<Shared>>,
}

impl Rig {
    fn new(cfg: Config, fail: Option<PadError>) -> Self {
        Self::build(cfg, fail, None)
    }
    /// Every plug waits for a `()` on the returned sender.
    fn gated(cfg: Config) -> (Self, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel();
        (Self::build(cfg, None, Some(rx)), tx)
    }
    fn build(cfg: Config, fail: Option<PadError>, gate: Option<mpsc::Receiver<()>>) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let pad = FakePad { shared: shared.clone(), fail, gate };
        let engine = Engine::spawn_with_fake_gamepad_and_pad(cfg, Box::new(pad)).expect("spawn");
        let h = engine.handle();
        shared.lock().unwrap().key_state = Some(h.key_state());
        Rig { engine, h, shared }
    }
    fn ops(&self) -> Vec<Op> {
        self.shared.lock().unwrap().ops.clone()
    }
    fn count(&self, f: impl Fn(&Op) -> bool) -> usize {
        self.ops().iter().filter(|o| f(o)).count()
    }
    fn plugs(&self) -> usize {
        self.count(|o| matches!(o, Op::Plug { .. }))
    }
    fn unplugs(&self) -> usize {
        self.count(|o| matches!(o, Op::Unplug))
    }
    fn last_update(&self) -> Option<XState> {
        self.ops().iter().rev().find_map(|o| match o {
            Op::Update(s) => Some(*s),
            _ => None,
        })
    }
    fn event(&self, ev: GamepadEvent) {
        self.h.fake_event(ev);
        std::thread::sleep(STEP);
    }
    fn set_mode(&self, slug: &str, m: OutputMode) {
        self.h.config_write().output_mode.insert(slug.into(), m);
        std::thread::sleep(STEP);
    }
    fn warnings(&self) -> Vec<String> {
        self.h.current_status().map(|s| s.warnings).unwrap_or_default()
    }
}

/// Plugged never goes above 1 or below 0, and each Plug that succeeded is
/// matched by at most one Unplug.
fn assert_balanced(ops: &[Op]) {
    let mut plugged = 0i32;
    for op in ops {
        match op {
            Op::Plug { .. } => plugged += 1,
            Op::Unplug => plugged -= 1,
            Op::Update(_) => assert_eq!(plugged, 1, "update while unplugged: {ops:?}"),
        }
        assert!((0..=1).contains(&plugged), "plug balance broken: {ops:?}");
    }
}

fn status_events(h: &Handle) -> Vec<Vec<String>> {
    h.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ControllerConnected { warnings, .. } => Some(warnings),
            _ => None,
        })
        .collect()
}

#[test]
fn ns2_mapping_off_plugs_and_feeds() {
    let r = Rig::new(ns2_xbox(), None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 1, "NS2 Pro + mapping OFF plugs the pad: {:?}", r.ops());

    r.h.fake_event(GamepadEvent::ButtonDown(0)); // NS B → Xbox A
    r.h.fake_event(GamepadEvent::ButtonDown(3)); // NS X → Xbox Y
    r.h.fake_event(GamepadEvent::Stick { axis: 1, value: 1.0 });
    r.h.fake_event(GamepadEvent::Trigger { axis: 5, value: 1.0 });
    std::thread::sleep(STEP);
    let s = r.last_update().expect("state pushed to the pad");
    assert_eq!(s.buttons, xbutton::A | xbutton::Y);
    assert_eq!((s.ly, s.rt), (32767, 255));
    assert_eq!(r.h.held_keys_count(), 0, "mapping OFF synthesizes no key");

    r.event(GamepadEvent::ButtonUp(0));
    assert_eq!(r.last_update().unwrap().buttons, xbutton::Y);

    let ops = r.ops();
    assert_balanced(&ops);
    r.engine.shutdown();
}

#[test]
fn failed_update_is_retried_next_tick() {
    // The bus can refuse a report (WinError 259: the XInput side has no
    // request pending yet). The latest state must still get through, even
    // when no further event arrives to trigger another update.
    let r = Rig::new(ns2_xbox(), None);
    r.shared.lock().unwrap().failing_updates = 3;
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    r.event(GamepadEvent::ButtonDown(0));
    assert_eq!(r.shared.lock().unwrap().failing_updates, 0, "failures consumed");
    assert_eq!(r.last_update().map(|s| s.buttons), Some(xbutton::A), "{:?}", r.ops());
    r.event(GamepadEvent::ButtonUp(0));
    assert_eq!(r.last_update().map(|s| s.buttons), Some(0));
    r.engine.shutdown();
}

#[test]
fn ns2_mapping_on_unplugs_and_synths_keys() {
    let r = Rig::new(ns2_xbox(), None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 1);

    r.set_mode("ns2_pro", OutputMode::Kbm);
    assert_eq!(r.unplugs(), 1, "mapping ON unplugs the pad: {:?}", r.ops());
    let n = r.ops().len();

    // Button 0 is bound to a key in the example config.
    r.event(GamepadEvent::ButtonDown(0));
    assert_eq!(r.h.held_keys_count(), 1, "mapping ON synthesizes keys again");
    assert_eq!(r.ops().len(), n, "no pad traffic while mapping ON: {:?}", r.ops());
    r.event(GamepadEvent::ButtonUp(0));
    assert_eq!(r.h.held_keys_count(), 0);

    assert_balanced(&r.ops());
    r.engine.shutdown();
}

#[test]
fn toggle_on_off_balanced() {
    let r = Rig::new(test_helpers::load_example(), None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 0, "mapping ON never plugs");

    for _ in 0..10 {
        // Hold a bound key, then flip mapping OFF: keys go up before the pad
        // appears.
        r.event(GamepadEvent::ButtonDown(0));
        assert_eq!(r.h.held_keys_count(), 1);
        r.set_mode("ns2_pro", OutputMode::Xbox);
        assert_eq!(r.h.held_keys_count(), 0, "ON→OFF releases held keys");
        r.event(GamepadEvent::ButtonUp(0));
        r.set_mode("ns2_pro", OutputMode::Kbm);
    }
    let ops = r.ops();
    assert_eq!(r.plugs(), 10, "{ops:?}");
    assert_eq!(r.unplugs(), 10, "{ops:?}");
    assert!(
        ops.iter().all(|o| !matches!(o, Op::Plug { held } if *held != 0)),
        "pad plugged while keys were still held: {ops:?}"
    );
    assert_balanced(&ops);
    r.engine.shutdown();
}

#[test]
fn disconnect_unplugs() {
    let r = Rig::new(ns2_xbox(), None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    r.event(GamepadEvent::ButtonDown(1));
    assert_eq!(r.plugs(), 1);

    r.event(GamepadEvent::Disconnected);
    assert_eq!(r.unplugs(), 1, "Disconnected unplugs: {:?}", r.ops());

    // A new session plugs a fresh pad whose state starts neutral (the held
    // button of the old session is forgotten).
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 2);
    r.event(GamepadEvent::ButtonDown(2));
    assert_eq!(r.last_update().unwrap().buttons, xbutton::X);

    // Disconnected and the next Connected in one batch: the old pad still
    // goes away before the new session's pad appears.
    r.h.fake_event(GamepadEvent::Disconnected);
    r.h.fake_event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    std::thread::sleep(STEP);
    assert_eq!((r.plugs(), r.unplugs()), (3, 2), "{:?}", r.ops());

    assert_balanced(&r.ops());
    r.engine.shutdown();
}

#[test]
fn shutdown_and_pause_unplug() {
    let r = Rig::new(ns2_xbox(), None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 1);

    // Pause: the app stops forwarding input, so the pad must not stay
    // plugged with a frozen state.
    r.h.set_paused(true);
    std::thread::sleep(STEP);
    assert_eq!(r.unplugs(), 1, "pause unplugs: {:?}", r.ops());
    r.h.set_paused(false);
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 2, "unpause plugs again: {:?}", r.ops());

    let shared = r.shared.clone();
    r.engine.shutdown();
    let ops = shared.lock().unwrap().ops.clone();
    assert_eq!(ops.iter().filter(|o| matches!(o, Op::Unplug)).count(), 2, "shutdown unplugs: {ops:?}");
    assert_balanced(&ops);
}

#[test]
fn bus_not_found_sets_warning_no_retry_storm() {
    let r = Rig::new(ns2_xbox(), Some(PadError::BusNotFound));
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    for i in 0..5 {
        r.event(GamepadEvent::ButtonDown(i));
    }
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 1, "one plug attempt per Connected: {:?}", r.ops());
    assert_eq!(r.unplugs(), 0, "nothing to unplug after a failed plug");
    assert!(r.warnings().contains(&"vigem_missing".to_string()), "got {:?}", r.warnings());
    assert!(
        status_events(&r.h).iter().any(|w| w.contains(&"vigem_missing".to_string())),
        "controller-status event carries vigem_missing"
    );
    for i in 0..5 {
        r.h.fake_event(GamepadEvent::ButtonUp(i));
    }

    // Mapping ON still works.
    r.set_mode("ns2_pro", OutputMode::Kbm);
    r.event(GamepadEvent::ButtonDown(0));
    assert_eq!(r.h.held_keys_count(), 1, "mapping ON works without ViGEmBus");
    r.event(GamepadEvent::ButtonUp(0));

    // A mapping OFF edge may retry once.
    r.set_mode("ns2_pro", OutputMode::Xbox);
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 2, "OFF edge retries once: {:?}", r.ops());

    // The warning belongs to the session; a fresh Connected retries once.
    r.event(GamepadEvent::Disconnected);
    assert!(r.h.current_status().is_none());
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 3, "fresh Connected retries once: {:?}", r.ops());
    assert!(r.warnings().contains(&"vigem_missing".to_string()));

    r.engine.shutdown();
}

/// A pre-v3.6.0 `mapping_enabled: false` on a pad with a native driver
/// means 手把 (`OutputMode::Pad`): the game reads the pad, nothing is plugged.
#[test]
fn dualsense_mapping_off_never_plugs() {
    let mut cfg = test_helpers::load_example();
    cfg.mapping_enabled.insert("dualsense".into(), false);
    cfg.mapping_enabled.insert("ns_pro".into(), false);
    let r = Rig::new(cfg, None);
    for kind in [ControllerKind::DualSense, ControllerKind::NsPro] {
        r.event(GamepadEvent::Connected(kind));
        r.event(GamepadEvent::ButtonDown(0));
        r.event(GamepadEvent::ButtonUp(0));
        r.event(GamepadEvent::Disconnected);
    }
    assert_eq!(r.ops(), vec![], "手把 mode plugs no virtual pad");
    assert!(r.warnings().is_empty());
    r.engine.shutdown();
}

#[test]
fn warnings_from_ble_channel_reach_status() {
    let r = Rig::new(ns2_xbox(), None);
    // Arrives before any controller: kept for the process lifetime.
    r.h.fake_warning("ble_rate_limited_win10");
    std::thread::sleep(STEP);
    assert!(r.h.current_status().is_none());

    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.warnings(), vec!["ble_rate_limited_win10".to_string()]);
    assert!(
        status_events(&r.h).iter().any(|w| w == &vec!["ble_rate_limited_win10".to_string()]),
        "Connected event carries the warning"
    );

    // A new warning while connected re-emits the status; duplicates are
    // ignored.
    r.h.fake_warning("something_else");
    r.h.fake_warning("ble_rate_limited_win10");
    std::thread::sleep(STEP);
    let want = vec!["ble_rate_limited_win10".to_string(), "something_else".to_string()];
    assert_eq!(r.warnings(), want);
    let evs = status_events(&r.h);
    assert_eq!(evs.last(), Some(&want), "status re-emitted on change: {evs:?}");

    // Survives a reconnect.
    r.event(GamepadEvent::Disconnected);
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.warnings(), want);
    r.engine.shutdown();
}

#[test]
fn retry_virtual_pad_after_install_plugs_again() {
    let r = Rig::new(ns2_xbox(), Some(PadError::BusNotFound));
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 1);
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 1, "no retry storm");
    r.h.retry_virtual_pad();
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 2, "one more attempt after an install");
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 2);
    r.engine.shutdown();
}

#[test]
fn retry_virtual_pad_respects_keys_mode() {
    let r = Rig::new(test_helpers::load_example(), Some(PadError::BusNotFound));
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    r.h.retry_virtual_pad();
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 0, "鍵鼠 mode never plugs");
    r.engine.shutdown();
}

#[test]
fn every_pad_in_xbox_mode_plugs_and_feeds() {
    // The Xbox Nano is the exception: its Xbox is 手把 (`nano_never_plugs_a_virtual_pad`).
    for kind in ControllerKind::ALL.into_iter().filter(|k| *k != ControllerKind::XboxNano) {
        let r = Rig::new(xbox(kind), None);
        r.event(GamepadEvent::Connected(kind));
        assert_eq!(r.plugs(), 1, "{kind:?} in Xbox mode plugs: {:?}", r.ops());
        // Ids 0..=14 are positional and shared by every pad (DualSense 0 =
        // Cross, NS 0 = B): bottom face button → Xbox A.
        r.event(GamepadEvent::ButtonDown(0));
        r.h.fake_event(GamepadEvent::Stick { axis: 0, value: -1.0 });
        std::thread::sleep(STEP);
        let s = r.last_update().expect("state pushed");
        assert_eq!((s.buttons, s.lx), (xbutton::A, -32767), "{kind:?}");
        assert_eq!(r.h.held_keys_count(), 0, "{kind:?}: Xbox mode synthesizes no key");
        r.event(GamepadEvent::ButtonUp(0));
        assert_eq!(r.last_update().unwrap().buttons, 0);
        let shared = r.shared.clone();
        r.engine.shutdown();
        let ops = shared.lock().unwrap().ops.clone();
        assert_eq!(ops.iter().filter(|o| matches!(o, Op::Unplug)).count(), 1, "{kind:?}: {ops:?}");
        assert_balanced(&ops);
    }
}

#[test]
fn kbm_and_pad_modes_never_plug() {
    for kind in [ControllerKind::DualSense, ControllerKind::NsPro] {
        for mode in [OutputMode::Kbm, OutputMode::Pad] {
            let mut cfg = test_helpers::load_example();
            cfg.output_mode.insert(kind.slug().into(), mode);
            let r = Rig::new(cfg, None);
            r.event(GamepadEvent::Connected(kind));
            r.event(GamepadEvent::ButtonDown(0));
            r.event(GamepadEvent::ButtonUp(0));
            assert_eq!(r.ops(), vec![], "{kind:?} {mode:?} plugs nothing");
            r.engine.shutdown();
        }
    }
    // The Switch 2 Pro has no 手把 path: `pad` is Xbox for it.
    let mut cfg = test_helpers::load_example();
    cfg.output_mode.insert("ns2_pro".into(), OutputMode::Pad);
    let r = Rig::new(cfg, None);
    r.event(GamepadEvent::Connected(ControllerKind::Ns2Pro));
    assert_eq!(r.plugs(), 1, "{:?}", r.ops());
    r.engine.shutdown();
}

#[test]
fn slow_plug_does_not_stall_the_engine() {
    // vigem-client's `wait_ready` can block (for good, in the worst case).
    // The plug runs off the engine thread: the status and the button
    // events keep coming while it waits.
    let (r, gate) = Rig::gated(xbox(ControllerKind::DualSense));
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.plugs(), 1, "plug started");
    assert!(r.h.current_status().is_some(), "status published while the plug is pending");
    r.h.drain_events();
    r.event(GamepadEvent::ButtonDown(0));
    assert!(
        r.h.drain_events().iter().any(|e| matches!(e, EngineEvent::ButtonDown { id: 0 })),
        "events keep flowing while the plug is pending"
    );

    // No second plug while one is in flight: not on a retry, not on a new
    // session.
    r.h.retry_virtual_pad();
    r.event(GamepadEvent::Disconnected);
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    r.event(GamepadEvent::ButtonDown(0));
    assert_eq!(r.plugs(), 1, "{:?}", r.ops());
    assert_eq!(r.last_update(), None, "nothing pushed before the plug returns");

    // The plug returns: the state fed meanwhile is pushed.
    gate.send(()).unwrap();
    std::thread::sleep(STEP);
    assert_eq!(r.last_update().map(|s| s.buttons), Some(xbutton::A), "{:?}", r.ops());
    assert_balanced(&r.ops());
    drop(gate);
    r.engine.shutdown();
}

#[test]
fn leaving_xbox_mode_while_plugging_unplugs_after() {
    let (r, gate) = Rig::gated(xbox(ControllerKind::DualSense));
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.plugs(), 1);
    r.set_mode("dualsense", OutputMode::Kbm);
    assert_eq!(r.unplugs(), 0, "nothing plugged yet: {:?}", r.ops());

    // 鍵鼠 works while the plug is still pending.
    r.event(GamepadEvent::ButtonDown(0));
    assert_eq!(r.h.held_keys_count(), 1);
    r.event(GamepadEvent::ButtonUp(0));

    gate.send(()).unwrap();
    std::thread::sleep(STEP);
    assert_eq!(r.ops(), vec![Op::Plug { held: 0 }, Op::Unplug], "unplugged once the plug returned");
    std::thread::sleep(STEP);
    assert_eq!(r.plugs(), 1, "no new plug in 鍵鼠 mode");
    drop(gate);
    r.engine.shutdown();
}

/// Shut the engine down on another thread; `None` if it took over `limit`.
fn timed_shutdown(engine: Engine, limit: Duration) -> Option<Duration> {
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        engine.shutdown();
        let _ = tx.send(());
    });
    rx.recv_timeout(limit).ok().map(|()| t0.elapsed())
}

#[test]
fn shutdown_does_not_hang_on_a_stuck_plug() {
    let (r, gate) = Rig::gated(xbox(ControllerKind::DualSense));
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.plugs(), 1);
    let took = timed_shutdown(r.engine, Duration::from_secs(5));
    drop(gate); // let the stuck plug thread end
    let took = took.expect("shutdown returned while the plug was stuck");
    assert!(took < Duration::from_secs(4), "shutdown took {took:?}");
}

#[test]
fn shutdown_waits_briefly_for_a_plug_then_unplugs() {
    let (r, gate) = Rig::gated(xbox(ControllerKind::DualSense));
    r.event(GamepadEvent::Connected(ControllerKind::DualSense));
    assert_eq!(r.plugs(), 1);
    let shared = r.shared.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let _ = gate.send(());
    });
    timed_shutdown(r.engine, Duration::from_secs(5)).expect("shutdown returned");
    let ops = shared.lock().unwrap().ops.clone();
    assert_eq!(ops.first(), Some(&Op::Plug { held: 0 }));
    assert_eq!(ops.last(), Some(&Op::Unplug), "a plug that lands during shutdown is unplugged: {ops:?}");
    assert_balanced(&ops);
}

/// The Nano's Xbox and 手把 are one output: whatever the config says, no
/// virtual pad is ever plugged for it.
#[test]
fn nano_never_plugs_a_virtual_pad() {
    let r = Rig::new(xbox(ControllerKind::XboxNano), None);
    r.event(GamepadEvent::Connected(ControllerKind::XboxNano));
    r.event(GamepadEvent::ButtonDown(0));
    r.event(GamepadEvent::ButtonUp(0));
    for m in [OutputMode::Xbox, OutputMode::Pad, OutputMode::Kbm, OutputMode::Xbox] {
        r.set_mode("xbox_nano", m);
        r.event(GamepadEvent::ButtonDown(0));
        r.event(GamepadEvent::ButtonUp(0));
    }
    r.event(GamepadEvent::Disconnected);
    assert_eq!(r.plugs(), 0, "no virtual pad for the Nano; ops: {:?}", r.ops());
    assert!(r.warnings().is_empty());
    r.engine.shutdown();
}
