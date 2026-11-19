//! Xbox Nano companion protocol (v3.10.0): the link layer Microsoft's Xbox
//! Accessories SDK speaks over the Nano's vendor GATT service
//! `00000000-16e2-469d-8013-a85ccd1a3f11`, characteristic
//! `00000004-16e2-…` (writes from the host, indications from the pad).
//!
//! Worked out from a static disassembly of `xaccessories_sdk_svc.dll` (the
//! XboxAccessoriesService package) and checked on the maintainer's Nano
//! without Microsoft's driver (2026-11-19, issue #56):
//!
//! * frames: `06` no-op; `02 ch type next` ACK (next = seq + 1);
//!   `T ch type seq len payload` data (T = 05 single, 03 first, 01 middle,
//!   04 last fragment); `FF chbase id(u32 LE) maj min patch maxlen(u16 LE)`
//!   link hello. No checksum, no authentication.
//! * the host sends a link hello with a random id; the pad echoes it with
//!   the channel it assigns (`ch` for host requests, `ch + 1` for the pad's
//!   own messages). Other hosts (XboxAccessoriesService) hold other
//!   channels at the same time: their frames are never ours to ACK.
//! * a new message advances the sender's sequence number by 4.
//!
//! Pure: no I/O. Only the messages in the allow-list can be built
//! ([`data_frames`]): the app hello, the battery request and the power-off
//! (`0x24 00`). Firmware update, reboot, trace and crash-dump requests can
//! never be sent.

use std::collections::HashMap;

/// Battery request / reply: `[flags][percent]`, flags bit1 = charging.
pub const T_BATT: u8 = 0x02;
/// App hello for links up to 2.0.21.
pub const T_HELLO5: u8 = 0x23;
/// Reset: payload `00` = power off (`01` = reboot, never sent).
pub const T_RST: u8 = 0x24;
/// Battery state pushed by the pad (same payload as [`T_BATT`]).
pub const T_ST_BATT: u8 = 0x1B;
/// App hello for links after 2.0.21.
pub const T_HELLO6: u8 = 0x2E;

/// One frame as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Nop,
    Ack { ch: u8, ty: u8, next: u8 },
    Data { kind: u8, ch: u8, ty: u8, seq: u8, payload: Vec<u8> },
    LinkHello { chbase: u8, id: u32, ver: (u8, u8, u8), max_len: u16 },
    Unknown,
}

pub fn parse(f: &[u8]) -> Frame {
    match f {
        [6] => Frame::Nop,
        [2, ch, ty, next] => Frame::Ack { ch: *ch, ty: *ty, next: *next },
        [k @ (1 | 3 | 4 | 5), ch, ty, seq, len, rest @ ..] if rest.len() >= *len as usize => Frame::Data {
            kind: *k,
            ch: *ch,
            ty: *ty,
            seq: *seq,
            payload: rest[..*len as usize].to_vec(),
        },
        [0xff, chbase, a, b, c, d, maj, min, patch, l0, l1, ..] => Frame::LinkHello {
            chbase: *chbase,
            id: u32::from_le_bytes([*a, *b, *c, *d]),
            ver: (*maj, *min, *patch),
            max_len: u16::from_le_bytes([*l0, *l1]),
        },
        _ => Frame::Unknown,
    }
}

/// The host's link hello: protocol 2.0.22, messages up to 0x1002 bytes.
pub fn link_hello(id: u32) -> [u8; 11] {
    let i = id.to_le_bytes();
    [0xff, 0x00, i[0], i[1], i[2], i[3], 0x02, 0x00, 0x16, 0x02, 0x10]
}

/// ACK of the fragment `seq` received on `ch`.
pub fn ack(ch: u8, ty: u8, seq: u8) -> [u8; 4] {
    [2, ch, ty, seq.wrapping_add(1)]
}

/// A message type the allow-list refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub ty: u8,
}

fn allowed(ty: u8, payload: &[u8]) -> bool {
    match ty {
        T_BATT | T_HELLO5 | T_HELLO6 => true,
        T_RST => payload == [0],
        _ => false,
    }
}

/// The data frames of one message, fragments of at most `frag_max` payload
/// bytes with consecutive sequence numbers from `first_seq`. Refused for any
/// type outside the allow-list.
pub fn data_frames(ch: u8, ty: u8, first_seq: u8, payload: &[u8], frag_max: usize) -> Result<Vec<Vec<u8>>, Refused> {
    if !allowed(ty, payload) {
        return Err(Refused { ty });
    }
    let frag_max = frag_max.clamp(1, 0xfa);
    let chunks: Vec<&[u8]> = if payload.is_empty() { vec![&[][..]] } else { payload.chunks(frag_max).collect() };
    let n = chunks.len();
    Ok(chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let kind = match (i == 0, i + 1 == n) {
                (true, true) => 5,
                (true, false) => 3,
                (false, true) => 4,
                (false, false) => 1,
            };
            let mut f = vec![kind, ch, ty, first_seq.wrapping_add(i as u8), c.len() as u8];
            f.extend_from_slice(c);
            f
        })
        .collect())
}

