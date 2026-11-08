//! Xbox Nano Bluetooth LE report decoding against the reports recorded on
//! real hardware (`tests/fixtures/xbox_nano/*.hex`, one report per line,
//! written by `hid_probe`). Each fixture is one control pressed and
//! released (or a stick turned a full circle).

use dualsense_mapper::gamepad::xbox_protocol::decode_xbox_bt;
use dualsense_mapper::gamepad::PadState;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<Vec<u8>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/xbox_nano")
        .join(format!("{name}.hex"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            l.split_whitespace()
                .map(|b| u8::from_str_radix(b, 16).unwrap_or_else(|e| panic!("{name}: {b}: {e}")))
                .collect()
        })
        .collect()
}

fn decode_all(name: &str) -> Vec<PadState> {
    let reports = fixture(name);
    assert!(!reports.is_empty(), "{name}: empty fixture");
    reports
        .iter()
        .enumerate()
        .map(|(i, r)| decode_xbox_bt(r).unwrap_or_else(|| panic!("{name} line {}: no decode", i + 1)))
        .collect()
}

/// Every button id seen pressed anywhere in the fixture.
fn pressed_ids(states: &[PadState]) -> Vec<usize> {
    (0..states[0].buttons.len())
        .filter(|&i| states.iter().any(|s| s.buttons[i]))
        .collect()
}

#[test]
fn each_button_fixture_presses_only_its_button() {
    let cases: [(&str, &[usize]); 15] = [
        ("a", &[0]),
        ("b", &[1]),
        ("x", &[2]),
        ("y", &[3]),
        ("view", &[4]),
        ("xbox", &[5]),
        ("menu", &[6]),
        ("l3", &[7]),
        ("r3", &[8]),
        ("lb", &[9]),
        ("rb", &[10]),
        ("up", &[11]),
        ("down", &[12]),
        ("left", &[13]),
        ("right", &[14]),
    ];
    for (name, want) in cases {
        let states = decode_all(name);
        assert_eq!(pressed_ids(&states), want, "{name}");
        assert!(!states.last().unwrap().buttons.iter().any(|&b| b), "{name}: released at the end");
    }
}

#[test]
fn share_is_not_in_the_gamepad_report() {
    // Share is not in the gamepad report (nor in XInput): the board greys it out.
    let states = decode_all("share");
    assert!(pressed_ids(&states).is_empty(), "share.hex: {:?}", pressed_ids(&states));
}

#[test]
fn stick_and_trigger_fixtures_press_no_button() {
    for name in ["ls", "rs", "lt", "rt"] {
        let states = decode_all(name);
        assert!(pressed_ids(&states).is_empty(), "{name}: {:?}", pressed_ids(&states));
    }
}

#[test]
fn triggers_reach_full_scale_and_release() {
    for (name, left) in [("lt", true), ("rt", false)] {
        let states = decode_all(name);
        let pull = |s: &PadState| if left { s.trigger_l2 } else { s.trigger_r2 };
        let other = |s: &PadState| if left { s.trigger_r2 } else { s.trigger_l2 };
        let max = states.iter().map(pull).fold(f32::MIN, f32::max);
        assert!(max >= 0.99, "{name}: max {max}");
        assert!(pull(states.last().unwrap()) <= 0.01, "{name}: released at the end");
        assert!(states.iter().all(|s| other(s) <= 0.01), "{name}: other trigger moved");
        assert!(states.iter().all(|s| (0.0..=1.0).contains(&pull(s))), "{name}: out of range");
    }
}

