//! Xbox Nano through XInput (Windows), when Microsoft's Xbox driver owns it.
//!
//! With Microsoft's "Xbox Wireless Accessory" driver installed (the
//! `xinputhid` upper filter on the Nano's HID-over-GATT node) the Nano's
//! gamepad collection still opens through hidapi but sends no report at all
//! (35 s of input, 0 reports, on the maintainer's PC, 2026-11-09); its input
//! goes to XInput. Its HID instance id then carries `&IG_`
//! (`HID\{00001812-…}&DEV&VID_045E&PID_0B4F&REV_0619&<address>&COL01&IG_00\…`),
//! the mark of an XInput-owned collection, so the Nano worker picks the
//! backend from the device path (`uses_xinput`): `&IG_` → this module,
//! anything else → the hidapi report path (`xbox_protocol`).
//!
//! * The Nano's slot is the one whose `XInputGetCapabilitiesEx` (xinput1_4
//!   ordinal 108) says 045E:0B4F; our own ViGEm virtual Xbox 360 pad
//!   (045E:028E) is never taken. Without ordinal 108 a slot is taken only when
//!   it is the one slot connected and not our virtual pad (`pick_slot`).
//! * Buttons come from `XInputGetStateEx` (ordinal 100, which includes the
//!   Xbox key), or `XInputGetState` without the Xbox key when it is missing.
//!   The slot is polled every `POLL`; only a new `dwPacketNumber` makes a new
//!   `PadState` (`to_pad_state`, the same convention as `decode_xbox_bt`).
//! * Connection state is XInput's: the first successful read connects, a
//!   failed read (`ERROR_DEVICE_NOT_CONNECTED`) ends the session (iron rule
//!   11). Nothing is ever written to the pad (no rumble; a manual disconnect only ends the session, the Nano cannot be
//!   turned off from the PC).
//!
//! Share (button 29) is not in any XInput state: it never reaches the
//! mapper. macOS: Phase 2 TBD, like the rest of the Nano.

use super::controller::ControllerKind;
use super::events::GamepadEvent;
use super::pad::{emit_pad_diff, PadState, BUTTON_SLOTS};
use super::SourceCmd;
use crossbeam_channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The Xbox Nano's vendor / product id as XInput reports it.
pub const NANO_IDS: (u16, u16) = (0x045e, 0x0b4f);
/// XInput user slots.
pub const SLOTS: u32 = 4;
/// How often the Nano's slot is read.
pub const POLL: Duration = Duration::from_millis(4);

/// One XInput read of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XRead {
    pub packet: u32,
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

/// What a slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotInfo {
    /// Nothing connected.
    Empty,
    /// A pad with this vendor / product id.
    Ids(u16, u16),
    /// A pad whose ids XInput did not give (no ordinal 108).
    Unknown,
}

/// Where our own ViGEm virtual Xbox 360 pad is (`xinput_sink::own_slot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnPad {
    /// Not plugged.
    None,
    /// Plugged, in this XInput slot.
    Slot(u32),
    /// Plugged, slot not known.
    Unknown,
}

/// The XInput calls the Nano worker makes (a fake in tests).
pub trait XInputApi {
    fn slot_info(&self, slot: u32) -> SlotInfo;
    /// `None`: the slot is not connected (or the read failed).
    fn state(&self, slot: u32) -> Option<XRead>;
}

/// Microsoft's Xbox driver owns the HID collection at `path`: its device id
/// carries `&IG_` (any case). Read the pad through XInput then.
pub fn uses_xinput(path: &str) -> bool {
    path.to_ascii_lowercase().contains("&ig_")
}

/// XInput button bits → button ids (the Nano's ids, DualSense numbering).
const BUTTON_BITS: [(u16, usize); 15] = [
    (0x1000, 0),  // A
    (0x2000, 1),  // B
    (0x4000, 2),  // X
    (0x8000, 3),  // Y
    (0x0020, 4),  // Back → View
    (0x0400, 5),  // Guide → Xbox (XInputGetStateEx only)
    (0x0010, 6),  // Start → Menu
    (0x0040, 7),  // left thumb
    (0x0080, 8),  // right thumb
    (0x0100, 9),  // LB
    (0x0200, 10), // RB
    (0x0001, 11), // D-pad up
    (0x0002, 12), // down
    (0x0004, 13), // left
    (0x0008, 14), // right
];

