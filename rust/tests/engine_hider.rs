//! HidHide through the engine (v3.6.0): the engine feeds the hider worker
//! the kinds wanted hidden (鍵鼠 or Xbox mode, native driver) and the connected
//! kinds; `get_xbox_status` reports `hidden` from the worker. A fake
//! HidHide stands in for the driver: tests never touch a real one.

#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::config::{Config, OutputMode};
use dualsense_mapper::engine::{Engine, EngineEvent, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use dualsense_mapper::gui::commands::{xbox_status_impl, XboxStatus};
use dualsense_mapper::hider::{Backend, HiderWorker};
use dualsense_mapper::hidhide::{Control, Error, HideState};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DS_USB: &str = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
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

struct FakeBackend(FakeCtl);

impl Backend for FakeBackend {
    type Ctl = FakeCtl;
    fn open(&mut self) -> Result<FakeCtl, Error> {
        Ok(self.0.clone())
    }
    fn present(&mut self) -> Vec<String> {
        vec![MOUSE.to_string(), DS_USB.to_string()]
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

struct Rig {
    _dir: tempfile::TempDir,
    state_path: std::path::PathBuf,
    ctl: FakeCtl,
    engine: Engine,
    h: Handle,
    worker: HiderWorker,
}

fn rig(cfg: Config) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let state_path = HideState::path_beside(&dir.path().join("padtokbm.json"));
    let ctl = FakeCtl(Arc::new(Mutex::new(Fake { black: vec![FOREIGN_DEV.into()], ..Default::default() })));
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    let worker = HiderWorker::spawn(
        state_path.clone(),
        FakeBackend(ctl.clone()),
        Duration::from_millis(20),
        Box::new(h.status_notifier()),
    )
    .unwrap();
    h.attach_hider(worker.link());
    Rig { _dir: dir, state_path, ctl, engine, h, worker }
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
    std::thread::sleep(Duration::from_millis(200));
    assert!(hidden(&r.h).is_some());
    assert!(r.ctl.black().contains(&DS_USB.to_string()));
    // Back to 鍵鼠: still hidden.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Kbm);
    std::thread::sleep(Duration::from_millis(200));
    assert!(hidden(&r.h).is_some());
    assert!(r.ctl.black().contains(&DS_USB.to_string()));

    // Leaves to 手把: the entry goes, the user's stays.
    r.h.config_write().output_mode.insert("dualsense".into(), OutputMode::Pad);
    eventually("unhidden", || r.ctl.black() == vec![FOREIGN_DEV.to_string()]);
    assert_eq!(hidden(&r.h), None);

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
    assert_eq!(hidden(&r.h), Some(false), "鍵鼠: hidden too, connected → needs a replug");

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

// ── get_xbox_status ──

#[test]
fn xbox_status_reports_hidden_from_the_worker() {
    let r = rig(xbox(ControllerKind::DualSense));
    eventually("first round", || hidden(&r.h) == Some(false));
    let st = xbox_status_impl(&r.h, "dualsense", "installed", "installed").unwrap();
    assert_eq!(st, XboxStatus { vigem: "installed".into(), hidhide: "installed".into(), hidden: Some(false) });
    assert_eq!(
        serde_json::to_value(&st).unwrap(),
        serde_json::json!({ "vigem": "installed", "hidhide": "installed", "hidden": false })
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
        serde_json::json!({ "vigem": "installed", "hidhide": "unsupported", "hidden": null })
    );
    h.retry_hiding(); // no worker: a no-op
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