#[test]
fn a_full_stick_circle_reaches_both_ends_of_both_axes() {
    for (name, left) in [("ls", true), ("rs", false)] {
        let states = decode_all(name);
        let (xs, ys): (Vec<f32>, Vec<f32>) = states
            .iter()
            .map(|s| if left { (s.stick_lx, s.stick_ly) } else { (s.stick_rx, s.stick_ry) })
            .unzip();
        for (axis, v) in [("x", &xs), ("y", &ys)] {
            let lo = v.iter().cloned().fold(f32::MAX, f32::min);
            let hi = v.iter().cloned().fold(f32::MIN, f32::max);
            assert!(lo < -0.9 && hi > 0.9, "{name} {axis}: {lo}..{hi}");
            assert!(lo >= -1.0 && hi <= 1.0, "{name} {axis}: {lo}..{hi} out of range");
        }
    }
}

#[test]
fn sticks_rest_near_centre() {
    // The A fixture leaves both sticks alone: within the recorded noise.
    for s in decode_all("a") {
        for v in [s.stick_lx, s.stick_ly, s.stick_rx, s.stick_ry] {
            assert!(v.abs() < 0.05, "{v}");
        }
    }
}

/// A neutral 16-byte report with `f` applied.
fn report(f: impl FnOnce(&mut [u8; 16])) -> [u8; 16] {
    let mut r = [0u8; 16];
    r[0] = 0x01;
    for i in [1, 3, 5, 7] {
        r[i] = 0x00;
        r[i + 1] = 0x80;
    }
    f(&mut r);
    r
}

#[test]
fn stick_up_is_positive_y_like_the_dualsense() {
    // HID convention: Y grows downwards, so raw 0 is full up. The decoder
    // flips it so up > 0 (`decode_31` convention, the mapper's 15 / 19).
    let up = decode_xbox_bt(&report(|r| { r[3] = 0; r[4] = 0; r[7] = 0; r[8] = 0; })).unwrap();
    assert!(up.stick_ly > 0.99 && up.stick_ry > 0.99, "{up:?}");
    let down = decode_xbox_bt(&report(|r| { r[3] = 0xFF; r[4] = 0xFF; r[7] = 0xFF; r[8] = 0xFF; })).unwrap();
    assert!(down.stick_ly < -0.99 && down.stick_ry < -0.99, "{down:?}");
    let left = decode_xbox_bt(&report(|r| { r[1] = 0; r[2] = 0; r[5] = 0; r[6] = 0; })).unwrap();
    assert!(left.stick_lx < -0.99 && left.stick_rx < -0.99, "{left:?}");
    let centre = decode_xbox_bt(&report(|_| {})).unwrap();
    assert_eq!((centre.stick_lx, centre.stick_ly, centre.stick_rx, centre.stick_ry), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn hat_diagonals_press_two_directions() {
    // 11 up, 12 down, 13 left, 14 right.
    let cases: [(u8, &[usize]); 10] = [
        (0, &[]),
        (1, &[11]),
        (2, &[11, 14]),
        (3, &[14]),
        (4, &[12, 14]),
        (5, &[12]),
        (6, &[12, 13]),
        (7, &[13]),
        (8, &[11, 13]),
        (15, &[]),
    ];
    for (hat, want) in cases {
        let s = decode_xbox_bt(&report(|r| r[13] = hat)).unwrap();
        assert_eq!(pressed_ids(&[s]), want, "hat {hat}");
    }
}

#[test]
fn unused_bits_press_nothing() {
    let s = decode_xbox_bt(&report(|r| { r[14] = 0x24; r[15] = 0x83; })).unwrap();
    assert!(pressed_ids(&[s]).is_empty());
}

#[test]
fn wrong_report_id_or_short_buffer_is_none() {
    let a = fixture("a")[1].clone();
    let mut wrong = a.clone();
    wrong[0] = 0x02;
    assert!(decode_xbox_bt(&wrong).is_none());
    assert!(decode_xbox_bt(&a[..15]).is_none());
    assert!(decode_xbox_bt(&[]).is_none());
    // A DualSense 0x31 report is not a Nano report.
    let mut ds = vec![0u8; 78];
    ds[0] = 0x31;
    assert!(decode_xbox_bt(&ds).is_none());
}
