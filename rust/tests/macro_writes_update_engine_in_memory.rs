//! Regression for v1.1.4 "macro UI doesn't refresh" — the IPC mutator path
//! must update the engine's in-memory config synchronously, so the frontend's
//! explicit reload (Task 1 ships an emit too, but the test below pins the
//! synchronous-state contract that the frontend's `await invoke()` + IPC
//! emit pipeline depends on).

use dualsense_mapper::config::{Binding, ButtonEntry, Config, MacroDef, MacroStep, StepAction};
use dualsense_mapper::config_io::{write_atomic, ConfigDoc};
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gui::commands::{
    delete_macro_impl, rename_macro_impl, set_binding_impl, set_macro_impl,
};
use std::collections::BTreeMap;
use tempfile::NamedTempFile;

fn baseline_config() -> Config {
    let mut buttons: BTreeMap<String, ButtonEntry> = BTreeMap::new();
    for id in dualsense_mapper::config::VALID_BUTTON_IDS {
        buttons.insert(
            id.to_string(),
            ButtonEntry { label: format!("b{id}"), binding: Binding::Unbound },
        );
    }
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

fn spawn_with_baseline_on_disk() -> (Engine, NamedTempFile) {
    let cfg = baseline_config();
    let tmp = NamedTempFile::new().expect("tmp");
    let doc = ConfigDoc::load_from_value(serde_json::to_value(&cfg).unwrap())
        .expect("doc from value");
    write_atomic(tmp.path(), &doc).expect("seed disk");
    let engine = Engine::spawn(cfg, /* dry_run */ true).expect("engine spawn");
    (engine, tmp)
}

#[test]
fn set_macro_impl_updates_engine_macros_synchronously() {
    let (engine, tmp) = spawn_with_baseline_on_disk();
    let h = engine.handle();
    assert!(h.config_read().macros.is_empty(), "baseline has no macros");

    let def = MacroDef {
        repeat: false,
        steps: vec![MacroStep {
            key: "x".to_string(),
            action: StepAction::Down,
            delay_ms: [10, 20],
        }],
    };
    set_macro_impl(&h, tmp.path(), "test_macro".to_string(), def).expect("set_macro ok");

    let live = h.config_read();
    assert!(
        live.macros.contains_key("test_macro"),
        "engine.config must reflect the new macro immediately"
    );
    assert_eq!(live.macros["test_macro"].steps.len(), 1);
    drop(live);
    engine.shutdown();
}

#[test]
fn delete_macro_impl_updates_engine_macros_synchronously() {
    let (engine, tmp) = spawn_with_baseline_on_disk();
    let h = engine.handle();
    let def = MacroDef {
        repeat: false,
        steps: vec![MacroStep {
            key: "x".to_string(),
            action: StepAction::Down,
            delay_ms: [10, 20],
        }],
    };
    set_macro_impl(&h, tmp.path(), "to_delete".to_string(), def).expect("seed");
    assert!(h.config_read().macros.contains_key("to_delete"));

    delete_macro_impl(&h, tmp.path(), "to_delete").expect("delete ok");

    assert!(
        !h.config_read().macros.contains_key("to_delete"),
        "engine.config must reflect the deletion immediately"
    );
    engine.shutdown();
}

#[test]
fn rename_macro_impl_updates_engine_and_bindings_synchronously() {
    let (engine, tmp) = spawn_with_baseline_on_disk();
    let h = engine.handle();
    let def = MacroDef {
        repeat: false,
        steps: vec![MacroStep {
            key: "x".to_string(),
            action: StepAction::Down,
            delay_ms: [10, 20],
        }],
    };
    set_macro_impl(&h, tmp.path(), "old".to_string(), def).expect("seed");
    set_binding_impl(
        &h,
        tmp.path(),
        dualsense_mapper::gamepad::ControllerKind::DualSense,
        dualsense_mapper::pad_bindings::Page::Base,
        0,
        ButtonEntry { label: "b0".to_string(), binding: Binding::Macro("old".to_string()) },
    )
    .expect("bind to old");

    rename_macro_impl(&h, tmp.path(), "old", "new").expect("rename");

    let live = h.config_read();
    assert!(!live.macros.contains_key("old"));
    assert!(live.macros.contains_key("new"));
    assert_eq!(live.pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons["0"].binding, Binding::Macro("new".to_string()));
    drop(live);
    engine.shutdown();
}

#[test]
fn set_binding_impl_updates_engine_buttons_synchronously() {
    let (engine, tmp) = spawn_with_baseline_on_disk();
    let h = engine.handle();
    set_binding_impl(
        &h,
        tmp.path(),
        dualsense_mapper::gamepad::ControllerKind::DualSense,
        dualsense_mapper::pad_bindings::Page::Base,
        0,
        ButtonEntry { label: "Cross".to_string(), binding: Binding::Key("x".to_string()) },
    )
    .expect("bind");

    assert_eq!(
        h.config_read().pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons["0"].binding,
        Binding::Key("x".to_string())
    );
    engine.shutdown();
}
