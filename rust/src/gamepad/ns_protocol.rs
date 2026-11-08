//! Pure Switch Pro Controller protocol functions (no I/O, no config).
//!
//! Output report 0x01 subcommand framing, 0x21 reply parsing, SPI stick
//! calibration parsing and the 0x30 standard input report decoder.
//! The calibration parsing logic is adapted from SDL's
//! `SDL_hidapi_switch.c` (zlib license).

use super::pad::PadState;

pub const NS_OUTPUT_LEN: usize = 49;

pub(crate) const NEUTRAL_RUMBLE: [u8; 8] = [0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40];
pub(super) const DEFAULT_CENTER: u16 = 2048;
pub(super) const DEFAULT_SPAN: u16 = 1400;
pub(super) const UNINIT: u16 = 0xFFF;

/// Build an output report 0x01 carrying one subcommand.
pub fn build_subcmd(counter: u8, id: u8, args: &[u8]) -> [u8; NS_OUTPUT_LEN] {
    let mut out = [0u8; NS_OUTPUT_LEN];
    out[0] = 0x01;
    out[1] = counter & 0x0F;
    out[2..10].copy_from_slice(&NEUTRAL_RUMBLE);
    out[10] = id;
    let n = args.len().min(NS_OUTPUT_LEN - 11);
    out[11..11 + n].copy_from_slice(&args[..n]);
    out
}

pub struct SubcmdReply<'a> {
    pub ack: bool,
    pub id: u8,
    pub data: &'a [u8],
}

