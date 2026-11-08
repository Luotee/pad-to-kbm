//! State-machine integration tests for `HidSource`. Uses the byte-stream
//! injection constructor so no real DualSense is required.

use crossbeam_channel::unbounded;
use dualsense_mapper::gamepad::cursor_params::CursorParams;
use dualsense_mapper::gamepad::events::GamepadEvent;
use dualsense_mapper::gamepad::ControllerKind;
use dualsense_mapper::gamepad::hid_source::HidSource;

fn neutral_report() -> Vec<u8> {
    let mut buf = vec![0u8; 78];
    buf[0] = 0x31;
    buf[2] = 128;
    buf[3] = 128;
    buf[4] = 128;
    buf[5] = 128;
    buf[9] = 0x08;  // hat = released
    buf[34] = 0x80; // finger 0 inactive (BT 0x31 offset, see ds_protocol)
    buf[38] = 0x80; // finger 1 inactive
    buf
}

fn finger_at(buf: &mut [u8], x: u16, y: u16) {
    buf[34] = 0x00; // active=1 (high bit clear), id=0
    buf[35] = (x & 0xFF) as u8;
    buf[36] = ((x >> 8) as u8 & 0x0F) | (((y & 0x0F) as u8) << 4);
    buf[37] = (y >> 4) as u8;
}

#[test]
fn first_streaming_frame_emits_connected() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    tx.send(neutral_report()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert_eq!(out.first(), Some(&GamepadEvent::Connected(ControllerKind::DualSense)),
        "expected Connected first, got {out:?}");
}

#[test]
fn cross_press_then_release_emits_button_events() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    let mut press = neutral_report();
    press[9] |= 1 << 5; // Cross
    tx.send(neutral_report()).unwrap();
    tx.send(press).unwrap();
    tx.send(neutral_report()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::ButtonDown(0))),
        "missing ButtonDown(0): {out:?}");
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::ButtonUp(0))),
        "missing ButtonUp(0): {out:?}");
}

#[test]
fn cursor_emits_mouse_delta_after_touch_down() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    // Frame 1: finger at (100, 100) — touch-down, no emit.
    // Frame 2: finger at (200, 150) — delta (+100, +50).
    //   mag = sqrt(100²+50²) ≈ 111.8 > fast_threshold(20)
    //   → gain = 1.50 (accel_gain_fast), total = sens(1.5) × gain(1.5) = 2.25
    //   → dx = (100 * 2.25) as i32 = 225, dy = (50 * 2.25) as i32 = 112.
    let mut a = neutral_report();
    finger_at(&mut a, 100, 100);
    let mut b = neutral_report();
    finger_at(&mut b, 200, 150);
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));

    let mut out = Vec::new();
    src.poll(&mut out);
    let delta = out.iter().find_map(|e| match e {
        GamepadEvent::MouseDelta { dx, dy } => Some((*dx, *dy)),
        _ => None,
    });
    let (dx, dy) = delta.expect(&format!("expected MouseDelta, got {out:?}"));
    assert_eq!(dx, 225);
    assert_eq!(dy, 112);
}

#[test]
fn cursor_jitter_floor_drops_one_pixel_motion() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    let mut a = neutral_report();
    finger_at(&mut a, 500, 500);
    let mut b = neutral_report();
    finger_at(&mut b, 501, 500);
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(!out.iter().any(|e| matches!(e, GamepadEvent::MouseDelta { .. })),
        "1px motion must be filtered: {out:?}");
}

#[test]
fn cursor_teleport_guard_suppresses_huge_jump() {
    // Reproduces v2.1.0 first-touch bug: DualSense reports one stale
    // frame on touch-down where `active` is true but x/y still carry
    // the previous touch's coordinates. The decoder sees a delta of
    // hundreds-to-thousands of raw px between frame N and frame N+1,
    // and without the guard would synthesise a screen-spanning jump.
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    // Frame 1: finger at far-corner stale coords (1800, 1000).
    // Frame 2: finger at fresh-touch coords (100, 100).
    //   Raw delta = (-1700, -900) → both well over CURSOR_TELEPORT_GUARD.
    // Frame 3: finger moves a believable amount, (130, 120).
    //   Delta from frame 2 = (+30, +20).
    //   mag = sqrt(30²+20²) ≈ 36.1 > fast_threshold(20)
    //   → gain = 1.50, total = sens(1.5) × gain(1.5) = 2.25
    //   → dx = (30 * 2.25) as i32 = 67, dy = (20 * 2.25) as i32 = 45.
    let mut a = neutral_report();
    finger_at(&mut a, 1800, 1000);
    let mut b = neutral_report();
    finger_at(&mut b, 100, 100);
    let mut c = neutral_report();
    finger_at(&mut c, 130, 120);
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    tx.send(c).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut out = Vec::new();
    src.poll(&mut out);
    let deltas: Vec<_> = out.iter().filter_map(|e| match e {
        GamepadEvent::MouseDelta { dx, dy } => Some((*dx, *dy)),
        _ => None,
    }).collect();
    assert_eq!(deltas.len(), 1,
        "expected exactly one MouseDelta after teleport guard, got {deltas:?}");
    let (dx, dy) = deltas[0];
    // With 3-layer filter: mag ≈ 36 > fast_threshold(20) → gain=1.5,
    // total = sens(1.5) * gain(1.5) = 2.25 → (67, 45).
    assert_eq!((dx, dy), ((30.0_f32 * 2.25) as i32, (20.0_f32 * 2.25) as i32));
}