/// One XInput read as a `PadState`: the same convention as
/// `decode_xbox_bt` (sticks in [-1, 1], up positive — XInput's Y already is;
/// triggers in [0, 1]).
pub fn to_pad_state(r: &XRead) -> PadState {
    let axis = |v: i16| (v as f32 / 32767.0).clamp(-1.0, 1.0);
    let mut s = PadState::default();
    s.stick_lx = axis(r.lx);
    s.stick_ly = axis(r.ly);
    s.stick_rx = axis(r.rx);
    s.stick_ry = axis(r.ry);
    s.trigger_l2 = r.lt as f32 / 255.0;
    s.trigger_r2 = r.rt as f32 / 255.0;
    for (bit, id) in BUTTON_BITS {
        s.buttons[id] = r.buttons & bit != 0;
    }
    s
}

/// The Nano's slot among `slots`. The slot that says 045E:0B4F (our
/// virtual pad says 045E:028E); if no slot gave its ids (no ordinal 108),
/// the one connected slot that is not our virtual pad — none when our
/// virtual pad is plugged at an unknown slot or more than one other slot is
/// connected.
pub fn pick_slot(slots: &[SlotInfo], own: OwnPad) -> Option<u32> {
    let mine = |i: usize| own == OwnPad::Slot(i as u32);
    if let Some(i) = slots.iter().position(|s| *s == SlotInfo::Ids(NANO_IDS.0, NANO_IDS.1)) {
        return Some(i as u32);
    }
    if slots.iter().any(|s| matches!(s, SlotInfo::Ids(..))) || own == OwnPad::Unknown {
        return None;
    }
    let mut others = slots.iter().enumerate().filter(|(i, s)| **s == SlotInfo::Unknown && !mine(*i));
    match (others.next(), others.next()) {
        (Some((i, _)), None) => Some(i as u32),
        _ => None,
    }
}

/// Look the Nano's slot up (`pick_slot` over every slot).
pub fn find_slot(api: &dyn XInputApi, own: OwnPad) -> Option<u32> {
    let slots: Vec<SlotInfo> = (0..SLOTS).map(|i| api.slot_info(i)).collect();
    pick_slot(&slots, own)
}

/// Read the Nano at `slot` until XInput says it is gone, a manual disconnect
/// or `stop`. `Connected` on the first read, then the change of every new
/// packet. Returns the outcome and whether it connected; the caller sends
/// `Disconnected` (the engine then releases everything, iron rule 3).
pub fn run_session(
    api: &dyn XInputApi,
    slot: u32,
    tx: &Sender<GamepadEvent>,
    stop: &AtomicBool,
    cmd_rx: &Receiver<SourceCmd>,
    poll: Duration,
) -> (&'static str, bool) {
    let mut last: Option<PadState> = None;
    let mut prev_buttons = [false; BUTTON_SLOTS];
    let mut packet: Option<u32> = None;
    while !stop.load(Ordering::SeqCst) {
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                SourceCmd::Disconnect => return ("manual-disconnect", last.is_some()),
                // Nothing is written to the Nano.
                SourceCmd::Rumble => {}
            }
        }
        let Some(r) = api.state(slot) else {
            return ("xinput-gone", last.is_some());
        };
        if packet != Some(r.packet) {
            packet = Some(r.packet);
            let s = to_pad_state(&r);
            if last.is_none() {
                let _ = tx.send(GamepadEvent::Connected(ControllerKind::XboxNano));
            }
            emit_pad_diff(last.as_ref(), &s, &mut prev_buttons, tx);
            last = Some(s);
        }
        std::thread::sleep(poll);
    }
    ("stop-signal", last.is_some())
}

#[cfg(windows)]
pub use win::RealXInput;

#[cfg(windows)]
mod win {
    use super::{SlotInfo, XInputApi, XRead};
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

    #[repr(C)]
    #[derive(Default)]
    struct Gamepad {
        buttons: u16,
        lt: u8,
        rt: u8,
        lx: i16,
        ly: i16,
        rx: i16,
        ry: i16,
    }

    /// `XINPUT_STATE`, with room for the reserved DWORD some versions of
    /// `XInputGetStateEx` write after the gamepad (SDL's `XINPUT_STATE_EX`).
    #[repr(C)]
    #[derive(Default)]
    struct StateEx {
        packet: u32,
        pad: Gamepad,
        _reserved: u32,
    }

