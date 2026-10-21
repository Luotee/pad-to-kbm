//! End-to-end check that a `Binding::Mouse(Left)` on a touchpad
//! quadrant id round-trips through the engine, the mapper, and the
//! MouseSink + safety refcount.

use dualsense_mapper::config::{Binding, ButtonEntry, Config, MouseButton};
use dualsense_mapper::engine::Engine;
use std::collections::BTreeMap;

fn cfg_with_touchpad_tl_left_click() -> Config {
    let mut buttons: BTreeMap<String, ButtonEntry> = BTreeMap::new();
    for id in dualsense_mapper::config::VALID_BUTTON_IDS {
        buttons.insert(
            id.to_string(),
            ButtonEntry { label: format!("b{id}"), binding: Binding::Unbound },
        );
    }
    // Touchpad TL → mouse left click.
    buttons.insert(
        "25".into(),
        ButtonEntry { label: "Touchpad TL".into(), binding: Binding::Mouse(MouseButton::Left) },
    );
    Config {
        version: 2,
        deadzone: 0.4,
        trigger_threshold: 0.5,
        min_press_ms: [8, 25],
        tick_jitter_ms: [0, 3],
        log_events: false,
        pads: dualsense_mapper::gamepad::ControllerKind::ALL
            .into_iter()
            .map(|k| (k.slug().to_string(), dualsense_mapper::pad_bindings::PadBindings {
                buttons: buttons.clone(),
                quick_l: BTreeMap::new(),
                quick_r: BTreeMap::new(),
                trigger_mode: Default::default(),
            }))
            .collect(),
        macros: BTreeMap::new(),
        touchpad_cursor_enabled: true,
        touchpad_cursor_sensitivity: 1.5,
        touchpad_midpoint_x: 960,
        touchpad_midpoint_y: 540,
        touchpad_accel_slow_threshold: 5,
        touchpad_accel_fast_threshold: 20,
        touchpad_accel_gain_slow: 0.5,
        touchpad_accel_gain_fast: 1.5,
        touchpad_deadzone_radius: 2,
        touchpad_click_freeze_enabled: true,
        mapping_enabled: Default::default(),
        rumble_on_connect: true,
        stick_cal: Default::default(),
        last_pad: None,
        lightbar: Default::default(),
    }
}

#[test]
fn touchpad_tl_press_release_round_trips_mouse_left() {
    let engine = Engine::spawn_with_fake_gamepad(cfg_with_touchpad_tl_left_click()).expect("spawn");
    let h = engine.handle();

    h.fake_button_down(25);
    // Engine loop ticks every 8 ms; give a couple of ticks.
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(
        h.key_state().lock().unwrap().is_mouse_held(MouseButton::Left),
        "Mouse(Left) should be held after ButtonDown(25)"
    );

    h.fake_button_up(25);
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(
        !h.key_state().lock().unwrap().is_mouse_held(MouseButton::Left),
        "Mouse(Left) should be released after ButtonUp(25)"
    );

    engine.shutdown();
}

#[test]
fn shutdown_drains_held_mouse_buttons() {
    let engine = Engine::spawn_with_fake_gamepad(cfg_with_touchpad_tl_left_click()).expect("spawn");
    let h = engine.handle();
    let state = h.key_state();

    h.fake_button_down(25);
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert!(state.lock().unwrap().is_mouse_held(MouseButton::Left));

    // Shutdown without an explicit ButtonUp — Iron rule #3: MouseSink::Drop
    // (called from run_loop's explicit drop) drains held mouse buttons via
    // the shared KeyState.
    engine.shutdown();

    assert!(!state.lock().unwrap().is_mouse_held(MouseButton::Left),
        "shutdown must drain held mouse buttons");
}
