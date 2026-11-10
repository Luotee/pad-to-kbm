//! 游標 (v3.9.0) through the running engine (fake gamepad, dry run): a
//! cursor stick moves the mouse every tick, and nothing moves while the
//! board captures a key, while paused, or while the pad is not on 鍵鼠.

use dualsense_mapper::config::{Binding, ButtonEntry, Config, OutputMode};
use dualsense_mapper::engine::{Engine, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use std::time::Duration;

const STEP: Duration = Duration::from_millis(150);

fn cfg_right_stick_cursor() -> Config {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let mut cfg = Config::load_from_path(&path).unwrap();
    for id in 19..=22 {
        cfg.pad_mut(ControllerKind::DualSense)
            .buttons
            .insert(id.to_string(), ButtonEntry { label: String::new(), binding: Binding::Cursor });
    }
    cfg.validate().unwrap();
    cfg
}

fn moved_during(h: &Handle, f: impl FnOnce()) -> (i64, i64) {
    std::thread::sleep(STEP);
    let before = h.fake_mouse_moved();
    f();
    std::thread::sleep(STEP);
    let after = h.fake_mouse_moved();
    (after.0 - before.0, after.1 - before.1)
}

#[test]
fn cursor_stick_moves_the_mouse_until_released() {
    let engine = Engine::spawn_with_fake_gamepad(cfg_right_stick_cursor()).unwrap();
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    h.fake_event(GamepadEvent::Stick { axis: 2, value: 1.0 });
    let (dx, dy) = moved_during(&h, || {});
    assert!(dx > 50, "150 ms of a full push right moves well over 50 px, got {dx}");
    assert_eq!(dy, 0);
    assert_eq!(h.held_keys_count(), 0, "the cursor presses no key");

    h.fake_event(GamepadEvent::Stick { axis: 2, value: 0.0 });
    std::thread::sleep(STEP);
    assert_eq!(moved_during(&h, || {}), (0, 0), "back in the centre: still");
    engine.shutdown();
}

#[test]
fn nothing_moves_while_capturing_paused_or_off_kbm() {
    let engine = Engine::spawn_with_fake_gamepad(cfg_right_stick_cursor()).unwrap();
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    h.fake_event(GamepadEvent::Stick { axis: 2, value: 1.0 });
    assert!(moved_during(&h, || {}).0 > 0, "moving to begin with");

    h.set_capture_active(true);
    assert_eq!(moved_during(&h, || {}), (0, 0), "key capture: the board owns the input");
    h.set_capture_active(false);
    assert!(moved_during(&h, || {}).0 > 0);

    h.set_paused(true);
    assert_eq!(moved_during(&h, || {}), (0, 0), "paused");
    h.set_paused(false);
    // A pause forgets the stick; the next report brings it back.
    h.fake_event(GamepadEvent::Stick { axis: 2, value: 1.0 });
    assert!(moved_during(&h, || {}).0 > 0);

    h.config_write().output_mode.insert("dualsense".into(), OutputMode::Pad);
    assert_eq!(moved_during(&h, || {}), (0, 0), "手把 mode synthesizes nothing");
    engine.shutdown();
}
