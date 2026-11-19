//! v3.10.0: which connected pads the app can turn off reaches the board in
//! `controller-status` (`power_off`): the classic pads always, the Xbox
//! Nano only once its companion channel is ready (`GamepadEvent::PowerOff`).

#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::engine::{Engine, EngineEvent, Handle};
use dualsense_mapper::gamepad::{ControllerKind, GamepadEvent};
use std::time::Duration;

const DS: ControllerKind = ControllerKind::DualSense;
const XN: ControllerKind = ControllerKind::XboxNano;

fn settle() {
    std::thread::sleep(Duration::from_millis(100));
}

fn last_power_off(h: &Handle) -> Option<Vec<ControllerKind>> {
    h.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ControllerConnected { power_off, .. } => Some(power_off),
            _ => None,
        })
        .last()
}

#[test]
fn ready_nano_joins_the_power_off_list() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(DS));
    settle();
    h.fake_event_second(GamepadEvent::Connected(XN));
    settle();
    assert_eq!(last_power_off(&h), Some(vec![DS]));
    assert_eq!(h.current_status().unwrap().power_off, vec![DS]);
    h.fake_event_second(GamepadEvent::PowerOff(true));
    settle();
    assert_eq!(last_power_off(&h), Some(vec![DS, XN]), "re-published on the change");
    let status = h.current_status().unwrap();
    assert_eq!(status.power_off, vec![DS, XN]);
    assert_eq!(serde_json::to_value(&status.power_off).unwrap(), serde_json::json!(["dualsense", "xbox_nano"]));
    h.fake_event_second(GamepadEvent::PowerOff(false));
    settle();
    assert_eq!(last_power_off(&h), Some(vec![DS]));
    engine.shutdown();
}
