//! v3.10.0: the Xbox Nano's companion protocol (vendor GATT service
//! 00000000-16e2-…, characteristic 00000004-16e2-…). Pure codec, send
//! allow-list and session state machine, checked against the bytes captured
//! from the maintainer's Nano on 2026-11-19 (driverless).

use dualsense_mapper::gamepad::nano_companion_proto::*;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// The app-hello (0x2E) reply payload from run3 (78 bytes).
const HELLO6_REPLY: &str = "5e 04 4c 0b 06 0c aa 03 e3 29 8e 0d 4a 7e 43 35 e4 b3 17 19 a6 d2 30 44 33 34 58 51 54 32 \
    36 33 32 42 42 50 01 06 00 00 00 ff 03 00 00 03 00 00 00 01 00 06 00 13 00 32 00 00 00 00 00 00 00 00 00 00 00 00 \
    00 00 00 00 00 00 00 00 01 5a";

const ID: u32 = 0x4965_80d1;

fn linked() -> Session {
    let mut s = Session::new(ID, 239);
    assert_eq!(s.on_frame(&hex("ff04d18065490200160210")), Inbound::Linked { ch: 4, ver: (2, 0, 22) });
    s
}

#[test]
fn link_hello_bytes() {
    assert_eq!(link_hello(ID), [0xff, 0x00, 0xd1, 0x80, 0x65, 0x49, 0x02, 0x00, 0x16, 0x02, 0x10]);
}

#[test]
fn parse_link_hello_echo() {
    assert_eq!(
        parse(&hex("ff04d18065490200160210")),
        Frame::LinkHello { chbase: 4, id: ID, ver: (2, 0, 22), max_len: 0x1002 }
    );
}

#[test]
fn parse_nop_and_ack() {
    assert_eq!(parse(&[6]), Frame::Nop);
    assert_eq!(parse(&hex("02042eea")), Frame::Ack { ch: 4, ty: 0x2e, next: 0xea });
    assert_eq!(
        parse(&hex("050402f2020300")),
        Frame::Data { kind: 5, ch: 4, ty: 2, seq: 0xf2, payload: vec![3, 0] }
    );
    assert_eq!(parse(&[0x77, 1]), Frame::Unknown);
}

#[test]
fn ack_is_seq_plus_one() {
    assert_eq!(ack(4, 0x02, 0xf2), [2, 4, 2, 0xf3]);
    assert_eq!(ack(4, 2, 0xff), [2, 4, 2, 0x00]);
}

#[test]
fn battery_request_frame() {
    assert_eq!(data_frames(4, T_BATT, 0xed, &[], 239).unwrap(), vec![hex("050402ed00")]);
}

#[test]
fn power_off_frame() {
    assert_eq!(data_frames(4, T_RST, 0xf3, &[0], 239).unwrap(), vec![hex("050424f30100")]);
}

#[test]
fn fragmenting() {
    let p: Vec<u8> = (0..30).collect();
    let f = data_frames(4, T_HELLO6, 0x10, &p, 12).unwrap();
    assert_eq!(f.len(), 3);
    assert_eq!(f.iter().map(|x| x[0]).collect::<Vec<_>>(), vec![3, 1, 4]);
    assert_eq!(f.iter().map(|x| x[3]).collect::<Vec<_>>(), vec![0x10, 0x11, 0x12]);
    assert_eq!(f.iter().map(|x| x[4]).collect::<Vec<_>>(), vec![12, 12, 6]);
    assert_eq!(f.iter().flat_map(|x| x[5..].to_vec()).collect::<Vec<_>>(), p);
}

#[test]
fn allow_list_refuses() {
    for ty in [0x03u8, 0x25, 0x0B, 0x0D, 0x21, 0x07, 0x1E, 0x19, 0x1A, 0x01] {
        assert_eq!(data_frames(2, ty, 0, &[], 239), Err(Refused { ty }), "type 0x{ty:02x}");
    }
    assert_eq!(data_frames(2, T_RST, 0, &[1], 239), Err(Refused { ty: T_RST }), "reboot");
    assert_eq!(data_frames(2, T_RST, 0, &[], 239), Err(Refused { ty: T_RST }), "rst needs exactly 00");
}