/// App-hello payload: Unix seconds (i64 LE) + u32 6.
pub fn hello_payload(unix_secs: i64) -> Vec<u8> {
    let mut p = unix_secs.to_le_bytes().to_vec();
    p.extend_from_slice(&6u32.to_le_bytes());
    p
}

/// The app hello the pad's link version takes.
pub fn hello_type(ver: (u8, u8, u8)) -> u8 {
    if ver > (2, 0, 21) {
        T_HELLO6
    } else {
        T_HELLO5
    }
}

/// The capability word of the app-hello reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps(pub u32);

impl Caps {
    pub fn battery(&self) -> bool {
        self.0 & 0x2 != 0
    }
    pub fn shutdown(&self) -> bool {
        self.0 & 0x4 != 0
    }
}

/// Capabilities: u32 LE at 0x29 of the app-hello reply.
pub fn caps_from_hello(p: &[u8]) -> Option<Caps> {
    let b = p.get(0x29..0x2d)?;
    Some(Caps(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

/// Charging (external power) from a battery reply or push. Links after
/// 2.0.21: flags byte bit1; older: a third byte ≠ 0.
pub fn charging_from_batt(p: &[u8], ver: (u8, u8, u8)) -> Option<bool> {
    if ver > (2, 0, 21) {
        (p.len() >= 2).then(|| p[0] & 0x2 != 0)
    } else {
        p.get(2).map(|b| *b != 0)
    }
}

/// What an inbound frame meant for our session. `ack` is to be written
/// back by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    Nothing,
    Linked { ch: u8, ver: (u8, u8, u8) },
    Message { ch: u8, ty: u8, payload: Vec<u8>, ack: [u8; 4] },
    Fragment { ack: [u8; 4] },
}

/// One companion session: our link hello id, the channel the pad gave us,
/// our sequence number and the reassembly of the pad's messages.
#[derive(Debug)]
pub struct Session {
    id: u32,
    frag_max: usize,
    ch: Option<u8>,
    ver: (u8, u8, u8),
    seq: u8,
    asm: HashMap<u8, (u8, Vec<u8>)>,
}

impl Session {
    pub fn new(id: u32, frag_max: usize) -> Self {
        Self { id, frag_max, ch: None, ver: (0, 0, 0), seq: id as u8, asm: HashMap::new() }
    }

    pub fn channel(&self) -> Option<u8> {
        self.ch
    }

    pub fn ver(&self) -> (u8, u8, u8) {
        self.ver
    }

    pub fn on_frame(&mut self, f: &[u8]) -> Inbound {
        match parse(f) {
            Frame::LinkHello { chbase, id, ver, .. } if id == self.id && self.ch.is_none() => {
                self.ch = Some(chbase);
                self.ver = ver;
                Inbound::Linked { ch: chbase, ver }
            }
            Frame::Data { kind, ch, ty, seq, payload } => {
                let Some(ours) = self.ch else { return Inbound::Nothing };
                if ch != ours && ch != ours.wrapping_add(1) {
                    return Inbound::Nothing;
                }
                let a = ack(ch, ty, seq);
                if matches!(kind, 3 | 5) {
                    self.asm.insert(ch, (ty, Vec::new()));
                }
                let Some((_, buf)) = self.asm.get_mut(&ch) else { return Inbound::Fragment { ack: a } };
                buf.extend_from_slice(&payload);
                if matches!(kind, 4 | 5) {
                    let (ty, payload) = self.asm.remove(&ch).unwrap_or((ty, Vec::new()));
                    Inbound::Message { ch, ty, payload, ack: a }
                } else {
                    Inbound::Fragment { ack: a }
                }
            }
            _ => Inbound::Nothing,
        }
    }

    /// The frames of a new request (sequence + 4). Refused before the link
    /// or for a type outside the allow-list.
    pub fn request(&mut self, ty: u8, payload: &[u8]) -> Result<Vec<Vec<u8>>, Refused> {
        let ch = self.ch.ok_or(Refused { ty })?;
        let seq = self.seq.wrapping_add(4);
        let frames = data_frames(ch, ty, seq, payload, self.frag_max)?;
        self.seq = seq;
        Ok(frames)
    }
}