#[test]
fn cursor_disabled_via_params_suppresses_mouse_delta() {
    let params = CursorParams::new(1.5, /*enabled=*/false);
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream_with_params(rx, params);

    let mut a = neutral_report();
    finger_at(&mut a, 100, 100);
    let mut b = neutral_report();
    finger_at(&mut b, 400, 400);
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(!out.iter().any(|e| matches!(e, GamepadEvent::MouseDelta { .. })),
        "cursor disabled must suppress all MouseDelta: {out:?}");
}

#[test]
fn touchpad_click_in_tl_emits_button_down_25() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    let mut a = neutral_report();
    finger_at(&mut a, 100, 100);   // TL quadrant
    let mut b = a.clone();
    b[11] |= 1 << 1;                // press touchpad
    let mut c = b.clone();
    c[11] &= !(1 << 1);             // release touchpad
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    tx.send(c).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::ButtonDown(25))),
        "missing ButtonDown(25) [Touchpad TL]: {out:?}");
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::ButtonUp(25))),
        "missing ButtonUp(25): {out:?}");
}

#[test]
fn touchpad_click_in_br_emits_button_down_28() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    let mut a = neutral_report();
    finger_at(&mut a, 1500, 800);  // BR quadrant
    let mut b = a.clone();
    b[11] |= 1 << 1;
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::ButtonDown(28))),
        "expected BR=28 down: {out:?}");
}

#[test]
fn touchpad_click_keeps_initial_quadrant_through_drag() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    // Press in TL, drag to BR, release.
    let mut a = neutral_report();
    finger_at(&mut a, 100, 100);     // TL
    let mut b = a.clone();
    b[11] |= 1 << 1;                  // click down
    let mut c = b.clone();
    finger_at(&mut c, 1500, 800);    // slide to BR while held
    let mut d = c.clone();
    d[11] &= !(1 << 1);              // release while finger in BR
    tx.send(a).unwrap();
    tx.send(b).unwrap();
    tx.send(c).unwrap();
    tx.send(d).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(120));

    let mut out = Vec::new();
    src.poll(&mut out);
    let downs: Vec<_> = out.iter().filter_map(|e| match e {
        GamepadEvent::ButtonDown(id) if (25..=28).contains(id) => Some(*id),
        _ => None,
    }).collect();
    let ups: Vec<_> = out.iter().filter_map(|e| match e {
        GamepadEvent::ButtonUp(id) if (25..=28).contains(id) => Some(*id),
        _ => None,
    }).collect();
    assert_eq!(downs, vec![25], "expected single TL down, got {downs:?}");
    assert_eq!(ups, vec![25], "expected single TL up (same id), got {ups:?}");
}

#[test]
fn channel_close_emits_disconnected() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream(rx);

    tx.send(neutral_report()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    drop(tx);
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(out.iter().any(|e| matches!(e, GamepadEvent::Disconnected)),
        "expected Disconnected after channel close: {out:?}");
}

// ── Switch Pro (0x30 standard input report) ─────────────────────────────

/// 49-byte 0x30 frame with both sticks at the default centre (2048).
fn ns_report(b3: u8, b4: u8, b5: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 49];
    buf[0] = 0x30;
    buf[3] = b3;
    buf[4] = b4;
    buf[5] = b5;
    buf[6..9].copy_from_slice(&[0x00, 0x08, 0x80]);
    buf[9..12].copy_from_slice(&[0x00, 0x08, 0x80]);
    buf
}

