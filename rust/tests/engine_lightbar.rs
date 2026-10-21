//! The engine tells the lightbar whether the DualSense is on 鍵鼠: owner is
//! a DualSense, mapping ON, pad not 關, engine not paused.

#[allow(dead_code)] // only load_example() is used here
mod test_helpers;

use dualsense_mapper::engine::{Engine, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use std::time::{Duration, Instant};

fn spawn() -> (Engine, Handle) {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    (engine, h)
}

fn connect(h: &Handle, kind: ControllerKind) {
    h.fake_event(GamepadEvent::Connected(kind));
}

/// Poll up to 1 s for `lightbar_kbm() == want`; true if it got there.
fn settles(h: &Handle, want: bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if h.lightbar_kbm() == want {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.lightbar_kbm() == want
}

#[test]
fn dualsense_owner_with_mapping_is_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::DualSense);
    assert!(settles(&h, true));
    engine.shutdown();
}

#[test]
fn mapping_off_is_not_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::DualSense);
    assert!(settles(&h, true));
    h.config_write().mapping_enabled.insert("dualsense".into(), false);
    assert!(settles(&h, false));
    engine.shutdown();
}

#[test]
fn pad_off_is_not_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::DualSense);
    assert!(settles(&h, true));
    h.set_pad_off(ControllerKind::DualSense, true);
    assert!(settles(&h, false));
    engine.shutdown();
}

#[test]
fn paused_is_not_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::DualSense);
    assert!(settles(&h, true));
    h.set_paused(true);
    assert!(settles(&h, false));
    h.set_paused(false);
    assert!(settles(&h, true));
    engine.shutdown();
}

#[test]
fn other_owner_is_not_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::NsPro);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!h.lightbar_kbm());
    engine.shutdown();
}

#[test]
fn disconnected_is_not_kbm() {
    let (engine, h) = spawn();
    connect(&h, ControllerKind::DualSense);
    assert!(settles(&h, true));
    h.fake_event(GamepadEvent::Disconnected);
    assert!(settles(&h, false));
    engine.shutdown();
}
