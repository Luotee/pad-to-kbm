#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::engine::{Engine, EngineEvent};
use dualsense_mapper::config::Config;
use std::time::Duration;

fn load_minimal_config() -> Config {
    let json = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples/maple_artale.json"),
    )
    .expect("example config must exist");
    Config::load_from_str(&json).expect("example config must parse")
}

#[test]
fn pause_drains_held_and_skips_subsequent_synth() {
    let cfg = load_minimal_config();
    let engine = Engine::spawn_with_fake_gamepad(cfg).expect("spawn");
    let handle = engine.handle();

    // Button 0 is bound to "x" in maple_artale.json.
    handle.fake_button_down(0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 1, "press registered");

    handle.set_paused(true);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 0, "pause drained held keys");

    handle.fake_button_down(0); // event arrives while paused
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 0, "paused engine ignores events");

    handle.set_paused(false);
    handle.fake_button_down(0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 1, "unpause restores synth");

    engine.shutdown();
}

#[test]
fn capture_active_gates_synth_only_not_event_emission() {
    let cfg = load_minimal_config();
    let engine = Engine::spawn_with_fake_gamepad(cfg).expect("spawn");
    let handle = engine.handle();

    handle.set_capture_active(true);
    handle.fake_button_down(0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 0, "capture-active blocks synth");
    let events: Vec<_> = handle.drain_events();
    assert!(
        events.iter().any(|e| matches!(e, EngineEvent::ButtonDown { id: 0 })),
        "events still emit while capture-active; got: {events:?}"
    );

    engine.shutdown();
}

#[test]
fn hot_rebind_picks_up_new_binding_on_next_event() {
    let cfg = load_minimal_config();
    let engine = Engine::spawn_with_fake_gamepad(cfg).expect("spawn");
    let handle = engine.handle();

    // Swap binding for id 0 from "x" to Unbound.
    {
        let mut guard = handle.config_write();
        guard.pad_mut(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons.get_mut("0").unwrap().binding = dualsense_mapper::config::Binding::Unbound;
    }

    // Give the engine loop a full tick (8 ms) to notice the config change.
    std::thread::sleep(Duration::from_millis(50));

    handle.fake_button_down(0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(handle.held_keys_count(), 0, "Unbound after rebind");

    engine.shutdown();
}

#[test]
fn connected_status_carries_kind() {
    use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
    let cfg = load_minimal_config();
    let engine = Engine::spawn_with_fake_gamepad(cfg).expect("spawn");
    let handle = engine.handle();

    handle.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    std::thread::sleep(Duration::from_millis(100));

    let status = handle.current_status().expect("status after Connected");
    assert_eq!(status.kind, ControllerKind::DualSense);
    assert_eq!(status.name, "DualSense");

    engine.shutdown();
}

#[test]
fn status_lists_connected_pads() {
    use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
    let engine = Engine::spawn_with_fake_gamepad(load_minimal_config()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(ControllerKind::DualSense));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.current_status().unwrap().connected, vec![ControllerKind::DualSense]);
    let connected_events: Vec<_> = h
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ControllerConnected { connected, .. } => Some(connected),
            _ => None,
        })
        .collect();
    assert_eq!(connected_events, vec![vec![ControllerKind::DualSense]], "one event, carrying the list");
    engine.shutdown();
}

#[test]
fn status_of_a_pad_without_battery_has_an_empty_battery_map() {
    // A pad that reports no battery (Switch 2 Pro, Xbox Nano) is left out:
    // the board keeps its green live dot.
    use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
    let engine = Engine::spawn_with_fake_gamepad(load_minimal_config()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(ControllerKind::XboxNano));
    std::thread::sleep(Duration::from_millis(100));
    assert!(h.current_status().unwrap().battery.is_empty());
    let batteries: Vec<_> = h
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ControllerConnected { battery, .. } => Some(battery),
            _ => None,
        })
        .collect();
    assert_eq!(batteries.len(), 1);
    assert!(batteries[0].is_empty());
    engine.shutdown();
}

#[test]
fn disconnect_aimed_at_the_owner_reaches_it() {
    use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
    let engine = Engine::spawn_with_fake_gamepad(load_minimal_config()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(ControllerKind::NsPro));
    std::thread::sleep(Duration::from_millis(100));
    h.disconnect(Some(ControllerKind::DualSense));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(h.fake_disconnects_received(), 0, "not connected: no-op");
    h.disconnect(Some(ControllerKind::NsPro));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(h.fake_disconnects_received(), 1);
    h.disconnect(None);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(h.fake_disconnects_received(), 1, "None = the owner");
    engine.shutdown();
}


#[test]
fn key_emit_carries_button_source() {
    use dualsense_mapper::mapper::Source;
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    h.fake_event(dualsense_mapper::gamepad::GamepadEvent::Connected(dualsense_mapper::gamepad::ControllerKind::DualSense));
    std::thread::sleep(std::time::Duration::from_millis(100));
    h.drain_events();
    h.fake_button_down(0);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let evs = h.drain_events();
    assert!(
        evs.iter().any(|e| matches!(e, EngineEvent::KeyEmit { source: Source::Button(0), action: "down", .. })),
        "{evs:?}"
    );
    engine.shutdown();
}