    /// `XINPUT_CAPABILITIES_EX` (SDL): `XINPUT_CAPABILITIES` then the ids.
    #[repr(C)]
    #[derive(Default)]
    struct CapsEx {
        kind: u8,
        sub_kind: u8,
        flags: u16,
        pad: Gamepad,
        vib_left: u16,
        vib_right: u16,
        vendor: u16,
        product: u16,
        version: u16,
        _unk1: u16,
        _unk2: u32,
    }

    type GetState = unsafe extern "system" fn(u32, *mut StateEx) -> u32;
    type GetCapsEx = unsafe extern "system" fn(u32, u32, u32, *mut CapsEx) -> u32;

    /// xinput1_4.dll, loaded once at run time.
    pub struct RealXInput {
        get_state: GetState,
        caps_ex: Option<GetCapsEx>,
    }

    impl RealXInput {
        /// The loaded DLL; `None` when xinput1_4.dll or its state call is
        /// missing (logged once).
        pub fn get() -> Option<&'static RealXInput> {
            static API: OnceLock<Option<RealXInput>> = OnceLock::new();
            API.get_or_init(Self::load).as_ref()
        }

        fn load() -> Option<RealXInput> {
            let name: Vec<u16> = "xinput1_4.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated name; an ordinal goes in the low word
            // of the name pointer (MAKEINTRESOURCEA); every pointer returned
            // is cast to the documented signature of that export.
            unsafe {
                let module = LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
                if module.is_null() {
                    tracing::warn!("XInput: xinput1_4.dll not loaded; the Xbox Nano on Microsoft's driver cannot be read");
                    return None;
                }
                let ex = GetProcAddress(module, 100usize as *const u8);
                let plain = GetProcAddress(module, b"XInputGetState\0".as_ptr());
                let caps = GetProcAddress(module, 108usize as *const u8);
                tracing::info!(
                    get_state_ex = ex.is_some(),
                    get_capabilities_ex = caps.is_some(),
                    "XInput: xinput1_4.dll loaded"
                );
                let get_state: GetState = match ex.or(plain) {
                    Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, GetState>(f),
                    None => {
                        tracing::warn!("XInput: no XInputGetState in xinput1_4.dll");
                        return None;
                    }
                };
                let caps_ex = caps.map(|f| std::mem::transmute::<unsafe extern "system" fn() -> isize, GetCapsEx>(f));
                Some(RealXInput { get_state, caps_ex })
            }
        }
    }

    impl XInputApi for RealXInput {
        fn slot_info(&self, slot: u32) -> SlotInfo {
            match self.caps_ex {
                Some(f) => {
                    let mut c = CapsEx::default();
                    // SAFETY: a valid out-pointer to a struct of the size SDL passes.
                    match unsafe { f(1, slot, 0, &mut c) } {
                        0 => SlotInfo::Ids(c.vendor, c.product),
                        ERROR_DEVICE_NOT_CONNECTED => SlotInfo::Empty,
                        _ if self.state(slot).is_some() => SlotInfo::Unknown,
                        _ => SlotInfo::Empty,
                    }
                }
                None if self.state(slot).is_some() => SlotInfo::Unknown,
                None => SlotInfo::Empty,
            }
        }

        fn state(&self, slot: u32) -> Option<XRead> {
            let mut s = StateEx::default();
            // SAFETY: a valid out-pointer at least as large as XINPUT_STATE.
            if unsafe { (self.get_state)(slot, &mut s) } != 0 {
                return None;
            }
            let g = &s.pad;
            Some(XRead { packet: s.packet, buttons: g.buttons, lt: g.lt, rt: g.rt, lx: g.lx, ly: g.ly, rx: g.rx, ry: g.ry })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[test]
    fn every_xinput_button_bit_maps_to_its_nano_id() {
        let want: [(u16, usize, &str); 15] = [
            (0x1000, 0, "A"),
            (0x2000, 1, "B"),
            (0x4000, 2, "X"),
            (0x8000, 3, "Y"),
            (0x0020, 4, "View (Back)"),
            (0x0400, 5, "Xbox (Guide)"),
            (0x0010, 6, "Menu (Start)"),
            (0x0040, 7, "LS"),
            (0x0080, 8, "RS"),
            (0x0100, 9, "LB"),
            (0x0200, 10, "RB"),
            (0x0001, 11, "up"),
            (0x0002, 12, "down"),
            (0x0004, 13, "left"),
            (0x0008, 14, "right"),
        ];
        for (bit, id, name) in want {
            let s = to_pad_state(&XRead { buttons: bit, ..XRead::default() });
            let down: Vec<usize> = (0..BUTTON_SLOTS).filter(|&i| s.buttons[i]).collect();
            assert_eq!(down, vec![id], "{name} {bit:#06x}");
        }
        let none = to_pad_state(&XRead::default());
        assert!(none.buttons.iter().all(|b| !b), "nothing held");
        // Share (29) and the analog-derived LT / RT ids are never set.
        let all = to_pad_state(&XRead { buttons: 0xFFFF, ..XRead::default() });
        assert!(!all.buttons[29] && !all.buttons[23] && !all.buttons[24]);
    }

    #[test]
    fn triggers_and_sticks_follow_the_decode_xbox_bt_convention() {
        let s = to_pad_state(&XRead { lt: 255, rt: 0, lx: 32767, ly: 32767, rx: -32768, ry: -32767, ..XRead::default() });
        assert_eq!((s.trigger_l2, s.trigger_r2), (1.0, 0.0));
        // XInput's Y is up-positive already: pushed up reads +1, as decode_xbox_bt.
        assert_eq!((s.stick_lx, s.stick_ly), (1.0, 1.0));
        assert_eq!((s.stick_rx, s.stick_ry), (-1.0, -1.0), "-32768 clamps to -1");
        let half = to_pad_state(&XRead { lt: 51, rt: 204, lx: 0, ly: -16384, ..XRead::default() });
        assert!((half.trigger_l2 - 0.2).abs() < 1e-6 && (half.trigger_r2 - 0.8).abs() < 1e-6);
        assert_eq!(half.stick_lx, 0.0);
        assert!(half.stick_ly < -0.49 && half.stick_ly > -0.51, "pulled down is negative");
    }

    const NANO: SlotInfo = SlotInfo::Ids(0x045e, 0x0b4f);
    const VIGEM: SlotInfo = SlotInfo::Ids(0x045e, 0x028e);
    use SlotInfo::{Empty, Unknown};

    #[test]
    fn the_nano_slot_is_the_one_with_its_ids() {
        assert_eq!(pick_slot(&[NANO, Empty, Empty, Empty], OwnPad::None), Some(0));
        assert_eq!(pick_slot(&[VIGEM, NANO, Empty, Empty], OwnPad::Slot(0)), Some(1), "our virtual pad in 0");
        assert_eq!(pick_slot(&[VIGEM, Empty, Empty, NANO], OwnPad::Unknown), Some(3));
        assert_eq!(pick_slot(&[VIGEM, Empty, Empty, Empty], OwnPad::Slot(0)), None, "our virtual pad only");
        assert_eq!(pick_slot(&[VIGEM, Empty, Empty, Empty], OwnPad::None), None, "an Xbox 360 pad is not the Nano");
        assert_eq!(pick_slot(&[Empty, Empty, Empty, Empty], OwnPad::None), None);
        assert_eq!(pick_slot(&[SlotInfo::Ids(0x045e, 0x0b13), Empty, Empty, Empty], OwnPad::None), None, "an Xbox Series pad");
    }

    #[test]
    fn without_ordinal_108_only_a_lone_slot_that_is_not_ours_is_taken() {
        assert_eq!(pick_slot(&[Empty, Unknown, Empty, Empty], OwnPad::None), Some(1));
        assert_eq!(pick_slot(&[Unknown, Unknown, Empty, Empty], OwnPad::None), None, "two pads: cannot tell");
        assert_eq!(pick_slot(&[Unknown, Unknown, Empty, Empty], OwnPad::Slot(0)), Some(1), "the other one");
        assert_eq!(pick_slot(&[Unknown, Empty, Empty, Empty], OwnPad::Slot(0)), None, "only ours");
        assert_eq!(pick_slot(&[Unknown, Empty, Empty, Empty], OwnPad::Unknown), None, "ours somewhere");
        assert_eq!(pick_slot(&[Empty; 4], OwnPad::None), None);
    }

    #[test]
    fn the_backend_follows_the_ig_mark_in_the_path() {
        // hidapi paths read on the maintainer's PC (2026-11-09).
        let official = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}&Dev&VID_045e&PID_0b4f&REV_0619&fb3a7c7df9a8&Col01&IG_00#8&4ca2c2&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let plain = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col01#8&113d3dd1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert!(uses_xinput(official));
        assert!(uses_xinput(&official.to_ascii_lowercase()), "any case");
        assert!(!uses_xinput(plain));
        assert!(!uses_xinput(r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6#9&2d1c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"));
    }

    /// Scripted slot reads; `None` = not connected. The last entry repeats.
    /// (`XInputApi` has no write call: nothing can be sent to the pad.)
    struct FakeX {
        slots: [SlotInfo; 4],
        reads: RefCell<VecDeque<Option<XRead>>>,
    }
    impl XInputApi for FakeX {
        fn slot_info(&self, slot: u32) -> SlotInfo {
            self.slots[slot as usize]
        }
        fn state(&self, _slot: u32) -> Option<XRead> {
            let mut r = self.reads.borrow_mut();
            if r.len() > 1 { r.pop_front().unwrap() } else { r.front().copied().flatten() }
        }
    }

    fn read(packet: u32, buttons: u16) -> Option<XRead> {
        Some(XRead { packet, buttons, ..XRead::default() })
    }

    #[test]
    fn a_session_connects_on_the_first_read_and_ends_when_xinput_says_gone() {
        let x = FakeX {
            slots: [VIGEM, NANO, Empty, Empty],
            reads: RefCell::new(VecDeque::from([read(1, 0x1000), read(1, 0x1000), read(2, 0x2000), read(2, 0xFFFF), None])),
        };
        let slot = find_slot(&x, OwnPad::Slot(0)).expect("the Nano's slot");
        assert_eq!(slot, 1);
        let (tx, rx) = unbounded();
        let (_ctx, crx) = unbounded();
        let (outcome, connected) = run_session(&x, slot, &tx, &AtomicBool::new(false), &crx, Duration::ZERO);
        assert_eq!((outcome, connected), ("xinput-gone", true));
        let evs: Vec<GamepadEvent> = rx.try_iter().collect();
        assert_eq!(evs.first(), Some(&GamepadEvent::Connected(ControllerKind::XboxNano)));
        let buttons: Vec<&GamepadEvent> = evs.iter().filter(|e| matches!(e, GamepadEvent::ButtonDown(_) | GamepadEvent::ButtonUp(_))).collect();
        // Packet 2 repeated with other buttons is not a change (the packet number decides).
        assert_eq!(buttons, vec![&GamepadEvent::ButtonDown(0), &GamepadEvent::ButtonUp(0), &GamepadEvent::ButtonDown(1)], "{evs:?}");
    }

    #[test]
    fn a_session_ends_on_a_manual_disconnect_and_ignores_rumble() {
        let x = FakeX { slots: [NANO, Empty, Empty, Empty], reads: RefCell::new(VecDeque::from([read(7, 0)])) };
        let (tx, _rx) = unbounded();
        let (ctx, crx) = unbounded();
        ctx.send(SourceCmd::Rumble).unwrap();
        ctx.send(SourceCmd::Disconnect).unwrap();
        assert_eq!(run_session(&x, 0, &tx, &AtomicBool::new(false), &crx, Duration::ZERO).0, "manual-disconnect");
        let stop = AtomicBool::new(true);
        assert_eq!(run_session(&x, 0, &tx, &stop, &crx, Duration::ZERO), ("stop-signal", false));
    }

    #[test]
    fn a_slot_not_connected_at_the_first_read_never_connects() {
        let x = FakeX { slots: [NANO, Empty, Empty, Empty], reads: RefCell::new(VecDeque::from([None])) };
        let (tx, rx) = unbounded();
        let (_ctx, crx) = unbounded();
        assert_eq!(run_session(&x, 0, &tx, &AtomicBool::new(false), &crx, Duration::ZERO), ("xinput-gone", false));
        assert_eq!(rx.try_iter().count(), 0, "no Connected");
    }

    /// Read-only look at the XInput slots of this machine: which slot holds
    /// what (the maintainer's Nano on Microsoft's driver: 045E:0B4F).
    #[cfg(windows)]
    #[test]
    fn lists_the_xinput_slots_of_this_machine() {
        let Some(api) = RealXInput::get() else {
            println!("xinput1_4.dll not loaded");
            return;
        };
        for i in 0..SLOTS {
            println!("XInput slot {i}: {:?} state={:?}", api.slot_info(i), api.state(i));
        }
        println!("Nano slot: {:?}", find_slot(api, OwnPad::None));
    }
}