/// Parse an input report 0x21 (subcommand reply).
pub fn parse_21(buf: &[u8]) -> Option<SubcmdReply<'_>> {
    if buf.len() < 15 || buf[0] != 0x21 {
        return None;
    }
    Some(SubcmdReply {
        ack: buf[13] & 0x80 != 0,
        id: buf[14],
        data: &buf[15..],
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisCal {
    pub center: u16,
    pub span_neg: u16,
    pub span_pos: u16,
}

impl Default for AxisCal {
    fn default() -> Self {
        AxisCal { center: DEFAULT_CENTER, span_neg: DEFAULT_SPAN, span_pos: DEFAULT_SPAN }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickCal {
    pub lx: AxisCal,
    pub ly: AxisCal,
    pub rx: AxisCal,
    pub ry: AxisCal,
}

impl Default for StickCal {
    fn default() -> Self {
        StickCal {
            lx: AxisCal::default(),
            ly: AxisCal::default(),
            rx: AxisCal::default(),
            ry: AxisCal::default(),
        }
    }
}

/// SPI flash read subcommand (0x10) arguments: address u32 LE + length.
pub fn spi_read_args(addr: u32, len: u8) -> [u8; 5] {
    let a = addr.to_le_bytes();
    [a[0], a[1], a[2], a[3], len]
}

/// Unpack two 12-bit values from 3 bytes (x low, y high).
pub(super) fn unpack12(b: &[u8]) -> (u16, u16) {
    let x = (b[0] as u16) | (((b[1] & 0x0F) as u16) << 8);
    let y = ((b[1] >> 4) as u16) | ((b[2] as u16) << 4);
    (x, y)
}

fn axis(center: u16, neg: u16, pos: u16) -> AxisCal {
    let span = |v: u16| if v == UNINIT || v == 0 { DEFAULT_SPAN } else { v };
    AxisCal {
        center: if center == UNINIT { DEFAULT_CENTER } else { center },
        span_neg: span(neg),
        span_pos: span(pos),
    }
}

/// Parse a 9-byte left-stick block: max-above, center, min-below.
/// (Adapted from SDL `LoadStickCalibration`, zlib.)
fn parse_left(b: &[u8]) -> (AxisCal, AxisCal) {
    let (xmax, ymax) = unpack12(&b[0..3]);
    let (xc, yc) = unpack12(&b[3..6]);
    let (xmin, ymin) = unpack12(&b[6..9]);
    (axis(xc, xmin, xmax), axis(yc, ymin, ymax))
}

/// Parse a 9-byte right-stick block: center, min-below, max-above.
fn parse_right(b: &[u8]) -> (AxisCal, AxisCal) {
    let (xc, yc) = unpack12(&b[0..3]);
    let (xmin, ymin) = unpack12(&b[3..6]);
    let (xmax, ymax) = unpack12(&b[6..9]);
    (axis(xc, xmin, xmax), axis(yc, ymin, ymax))
}

fn from_blocks(left: &[u8], right: &[u8]) -> StickCal {
    let (lx, ly) = parse_left(left);
    let (rx, ry) = parse_right(right);
    StickCal { lx, ly, rx, ry }
}

/// User calibration: 0x16 bytes from 0x8010. `None` unless both sticks
/// carry the `B2 A1` magic.
pub fn parse_user_cal(spi: &[u8]) -> Option<StickCal> {
    if spi.len() < 0x16 || spi[0..2] != [0xB2, 0xA1] || spi[11..13] != [0xB2, 0xA1] {
        return None;
    }
    Some(from_blocks(&spi[2..11], &spi[13..22]))
}

/// Factory calibration: 0x12 bytes from 0x603D (left 9 + right 9).
/// Uninitialised (0xFFF) fields fall back to defaults.
pub fn parse_factory_cal(spi: &[u8]) -> StickCal {
    if spi.len() < 0x12 {
        return StickCal::default();
    }
    from_blocks(&spi[0..9], &spi[9..18])
}

pub(super) fn norm(raw: u16, c: &AxisCal) -> f32 {
    let d = raw as f32 - c.center as f32;
    let v = if d >= 0.0 { d / c.span_pos.max(1) as f32 } else { d / c.span_neg.max(1) as f32 };
    v.clamp(-1.0, 1.0)
}

/// Decode a standard input report 0x30.
pub fn decode_30(buf: &[u8], cal: &StickCal) -> Option<PadState> {
    if buf.len() < 12 || buf[0] != 0x30 {
        return None;
    }
    let mut s = PadState::default();
    let (b3, b4, b5) = (buf[3], buf[4], buf[5]);

    s.buttons[2] = b3 & 0x01 != 0; // Y
    s.buttons[3] = b3 & 0x02 != 0; // X
    s.buttons[0] = b3 & 0x04 != 0; // B
    s.buttons[1] = b3 & 0x08 != 0; // A
    s.buttons[10] = b3 & 0x40 != 0; // R
    s.buttons[24] = b3 & 0x80 != 0; // ZR

    s.buttons[4] = b4 & 0x01 != 0; // Minus
    s.buttons[6] = b4 & 0x02 != 0; // Plus
    s.buttons[8] = b4 & 0x04 != 0; // RS
    s.buttons[7] = b4 & 0x08 != 0; // LS
    s.buttons[5] = b4 & 0x10 != 0; // Home
    s.buttons[29] = b4 & 0x20 != 0; // Capture

    s.buttons[12] = b5 & 0x01 != 0; // Down
    s.buttons[11] = b5 & 0x02 != 0; // Up
    s.buttons[14] = b5 & 0x04 != 0; // Right
    s.buttons[13] = b5 & 0x08 != 0; // Left
    s.buttons[9] = b5 & 0x40 != 0; // L
    s.buttons[23] = b5 & 0x80 != 0; // ZL

    s.trigger_l2 = if s.buttons[23] { 1.0 } else { 0.0 };
    s.trigger_r2 = if s.buttons[24] { 1.0 } else { 0.0 };

    let (lx, ly) = unpack12(&buf[6..9]);
    let (rx, ry) = unpack12(&buf[9..12]);
    s.stick_lx = norm(lx, &cal.lx);
    s.stick_ly = norm(ly, &cal.ly);
    s.stick_rx = norm(rx, &cal.rx);
    s.stick_ry = norm(ry, &cal.ry);
    s.battery = Some(super::battery::from_switch_pro(buf[2]));
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(x: u16, y: u16) -> [u8; 3] {
        [(x & 0xFF) as u8, ((x >> 8) as u8 & 0x0F) | ((y & 0x0F) as u8) << 4, (y >> 4) as u8]
    }

    fn report(b3: u8, b4: u8, b5: u8, l: (u16, u16), r: (u16, u16)) -> [u8; 20] {
        let mut b = [0u8; 20];
        b[0] = 0x30;
        b[3] = b3;
        b[4] = b4;
        b[5] = b5;
        b[6..9].copy_from_slice(&pack(l.0, l.1));
        b[9..12].copy_from_slice(&pack(r.0, r.1));
        b
    }

    fn neutral(b3: u8, b4: u8, b5: u8) -> [u8; 20] {
        report(b3, b4, b5, (2048, 2048), (2048, 2048))
    }

    #[test]
    fn subcmd_layout() {
        let out = build_subcmd(0x7, 0x10, &[1, 2, 3]);
        assert_eq!(out.len(), 49);
        assert_eq!(out[0], 0x01);
        assert_eq!(out[1], 0x7);
        assert_eq!(&out[2..10], &[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40]);
        assert_eq!(out[10], 0x10);
        assert_eq!(&out[11..14], &[1, 2, 3]);
        assert!(out[14..].iter().all(|&b| b == 0));
    }

    #[test]
    fn spi_args_le() {
        assert_eq!(spi_read_args(0x603D, 0x12), [0x3D, 0x60, 0, 0, 0x12]);
    }

    #[test]
    fn parse_21_ack_and_data() {
        let mut b = [0u8; 40];
        b[0] = 0x21;
        b[13] = 0x90;
        b[14] = 0x10;
        b[15] = 0xAA;
        let r = parse_21(&b).unwrap();
        assert!(r.ack);
        assert_eq!(r.id, 0x10);
        assert_eq!(r.data[0], 0xAA);
        b[13] = 0x00;
        assert!(!parse_21(&b).unwrap().ack);
    }

    #[test]
    fn parse_21_rejects_non_21() {
        let mut b = [0u8; 40];
        b[0] = 0x30;
        assert!(parse_21(&b).is_none());
        b[0] = 0x21;
        assert!(parse_21(&b[..10]).is_none());
    }

    #[test]
    fn decode_30_each_button() {
        let cases: &[(usize, u8, usize)] = &[
            (3, 0x01, 2), (3, 0x02, 3), (3, 0x04, 0), (3, 0x08, 1), (3, 0x40, 10), (3, 0x80, 24),
            (4, 0x01, 4), (4, 0x02, 6), (4, 0x04, 8), (4, 0x08, 7), (4, 0x10, 5), (4, 0x20, 29),
            (5, 0x01, 12), (5, 0x02, 11), (5, 0x04, 14), (5, 0x08, 13), (5, 0x40, 9), (5, 0x80, 23),
        ];
        for &(byte, mask, id) in cases {
            let mut b = neutral(0, 0, 0);
            b[byte] = mask;
            let s = decode_30(&b, &StickCal::default()).unwrap();
            for (i, &p) in s.buttons.iter().enumerate() {
                assert_eq!(p, i == id, "byte {byte} mask {mask:#x}: slot {i}");
            }
        }
    }

    #[test]
    fn decode_30_zl_zr_set_trigger_and_button() {
        let s = decode_30(&neutral(0x80, 0, 0x80), &StickCal::default()).unwrap();
        assert!(s.buttons[23] && s.buttons[24]);
        assert_eq!((s.trigger_l2, s.trigger_r2), (1.0, 1.0));
        let s = decode_30(&neutral(0, 0, 0), &StickCal::default()).unwrap();
        assert_eq!((s.trigger_l2, s.trigger_r2), (0.0, 0.0));
    }

    #[test]
    fn decode_30_zl_only() {
        let s = decode_30(&neutral(0, 0, 0x80), &StickCal::default()).unwrap();
        assert_eq!((s.trigger_l2, s.trigger_r2), (1.0, 0.0));
        assert!(s.buttons[23]);
        assert!(!s.buttons[24]);
    }

    #[test]
    fn decode_30_zr_only() {
        let s = decode_30(&neutral(0x80, 0, 0), &StickCal::default()).unwrap();
        assert_eq!((s.trigger_l2, s.trigger_r2), (0.0, 1.0));
        assert!(!s.buttons[23]);
        assert!(s.buttons[24]);
    }

    #[test]
    fn parse_21_ack_bit7_only() {
        let mut b = [0u8; 40];
        b[0] = 0x21;
        b[13] = 0x80;
        assert!(parse_21(&b).unwrap().ack, "bit 7 alone = ACK");
        b[13] = 0x10;
        assert!(!parse_21(&b).unwrap().ack, "low bits without bit 7 = NACK");
    }

    #[test]
    fn subcmd_counter_masked() {
        assert_eq!(build_subcmd(0x13, 0x03, &[0x30])[1], 0x03);
    }

    #[test]
    fn decode_30_rejects_bad() {
        assert!(decode_30(&[0x30; 11], &StickCal::default()).is_none());
        let mut b = neutral(0, 0, 0);
        b[0] = 0x31;
        assert!(decode_30(&b, &StickCal::default()).is_none());
    }

    #[test]
    fn decode_30_stick_center_is_zero() {
        let s = decode_30(&neutral(0, 0, 0), &StickCal::default()).unwrap();
        assert_eq!((s.stick_lx, s.stick_ly, s.stick_rx, s.stick_ry), (0.0, 0.0, 0.0, 0.0));
        let mut cal = StickCal::default();
        cal.lx.center = 1900;
        let b = report(0, 0, 0, (1900, 2048), (2048, 2048));
        assert_eq!(decode_30(&b, &cal).unwrap().stick_lx, 0.0);
    }

    #[test]
    fn decode_30_stick_extremes_clamp() {
        let b = report(0, 0, 0, (0, 4095), (4095, 0));
        let s = decode_30(&b, &StickCal::default()).unwrap();
        assert_eq!(s.stick_lx, -1.0);
        assert_eq!(s.stick_ly, 1.0);
        assert_eq!(s.stick_rx, 1.0);
        assert_eq!(s.stick_ry, -1.0);
    }

    #[test]
    fn decode_30_y_up_positive() {
        let b = report(0, 0, 0, (2048, 2048 + 700), (2048, 2048 - 700));
        let s = decode_30(&b, &StickCal::default()).unwrap();
        assert!((s.stick_ly - 0.5).abs() < 1e-6);
        assert!((s.stick_ry + 0.5).abs() < 1e-6);
    }

    #[test]
    fn decode_30_zero_span_no_div_by_zero() {
        let mut cal = StickCal::default();
        cal.lx.span_pos = 0;
        cal.lx.span_neg = 0;
        let b = report(0, 0, 0, (4095, 2048), (2048, 2048));
        let s = decode_30(&b, &cal).unwrap();
        assert!(s.stick_lx.is_finite());
    }

    #[test]
    fn factory_cal_uncalibrated_falls_back() {
        assert_eq!(parse_factory_cal(&[0xFF; 0x12]), StickCal::default());
    }

    #[test]
    fn factory_cal_parses_layouts() {
        let mut spi = [0u8; 18];
        // left: max (2000,2100), center (2050,2060), min (1000,1100)
        spi[0..3].copy_from_slice(&pack(2000, 2100));
        spi[3..6].copy_from_slice(&pack(2050, 2060));
        spi[6..9].copy_from_slice(&pack(1000, 1100));
        // right: center (2070,2080), min (1200,1300), max (1500,1600)
        spi[9..12].copy_from_slice(&pack(2070, 2080));
        spi[12..15].copy_from_slice(&pack(1200, 1300));
        spi[15..18].copy_from_slice(&pack(1500, 1600));
        let c = parse_factory_cal(&spi);
        assert_eq!(c.lx, AxisCal { center: 2050, span_neg: 1000, span_pos: 2000 });
        assert_eq!(c.ly, AxisCal { center: 2060, span_neg: 1100, span_pos: 2100 });
        assert_eq!(c.rx, AxisCal { center: 2070, span_neg: 1200, span_pos: 1500 });
        assert_eq!(c.ry, AxisCal { center: 2080, span_neg: 1300, span_pos: 1600 });
    }

    #[test]
    fn user_cal_requires_magic() {
        let mut spi = [0u8; 0x16];
        assert!(parse_user_cal(&spi).is_none());
        spi[0] = 0xB2;
        spi[1] = 0xA1;
        assert!(parse_user_cal(&spi).is_none(), "right magic missing");
        spi[11] = 0xB2;
        spi[12] = 0xA1;
        spi[2..5].copy_from_slice(&pack(1800, 1900));
        spi[5..8].copy_from_slice(&pack(2000, 2010));
        spi[8..11].copy_from_slice(&pack(900, 950));
        spi[13..16].copy_from_slice(&pack(2020, 2030));
        spi[16..19].copy_from_slice(&pack(1000, 1100));
        spi[19..22].copy_from_slice(&pack(1200, 1300));
        let c = parse_user_cal(&spi).unwrap();
        assert_eq!(c.lx, AxisCal { center: 2000, span_neg: 900, span_pos: 1800 });
        assert_eq!(c.ry, AxisCal { center: 2030, span_neg: 1100, span_pos: 1300 });
        assert!(parse_user_cal(&spi[..0x10]).is_none());
    }
}