#[test]
fn hello_payload_and_type() {
    let p = hello_payload(0x0000_0000_6a00_0001);
    assert_eq!(p, vec![0x01, 0x00, 0x00, 0x6a, 0, 0, 0, 0, 6, 0, 0, 0]);
    assert_eq!(hello_type((2, 0, 22)), T_HELLO6);
    assert_eq!(hello_type((2, 0, 21)), T_HELLO5);
    assert_eq!(hello_type((3, 0, 0)), T_HELLO6);
}

#[test]
fn hello6_reply_caps() {
    let c = caps_from_hello(&hex(HELLO6_REPLY)).unwrap();
    assert_eq!(c, Caps(0x3ff));
    assert!(c.battery() && c.shutdown());
    assert!(!Caps(0x3fb).shutdown());
    assert_eq!(caps_from_hello(&[0; 0x2c]), None, "too short");
}

#[test]
fn charging_bit() {
    assert_eq!(charging_from_batt(&[0x01, 0x5a], (2, 0, 22)), Some(false));
    assert_eq!(charging_from_batt(&[0x03, 0x5a], (2, 0, 22)), Some(true));
    assert_eq!(charging_from_batt(&[0x03], (2, 0, 22)), None);
    assert_eq!(charging_from_batt(&[1, 50, 1], (2, 0, 21)), Some(true));
    assert_eq!(charging_from_batt(&[1, 50, 0], (2, 0, 21)), Some(false));
    assert_eq!(charging_from_batt(&[1, 50], (2, 0, 21)), None);
}

#[test]
fn session_flow() {
    let mut s = Session::new(ID, 239);
    assert_eq!(s.channel(), None);
    assert!(s.request(T_BATT, &[]).is_err(), "not linked yet");
    let mut s2 = linked();
    assert_eq!(s2.channel(), Some(4));
    assert_eq!(s2.ver(), (2, 0, 22));
    assert_eq!(
        s2.on_frame(&hex("050402f2020300")),
        Inbound::Message { ch: 4, ty: 2, payload: vec![3, 0], ack: [2, 4, 2, 0xf3] }
    );
    let a = s2.request(T_BATT, &[]).unwrap();
    let b = s2.request(T_BATT, &[]).unwrap();
    assert_eq!(b[0][3], a[0][3].wrapping_add(4));
    assert_eq!(a[0][..3], [5, 4, 2]);
    assert!(s2.request(0x03, &[]).is_err(), "allow-list applies to requests");
    let _ = &mut s;
}

#[test]
fn other_channels_are_ignored() {
    let mut s = linked();
    // XboxAccessoriesService's own session on channels 2/3 (run2).
    assert_eq!(s.on_frame(&hex("0502196d0446000000")), Inbound::Nothing);
    assert_eq!(s.on_frame(&hex("05031d4200")), Inbound::Nothing);
    assert_eq!(s.on_frame(&hex("02021972")), Inbound::Nothing);
    assert_eq!(s.on_frame(&[6]), Inbound::Nothing);
    // An echo of someone else's link hello.
    assert_eq!(s.on_frame(&hex("ff027ac1761c0200160210")), Inbound::Nothing);
    let mut fresh = Session::new(ID, 239);
    assert_eq!(fresh.on_frame(&hex("ff027ac1761c0200160210")), Inbound::Nothing, "not our id");
    assert_eq!(fresh.on_frame(&hex("050402f2020300")), Inbound::Nothing, "data before the link");
}

#[test]
fn reassembly() {
    let mut s = linked();
    assert_eq!(s.on_frame(&hex("03051b1002aabb")), Inbound::Fragment { ack: [2, 5, 0x1b, 0x11] });
    assert_eq!(
        s.on_frame(&hex("04051b1101cc")),
        Inbound::Message { ch: 5, ty: 0x1b, payload: vec![0xaa, 0xbb, 0xcc], ack: [2, 5, 0x1b, 0x12] }
    );
}
