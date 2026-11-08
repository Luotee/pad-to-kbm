//! v3.8.0: every connected pad's battery reaches the board in
//! `controller-status` (`EngineEvent::ControllerConnected.battery`), and the
//! status is re-sent only when a reading changes. Two fake sources: the
//! first owns the session, the second waits.

#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::engine::{Engine, EngineEvent, Handle};
use dualsense_mapper::gamepad::{Battery, ControllerKind, GamepadEvent};
use std::collections::BTreeMap;
use std::time::Duration;

const DS: ControllerKind = ControllerKind::DualSense;
const NS: ControllerKind = ControllerKind::NsPro;

fn settle() {
    std::thread::sleep(Duration::from_millis(100));
}

fn batt(pct: u8, charging: bool) -> Battery {
    Battery { pct, charging }
}

/// The `battery` of every `ControllerConnected` since the last drain.
fn status_batteries(h: &Handle) -> Vec<BTreeMap<ControllerKind, Battery>> {
    h.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::ControllerConnected { battery, .. } => Some(battery),
            _ => None,
        })
        .collect()
}

/// DualSense owns at 80 %, the Switch Pro waits at 75 % charging.
fn both_connected(h: &Handle) {
    h.fake_event(GamepadEvent::Connected(DS));
    h.fake_event(GamepadEvent::Battery(Some(batt(80, false))));
    settle();
    h.fake_event_second(GamepadEvent::Connected(NS));
    h.fake_event_second(GamepadEvent::Battery(Some(batt(75, true))));
    settle();
}

#[test]
fn owner_and_waiting_pad_both_carry_their_battery() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    both_connected(&h);

    let want = BTreeMap::from([(DS, batt(80, false)), (NS, batt(75, true))]);
    let last = status_batteries(&h).pop().expect("a status event");
    assert_eq!(last, want);
    let status = h.current_status().expect("connected");
    assert_eq!(status.battery, want);
    assert_eq!(status.connected, vec![DS, NS]);
    assert_eq!(
        serde_json::to_value(&status.battery).unwrap(),
        serde_json::json!({
            "dualsense": { "pct": 80, "charging": false },
            "ns_pro": { "pct": 75, "charging": true }
        }),
        "payload shape: slug -> {{pct, charging}}"
    );
    engine.shutdown();
}

#[test]
fn status_is_resent_only_when_a_reading_changes() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(DS));
    h.fake_event(GamepadEvent::Battery(Some(batt(80, false))));
    settle();
    h.drain_events();

    for _ in 0..50 {
        h.fake_event(GamepadEvent::Battery(Some(batt(80, false))));
    }
    settle();
    assert_eq!(status_batteries(&h).len(), 0, "same reading: no resend");

    h.fake_event(GamepadEvent::Battery(Some(batt(70, false))));
    settle();
    assert_eq!(status_batteries(&h), vec![BTreeMap::from([(DS, batt(70, false))])], "pct changed");

    h.fake_event(GamepadEvent::Battery(Some(batt(70, true))));
    settle();
    assert_eq!(status_batteries(&h), vec![BTreeMap::from([(DS, batt(70, true))])], "charging changed");
    engine.shutdown();
}

#[test]
fn waiting_pad_leaving_drops_its_battery_in_the_next_status() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    both_connected(&h);
    h.drain_events();

    h.fake_event_second(GamepadEvent::Disconnected);
    settle();
    let next = h
        .drain_events()
        .into_iter()
        .find_map(|e| match e {
            EngineEvent::ControllerConnected { battery, connected, .. } => Some((battery, connected)),
            _ => None,
        })
        .expect("status re-sent when the waiting pad leaves");
    assert_eq!(next.1, vec![DS]);
    assert_eq!(next.0, BTreeMap::from([(DS, batt(80, false))]), "no ns_pro");
    engine.shutdown();
}

#[test]
fn owner_leaving_hands_over_with_the_waiting_pads_battery() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    both_connected(&h);
    h.drain_events();

    h.fake_event(GamepadEvent::Disconnected);
    settle();
    let status = h.current_status().expect("the Switch Pro took over");
    assert_eq!(status.kind, NS);
    assert_eq!(status.battery, BTreeMap::from([(NS, batt(75, true))]));
    assert_eq!(status_batteries(&h).last(), Some(&BTreeMap::from([(NS, batt(75, true))])));
    engine.shutdown();
}

#[test]
fn battery_alone_never_connects_a_pad() {
    // Iron rule 11: connection state comes from the report stream, never
    // from the battery.
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Battery(Some(batt(50, false))));
    settle();
    assert!(h.current_status().is_none());
    assert!(status_batteries(&h).is_empty());
    engine.shutdown();
}

#[test]
fn a_reading_that_disappears_is_resent_as_no_battery() {
    let engine = Engine::spawn_with_fake_gamepad(test_helpers::load_example()).expect("spawn");
    let h = engine.handle();
    h.fake_event(GamepadEvent::Connected(DS));
    h.fake_event(GamepadEvent::Battery(Some(batt(60, false))));
    settle();
    h.drain_events();

    h.fake_event(GamepadEvent::Battery(None));
    settle();
    assert_eq!(status_batteries(&h), vec![BTreeMap::new()], "the board drops the stale value");
    assert_eq!(h.current_status().expect("still connected").kind, DS);
    engine.shutdown();
}