#[test]
fn ns_byte_stream_connected_and_capture() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream_for(ControllerKind::NsPro, rx);

    tx.send(ns_report(0, 0x20, 0)).unwrap(); // Capture held
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert_eq!(out.first(), Some(&GamepadEvent::Connected(ControllerKind::NsPro)),
        "expected Connected(NsPro) first, got {out:?}");
    let downs: Vec<u32> = out.iter().filter_map(|e| match e {
        GamepadEvent::ButtonDown(id) => Some(*id),
        _ => None,
    }).collect();
    assert_eq!(downs, vec![29], "Capture → id 29 only: {out:?}");
    assert!(!out.iter().any(|e| matches!(e,
        GamepadEvent::TouchpadHover { .. } | GamepadEvent::TouchpadClick { .. }
        | GamepadEvent::MouseDelta { .. })),
        "no touchpad processing for NsPro: {out:?}");
}

#[test]
fn ns_byte_stream_zl_emits_button_and_trigger() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream_for(ControllerKind::NsPro, rx);

    tx.send(ns_report(0, 0, 0)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut out = Vec::new();
    src.poll(&mut out);
    assert!(out.contains(&GamepadEvent::Trigger { axis: 4, value: 0.0 }), "{out:?}");

    tx.send(ns_report(0, 0, 0x80)).unwrap(); // ZL down
    std::thread::sleep(std::time::Duration::from_millis(50));
    out.clear();
    src.poll(&mut out);
    assert!(out.contains(&GamepadEvent::ButtonDown(23)), "missing ButtonDown(23): {out:?}");
    assert!(out.contains(&GamepadEvent::Trigger { axis: 4, value: 1.0 }), "{out:?}");
    assert!(!out.iter().any(|e| matches!(e, GamepadEvent::Trigger { axis: 5, .. })), "{out:?}");

    tx.send(ns_report(0, 0, 0)).unwrap(); // ZL up
    std::thread::sleep(std::time::Duration::from_millis(50));
    out.clear();
    src.poll(&mut out);
    assert!(out.contains(&GamepadEvent::ButtonUp(23)), "missing ButtonUp(23): {out:?}");
    assert!(out.contains(&GamepadEvent::Trigger { axis: 4, value: 0.0 }), "{out:?}");
}

#[test]
fn byte_stream_ignores_other_kinds_reports() {
    // A DualSense 0x31 frame on the NsPro worker, and a 0x30 frame on the
    // DualSense worker, decode to nothing (no Connected, no buttons).
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut ns = HidSource::new_from_byte_stream_for(ControllerKind::NsPro, rx);
    tx.send(neutral_report()).unwrap();
    let (tx2, rx2) = unbounded::<Vec<u8>>();
    let mut ds = HidSource::new_from_byte_stream(rx2);
    tx2.send(ns_report(0xFF, 0xFF, 0xFF)).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut out = Vec::new();
    ns.poll(&mut out);
    ds.poll(&mut out);
    assert!(out.is_empty(), "foreign reports must not decode: {out:?}");
}

// ── Xbox Nano (Bluetooth LE report 0x01) ────────────────────────────────

/// The recorded A press (`tests/fixtures/xbox_nano/a.hex`).
fn nano_fixture(name: &str) -> Vec<Vec<u8>> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/xbox_nano")
        .join(format!("{name}.hex"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect())
        .collect()
}

#[test]
fn nano_byte_stream_connected_and_a() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut src = HidSource::new_from_byte_stream_for(ControllerKind::XboxNano, rx);
    for r in nano_fixture("a") {
        tx.send(r).unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(100));

    let mut out = Vec::new();
    src.poll(&mut out);
    assert_eq!(out.first(), Some(&GamepadEvent::Connected(ControllerKind::XboxNano)),
        "expected Connected(XboxNano) first, got {out:?}");
    let buttons: Vec<&GamepadEvent> = out.iter()
        .filter(|e| matches!(e, GamepadEvent::ButtonDown(_) | GamepadEvent::ButtonUp(_)))
        .collect();
    assert_eq!(buttons, vec![&GamepadEvent::ButtonDown(0), &GamepadEvent::ButtonUp(0)], "{out:?}");
    assert!(!out.iter().any(|e| matches!(e,
        GamepadEvent::TouchpadHover { .. } | GamepadEvent::TouchpadClick { .. }
        | GamepadEvent::MouseDelta { .. })),
        "no touchpad processing for the Nano: {out:?}");
}

#[test]
fn nano_and_dualsense_workers_ignore_each_others_reports() {
    let (tx, rx) = unbounded::<Vec<u8>>();
    let mut nano = HidSource::new_from_byte_stream_for(ControllerKind::XboxNano, rx);
    tx.send(neutral_report()).unwrap();
    let (tx2, rx2) = unbounded::<Vec<u8>>();
    let mut ds = HidSource::new_from_byte_stream(rx2);
    tx2.send(nano_fixture("a")[1].clone()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut out = Vec::new();
    nano.poll(&mut out);
    ds.poll(&mut out);
    assert!(out.is_empty(), "foreign reports must not decode: {out:?}");
}
