//! HidHide through the engine (v3.6.0): the engine feeds the hider worker
//! the kinds wanted hidden (鍵鼠 or Xbox mode, native driver) and the connected
//! kinds; `get_xbox_status` reports `hidden` from the worker. A fake
//! HidHide stands in for the driver: tests never touch a real one.

#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::config::{Config, KnownPad, OutputMode};
use dualsense_mapper::engine::{Engine, EngineEvent, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use dualsense_mapper::gui::commands::{xbox_status_impl, XboxStatus};
use dualsense_mapper::hider::{Backend, HiderWorker};
use dualsense_mapper::hidhide::{Control, Error, HideState};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DS_USB: &str = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
const DS_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000";
const MOUSE: &str = r"HID\VID_046D&PID_C52B&MI_01&COL02\7&abc&0&0001";
const OWN: &str = r"\Device\HarddiskVolume3\Games\PadToKBM.exe";
const FOREIGN_DEV: &str = r"HID\VID_045E&PID_0B13\8&x&0&0000";

#[derive(Default)]
struct Fake {
    black: Vec<String>,
    white: Vec<String>,
    active: bool,
}

#[derive(Clone, Default)]
struct FakeCtl(Arc<Mutex<Fake>>);

impl FakeCtl {
    fn black(&self) -> Vec<String> {
        self.0.lock().unwrap().black.clone()
    }
}

impl Control for FakeCtl {
    fn blacklist(&self) -> Result<Vec<String>, Error> {
        Ok(self.0.lock().unwrap().black.clone())
    }
    fn set_blacklist(&self, list: &[String]) -> Result<(), Error> {
        self.0.lock().unwrap().black = list.to_vec();
        Ok(())
    }
    fn whitelist(&self) -> Result<Vec<String>, Error> {
        Ok(self.0.lock().unwrap().white.clone())
    }
    fn set_whitelist(&self, list: &[String]) -> Result<(), Error> {
        self.0.lock().unwrap().white = list.to_vec();
        Ok(())
    }
    fn active(&self) -> Result<bool, Error> {
        Ok(self.0.lock().unwrap().active)
    }
    fn set_active(&self, on: bool) -> Result<(), Error> {
        self.0.lock().unwrap().active = on;
        Ok(())
    }
    fn inverse(&self) -> Result<bool, Error> {
        Ok(false)
    }
}

/// The driver and the present HID instance ids (a test may change them).
struct FakeBackend(FakeCtl, Arc<Mutex<Vec<String>>>);

impl Backend for FakeBackend {
    type Ctl = FakeCtl;
    fn open(&mut self) -> Result<FakeCtl, Error> {
        Ok(self.0.clone())
    }
    fn present(&mut self) -> Vec<String> {
        self.1.lock().unwrap().clone()
    }
    fn own_path(&mut self) -> Option<String> {
        Some(OWN.to_string())
    }
}

fn xbox(kind: ControllerKind) -> Config {
    let mut cfg = test_helpers::load_example();
    cfg.output_mode.insert(kind.slug().into(), OutputMode::Xbox);
    cfg
}

fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(3);
    while !cond() {
        assert!(Instant::now() < until, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pad must stay hidden (never released) for a short window.
fn stays_hidden(r: &Rig) {
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        assert!(hidden(&r.h).is_some(), "released");
        assert!(r.ctl.black().contains(&DS_USB.to_string()), "released");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    state_path: std::path::PathBuf,
    ctl: FakeCtl,
    engine: Engine,
    h: Handle,
    worker: HiderWorker,
    present: Arc<Mutex<Vec<String>>>,
}

fn rig(cfg: Config) -> Rig {
    rig_with(cfg, |_, _| Box::new(|_| {}))
}

/// `learn(handle, config path)`: the worker's `on_learn`.
fn rig_with(
    cfg: Config,
    learn: impl FnOnce(&Handle, &std::path::Path) -> Box<dyn Fn(Vec<KnownPad>) + Send>,
) -> Rig {
    rig_present(cfg, &[MOUSE, DS_USB], learn)
}

fn rig_present(
    cfg: Config,
    present: &[&str],
    learn: impl FnOnce(&Handle, &std::path::Path) -> Box<dyn Fn(Vec<KnownPad>) + Send>,
) -> Rig {
    let present = Arc::new(Mutex::new(present.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("padtokbm.json");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let state_path = HideState::path_beside(&dir.path().join("padtokbm.json"));
    let ctl = FakeCtl(Arc::new(Mutex::new(Fake { black: vec![FOREIGN_DEV.into()], ..Default::default() })));
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    let worker = HiderWorker::spawn(
        state_path.clone(),
        FakeBackend(ctl.clone(), present.clone()),
        Duration::from_millis(20),
        Box::new(h.status_notifier()),
        learn(&h, &cfg_path),
    )
    .unwrap();
    h.attach_hider(worker.link());
    Rig { _dir: dir, state_path, ctl, engine, h, worker, present }
}

fn hidden(h: &Handle) -> Option<bool> {
    h.hider().unwrap().hidden(ControllerKind::DualSense)
}

#[test]
fn a_pad_is_hidden_in_kbm_and_xbox_and_released_on_pad() {
    // Default 鍵鼠 config: the pad is hidden too.
    let r = rig(test_helpers::load_example());
    // Wanted from the config alone, before the pad connects. It was present
    // when hidden, so a game may still hold it: one replug needed.
    eventually("first round", || hidden(&r.h) == Some(false));
    assert_eq!(r.ctl.black(), vec![FOREIGN_DEV.to_string(), DS_USB.to_string()], "present → hidden");
    assert_eq!(HideState::load(&r.state_path).ids, vec![DS_USB.to_string()], "recorded for crash recovery");

    // Xbox keeps it hidden.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Xbox);
    stays_hidden(&r);
    // Back to 鍵鼠: still hidden.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Kbm);
    stays_hidden(&r);

    // Leaves to 手把: the entry goes, the user's stays.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Pad);
    eventually("unhidden", || r.ctl.black() == vec![FOREIGN_DEV.to_string()]);
    // The status is published after the round that wrote the blacklist.
    eventually("not applicable", || hidden(&r.h).is_none());

    r.worker.stop();
    r.engine.shutdown();
    assert!(!r.state_path.exists());
}

#[test]
fn kbm_to_xbox_keeps_the_pad_hidden() {
    let r = rig(test_helpers::load_example());
    eventually("hidden in kbm", || r.ctl.black().contains(&DS_USB.to_string()));
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Xbox);
    let until = Instant::now() + Duration::from_millis(500);
    while Instant::now() < until {
        assert!(r.ctl.black().contains(&DS_USB.to_string()), "released between kbm and xbox");
        std::thread::sleep(Duration::from_millis(5));
    }
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn a_pad_hidden_while_connected_needs_a_replug_and_the_status_is_resent() {
    let r = rig(test_helpers::load_example());
    r.h.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    eventually("connected", || r.h.current_status().is_some());
    eventually("鍵鼠: hidden too, connected → needs a replug", || hidden(&r.h) == Some(false));

    // 手把 releases it; hiding again (Xbox) while connected needs a replug.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Pad);
    eventually("released", || hidden(&r.h) == None);
    r.h.drain_events();

    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Xbox);
    eventually("needs replug", || hidden(&r.h) == Some(false));
    eventually("status re-sent for the frontend to re-probe", || {
        r.h.drain_events().iter().any(|e| matches!(e, EngineEvent::ControllerConnected { .. }))
    });

    r.h.fake_event(GamepadEvent::Disconnected);
    eventually("left: hidden", || hidden(&r.h) == Some(true));
    assert!(r.ctl.black().contains(&DS_USB.to_string()), "kept for the reconnect");

    // Graceful exit undoes it.
    r.engine.shutdown();
    assert!(r.worker.stop());
    assert_eq!(r.ctl.black(), vec![FOREIGN_DEV.to_string()]);
}

// ── remembered pads (hidhide_known) ──

#[test]
fn a_remembered_pad_is_hidden_from_the_config_and_forgetting_it_unhides_it() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![KnownPad { kind: "dualsense".into(), id: DS_BT.into() }];
    let r = rig(cfg);
    // DS_BT is not present (the fake lists the USB one only).
    eventually("remembered id hidden", || r.ctl.black().contains(&DS_BT.to_string()));
    assert!(r.ctl.black().contains(&DS_USB.to_string()));
    // The settings' 刪除: the engine sends the new list, the worker drops it.
    r.h.config_write().hidhide_known.clear();
    eventually("forgotten id unhidden", || !r.ctl.black().contains(&DS_BT.to_string()));
    assert!(r.ctl.black().contains(&DS_USB.to_string()), "present and wanted: stays");
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn a_learned_pad_is_saved_to_the_config_and_announced() {
    let r = rig_with(test_helpers::load_example(), |h, p| dualsense_mapper::hider::learn_saver(h, p));
    let path = r.state_path.with_file_name("padtokbm.json");
    eventually("saved to the file", || {
        dualsense_mapper::config_io::ConfigDoc::load(&path)
            .map(|d| d.typed().hidhide_known.iter().any(|k| k.id == DS_USB && k.kind == "dualsense"))
            .unwrap_or(false)
    });
    assert_eq!(r.h.config_read().hidhide_known.len(), 1, "live config too");
    eventually("config-changed announced", || {
        r.h.drain_events().iter().any(|e| matches!(e, EngineEvent::ConfigChanged { .. }))
    });
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn present_pads_cannot_be_forgotten_and_forget_all_keeps_them() {
    use dualsense_mapper::gui::commands::{forget_all_hidden_pads_impl, forget_hidden_pad_impl, present_pad_ids_impl};
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![
        KnownPad { kind: "dualsense".into(), id: DS_USB.into() },
        KnownPad { kind: "dualsense".into(), id: DS_BT.into() },
    ];
    let r = rig(cfg);
    let path = r.state_path.with_file_name("padtokbm.json");
    eventually("a round ran", || present_pad_ids_impl(&r.h) == vec![DS_USB.to_string()]);
    let err = forget_hidden_pad_impl(&r.h, &path, &DS_USB.to_lowercase()).unwrap_err();
    assert!(format!("{err:#}").contains("連線中"), "{err:#}");
    assert_eq!(r.h.config_read().hidhide_known.len(), 2, "nothing written");
    assert!(forget_all_hidden_pads_impl(&r.h, &path).unwrap());
    let left: Vec<String> = r.h.config_read().hidhide_known.iter().map(|k| k.id.clone()).collect();
    assert_eq!(left, vec![DS_USB.to_string()], "only the absent one goes");
    assert!(!forget_all_hidden_pads_impl(&r.h, &path).unwrap(), "only present ones left: no change");
    r.worker.stop();
    r.engine.shutdown();
}

// ── the app powers a pad off when it needs a replug (Task 9) ──

use ControllerKind::DualSense as DS;

fn auto_off(h: &Handle) -> bool {
    xbox_status_impl(h, "dualsense", "installed", "installed").unwrap().auto_off
}

/// No power-off reaches the source for a while.
fn no_power_off(h: &Handle, why: &str) {
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        assert_eq!(h.fake_disconnects_received(), 0, "powered off: {why}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `hidden` and `auto_off` stay as they are for a while.
fn stays(r: &Rig, hid: Option<bool>, off: bool) {
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        assert_eq!((hidden(&r.h), auto_off(&r.h)), (hid, off));
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_pad_hidden_while_connected_is_powered_off_once() {
    let r = rig(test_helpers::load_example());
    r.h.fake_disconnects_received();
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("the app sends the ⏻", || r.h.fake_disconnects_received() == 1);
    assert!(!auto_off(&r.h), "not known yet whether a link went down");
    // The worker: the link went down (one pad), then the pad left.
    r.h.fake_power_off(DS, 1);
    r.h.fake_event(GamepadEvent::Disconnected);
    eventually("auto_off reported", || auto_off(&r.h));
    eventually("left: hidden", || hidden(&r.h) == Some(true));
    assert!(r.ctl.black().contains(&DS_USB.to_string()), "kept for the reconnect");
    // Turned back on: it arrives hidden, auto_off goes.
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("auto_off cleared on the reconnect", || !auto_off(&r.h));
    stays(&r, Some(true), false);
    no_power_off(&r.h, "hidden already");
    // Released (手把) and hidden again while connected: one replug needed,
    // but the app powers a pad off at most once per launch.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Pad);
    eventually("released", || hidden(&r.h).is_none());
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Xbox);
    eventually("needs a replug again", || hidden(&r.h) == Some(false));
    no_power_off(&r.h, "second time this launch");
    assert!(!auto_off(&r.h));
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn a_power_off_that_dropped_no_link_keeps_the_reconnect_hint() {
    let r = rig(test_helpers::load_example());
    r.h.fake_disconnects_received();
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("the app sends the ⏻", || r.h.fake_disconnects_received() == 1);
    // USB, or no Bluetooth link matched: nothing went down; the worker
    // dropped its handle and the pad comes straight back.
    r.h.fake_power_off(DS, 0);
    r.h.fake_event(GamepadEvent::Disconnected);
    stays(&r, Some(false), false);
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("connected", || r.h.current_status().is_some());
    stays(&r, Some(false), false);
    no_power_off(&r.h, "no retry");
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn a_remembered_pad_hidden_before_it_came_is_not_powered_off() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![KnownPad { kind: "dualsense".into(), id: DS_USB.into() }];
    let r = rig_present(cfg, &[MOUSE], |_, _| Box::new(|_| {}));
    eventually("hidden at start, absent", || r.ctl.black().contains(&DS_USB.to_string()));
    eventually("no replug", || hidden(&r.h) == Some(true));
    r.h.fake_disconnects_received();
    *r.present.lock().unwrap() = vec![MOUSE.to_string(), DS_USB.to_string()];
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("a round saw it", || dualsense_mapper::gui::commands::present_pad_ids_impl(&r.h) == vec![DS_USB.to_string()]);
    no_power_off(&r.h, "it arrived hidden");
    stays(&r, Some(true), false);
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn a_pad_not_wanted_hidden_is_not_powered_off() {
    let mut cfg = test_helpers::load_example();
    cfg.output_mode.insert("dualsense".into(), OutputMode::Pad);
    let r = rig(cfg);
    r.h.fake_disconnects_received();
    r.h.fake_event(GamepadEvent::Connected(DS));
    eventually("connected", || r.h.current_status().is_some());
    no_power_off(&r.h, "手把 mode hides nothing");
    assert_eq!(hidden(&r.h), None);
    r.worker.stop();
    r.engine.shutdown();
}

// ── get_xbox_status ──

#[test]
fn xbox_status_reports_hidden_from_the_worker() {
    let r = rig(xbox(ControllerKind::DualSense));
    eventually("first round", || hidden(&r.h) == Some(false));
    let st = xbox_status_impl(&r.h, "dualsense", "installed", "installed").unwrap();
    assert_eq!(st, XboxStatus { vigem: "installed".into(), hidhide: "installed".into(), hidden: Some(false), auto_off: false });
    assert_eq!(
        serde_json::to_value(&st).unwrap(),
        serde_json::json!({ "vigem": "installed", "hidhide": "installed", "hidden": false, "auto_off": false })
    );
    // HidHide missing: hidden is not applicable.
    let st = xbox_status_impl(&r.h, "dualsense", "missing", "missing").unwrap();
    assert_eq!(st.hidden, None);
    // 鍵鼠 is hidden too; the Switch 2 Pro has no native driver, never hidden.
    // (ns_pro is absent from the fake's present list: hidden, no replug needed.)
    eventually("ns_pro wanted", || xbox_status_impl(&r.h, "ns_pro", "installed", "installed").unwrap().hidden == Some(true));
    assert_eq!(xbox_status_impl(&r.h, "ns2_pro", "installed", "installed").unwrap().hidden, None);
    assert!(xbox_status_impl(&r.h, "joycon", "installed", "installed").is_err());
    r.worker.stop();
    r.engine.shutdown();
}

#[test]
fn xbox_status_without_a_worker_says_hidhide_is_unsupported() {
    // Dry run / not Windows: no worker, HidHide is never touched.
    let engine = Engine::spawn_with_fake_gamepad(xbox(ControllerKind::DualSense)).unwrap();
    let h = engine.handle();
    assert!(h.hider().is_none());
    let st = xbox_status_impl(&h, "dualsense", "installed", "installed").unwrap();
    assert_eq!(
        serde_json::to_value(&st).unwrap(),
        serde_json::json!({ "vigem": "installed", "hidhide": "unsupported", "hidden": null, "auto_off": false })
    );
    h.retry_hiding(); // no worker: a no-op
    // No worker, no replug: a connected pad is never powered off.
    h.fake_disconnects_received();
    h.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    eventually("connected", || h.current_status().is_some());
    no_power_off(&h, "no hider");
    engine.shutdown();
}

#[test]
fn start_for_starts_nothing_on_a_dry_run() {
    let engine = Engine::spawn_with_fake_gamepad(xbox(ControllerKind::DualSense)).unwrap();
    let h = engine.handle();
    let dir = tempfile::tempdir().unwrap();
    assert!(dualsense_mapper::hider::start_for(&h, &dir.path().join("padtokbm.json"), true).is_none());
    assert!(h.hider().is_none());
    engine.shutdown();
}
