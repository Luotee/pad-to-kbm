//! Virtual Xbox 360 pad backend (Xbox output mode, any pad).
//!
//! Iron rule 7 exception: a pad in Xbox output mode is shown to games as a
//! virtual Xbox 360 pad through an **externally installed** ViGEmBus driver
//! (user-mode client calls only; the app never ships or installs a driver,
//! and never hooks or injects). The engine owns the plug / unplug
//! lifecycle; this module only talks to the bus.
//!
//! * `VigemPad` (Windows): vigem-client 0.1 (MIT). `plug` connects to the
//!   bus, plugs an Xbox 360 wired target and waits until it accepts reports.
//! * `NullPad` (every platform): never plugs (`PadError::Unsupported`). The
//!   non-Windows builds and the engine's test seam use it.
//!
//! Stuck-input safety (iron rule 3 spirit): the engine unplugs on leaving
//! Xbox mode, Disconnected, pause and shutdown. If the engine thread unwinds, the
//! pad is dropped, and vigem-client's `Xbox360Wired` unplugs itself in its
//! own `Drop` (vigem-client 0.1.4 `src/x360.rs`, `impl Drop for
//! Xbox360Wired`). The panic hook in `main.rs` also calls
//! `emergency_unplug` for a panic on any other thread. A process that dies
//! without unplugging (killed, aborted) loses its pad too: ViGEmBus removes
//! a client's targets when its handle closes (checked on the dev machine,
//! 2026-10-11: a pad leaked with `mem::forget` + `process::exit` was gone
//! from XInput right after).

use crate::gamepad::xinput_source::OwnPad;
use crate::virtual_pad::XState;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, Weak};

/// Why a plug failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadError {
    /// ViGEmBus is not installed. Shown as the `vigem_missing` warning.
    BusNotFound,
    /// No virtual pad backend on this platform (or in this build).
    Unsupported,
    Other(String),
}

/// A virtual Xbox 360 pad. The engine calls `update` / `unplug` only after a
/// successful `plug`; implementations still make both harmless otherwise.
pub trait VirtualPad: Send {
    fn plug(&mut self) -> Result<(), PadError>;
    /// Report `s`. An `Err` means the bus refused it (e.g. WinError 259
    /// while the XInput side has no request pending): the engine retries
    /// with its latest state on the next tick.
    fn update(&mut self, s: &XState) -> Result<(), PadError>;
    fn unplug(&mut self);
}

/// The pad as the engine thread shares it with the panic hook.
pub type SharedPad = std::sync::Arc<Mutex<Box<dyn VirtualPad>>>;

/// A backend that never plugs.
#[derive(Debug, Default)]
pub struct NullPad;

impl VirtualPad for NullPad {
    fn plug(&mut self) -> Result<(), PadError> {
        Err(PadError::Unsupported)
    }
    fn update(&mut self, _s: &XState) -> Result<(), PadError> {
        Ok(())
    }
    fn unplug(&mut self) {}
}

/// The production backend for this platform.
pub fn platform_pad() -> Box<dyn VirtualPad> {
    #[cfg(windows)]
    {
        Box::new(VigemPad::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(NullPad)
    }
}

// ─── Panic-hook registry ─────────────────────────────────────────────────────

/// The production engine's pad, for the panic hook. `Weak`: the engine
/// thread holds the only lasting strong reference (a plug in flight holds
/// one until it returns), so an unwinding engine thread still drops (and so
/// unplugs) the pad.
static GLOBAL: OnceLock<Weak<Mutex<Box<dyn VirtualPad>>>> = OnceLock::new();

/// Bind the panic hook to the production engine's pad (first call wins, as
/// `safety::register_global`).
pub fn register_global(pad: Weak<Mutex<Box<dyn VirtualPad>>>) {
    let _ = GLOBAL.set(pad);
}

/// Unplug the registered pad. Called from the panic hook; never blocks for
/// long and never panics. A pad whose lock is held by the panicking engine
/// thread is skipped: that thread's unwinding drops it.
pub fn emergency_unplug() {
    let Some(pad) = GLOBAL.get().and_then(Weak::upgrade) else { return };
    for _ in 0..20 {
        match pad.try_lock() {
            Ok(mut p) => return p.unplug(),
            Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner().unplug(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

// ─── Where our virtual pad is in XInput ──────────────────────────────────────

/// `OWN_SLOT` values besides a slot number.
const OWN_NONE: u32 = u32::MAX;
const OWN_UNKNOWN: u32 = u32::MAX - 1;
/// The XInput slot our plugged virtual pad holds, for the Xbox Nano's XInput
/// reader (`gamepad::xinput_source`), which must never read it as the Nano.
static OWN_SLOT: AtomicU32 = AtomicU32::new(OWN_NONE);

/// Where our own virtual Xbox 360 pad is in XInput right now.
pub fn own_slot() -> OwnPad {
    match OWN_SLOT.load(Ordering::SeqCst) {
        OWN_NONE => OwnPad::None,
        OWN_UNKNOWN => OwnPad::Unknown,
        n => OwnPad::Slot(n),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn set_own_slot(own: OwnPad) {
    let v = match own {
        OwnPad::None => OWN_NONE,
        OwnPad::Unknown => OWN_UNKNOWN,
        OwnPad::Slot(n) => n,
    };
    OWN_SLOT.store(v, Ordering::SeqCst);
}

// ─── Windows: ViGEmBus ───────────────────────────────────────────────────────

#[cfg(windows)]
pub use vigem::VigemPad;

#[cfg(windows)]
mod vigem {
    use super::{set_own_slot, OwnPad, PadError, VirtualPad};
    use crate::virtual_pad::XState;
    use std::time::Duration;
    use vigem_client::{Client, Error, TargetId, XButtons, XGamepad, Xbox360Wired};

    /// The bus reports the target ready a moment before it can deliver
    /// reports: until the XInput side has a request pending, a report fails
    /// with WinError(259) (measured on the dev machine: the first one right
    /// after `wait_ready`, accepted ~50 ms later). Retry up to 40 × 50 ms.
    const FIRST_UPDATE_TRIES: u32 = 40;
    const FIRST_UPDATE_GAP: Duration = Duration::from_millis(50);

    /// Xbox 360 wired target over ViGEmBus. Dropping it unplugs the target
    /// (vigem-client's `Drop for Xbox360Wired`).
    #[derive(Default)]
    pub struct VigemPad {
        target: Option<Xbox360Wired<Client>>,
        update_failed: bool,
        /// Refused reports during the last `plug` (hardware test evidence).
        pub(super) plug_retries: u32,
    }

    /// The readiness probe. The bus answers a report equal to the one it
    /// holds (all zero after plug-in) with success without delivering
    /// anything, so a neutral probe proves nothing; this one must really be
    /// delivered. A 1/32767 left-stick X deflection is deep inside every
    /// dead zone, and the neutral report follows right after.
    fn probe() -> XGamepad {
        XGamepad { thumb_lx: 1, ..XGamepad::default() }
    }

    /// `update` until the bus accepts `g`; returns the refusals seen.
    fn submit_when_ready(t: &mut Xbox360Wired<Client>, g: &XGamepad) -> Result<u32, PadError> {
        let mut tries = 0;
        loop {
            match t.update(g) {
                Ok(()) => return Ok(tries),
                Err(e) if tries < FIRST_UPDATE_TRIES => {
                    tracing::debug!(error = %e, tries, "virtual pad: report not accepted yet");
                    tries += 1;
                    std::thread::sleep(FIRST_UPDATE_GAP);
                }
                Err(e) => return Err(other("first update", e)),
            }
        }
    }

    pub(super) fn to_xgamepad(s: &XState) -> XGamepad {
        XGamepad {
            buttons: XButtons { raw: s.buttons },
            left_trigger: s.lt,
            right_trigger: s.rt,
            thumb_lx: s.lx,
            thumb_ly: s.ly,
            thumb_rx: s.rx,
            thumb_ry: s.ry,
        }
    }

    fn other(what: &str, e: Error) -> PadError {
        PadError::Other(format!("{what}: {e}"))
    }

    impl VirtualPad for VigemPad {
        fn plug(&mut self) -> Result<(), PadError> {
            if self.target.is_some() {
                return Ok(());
            }
            let client = Client::connect().map_err(|e| match e {
                Error::BusNotFound => PadError::BusNotFound,
                e => other("ViGEmBus connect", e),
            })?;
            let mut t = Xbox360Wired::new(client, TargetId::XBOX360_WIRED);
            t.plugin().map_err(|e| other("plug", e))?;
            // From here on, dropping `t` unplugs it.
            t.wait_ready().map_err(|e| other("wait_ready", e))?;
            let tries = submit_when_ready(&mut t, &probe())?
                + submit_when_ready(&mut t, &XGamepad::default())?;
            // Its XInput slot, so the Xbox Nano's XInput reader never takes it.
            let own = match t.get_user_index() {
                Ok(i) => OwnPad::Slot(i),
                Err(_) => OwnPad::Unknown,
            };
            set_own_slot(own);
            tracing::info!(retries = tries, ?own, "virtual Xbox pad plugged");
            self.plug_retries = tries;
            self.target = Some(t);
            self.update_failed = false;
            Ok(())
        }

        fn update(&mut self, s: &XState) -> Result<(), PadError> {
            let Some(t) = self.target.as_mut() else { return Ok(()) };
            match t.update(&to_xgamepad(s)) {
                Ok(()) => {
                    self.update_failed = false;
                    Ok(())
                }
                Err(e) => {
                    // The engine retries every tick: log once per streak.
                    if !self.update_failed {
                        tracing::warn!(error = %e, "virtual pad report refused; retrying");
                    }
                    self.update_failed = true;
                    Err(other("update", e))
                }
            }
        }

        fn unplug(&mut self) {
            if let Some(mut t) = self.target.take() {
                set_own_slot(OwnPad::None);
                if let Err(e) = t.unplug() {
                    tracing::warn!(error = %e, "virtual pad unplug failed");
                }
                tracing::info!("virtual Xbox pad unplugged");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Count {
        unplugs: usize,
    }
    struct CountingPad(Arc<Mutex<Count>>);
    impl VirtualPad for CountingPad {
        fn plug(&mut self) -> Result<(), PadError> {
            Ok(())
        }
        fn update(&mut self, _s: &XState) -> Result<(), PadError> {
            Ok(())
        }
        fn unplug(&mut self) {
            self.0.lock().unwrap().unplugs += 1;
        }
    }

    #[test]
    fn null_pad_never_plugs() {
        let mut p = NullPad;
        assert_eq!(p.plug(), Err(PadError::Unsupported));
        assert_eq!(p.update(&XState::default()), Ok(()));
        p.unplug();
    }

    #[test]
    fn emergency_unplug_reaches_the_registered_pad() {
        let count = Arc::new(Mutex::new(Count::default()));
        let pad: SharedPad = Arc::new(Mutex::new(Box::new(CountingPad(count.clone()))));
        register_global(Arc::downgrade(&pad));
        emergency_unplug();
        assert_eq!(count.lock().unwrap().unplugs, 1);

        // Poisoned lock: still unplugs.
        let p2 = pad.clone();
        let _ = std::thread::spawn(move || {
            let _g = p2.lock().unwrap();
            panic!("poison");
        })
        .join();
        emergency_unplug();
        assert_eq!(count.lock().unwrap().unplugs, 2);

        // Owner gone: nothing to do, no panic.
        drop(pad);
        emergency_unplug();
        assert_eq!(count.lock().unwrap().unplugs, 2);
    }

    #[cfg(windows)]
    #[test]
    fn xbutton_bits_match_vigem() {
        use crate::virtual_pad::xbutton as xb;
        use vigem_client::XButtons as V;
        let pairs = [
            (xb::DPAD_UP, V::UP),
            (xb::DPAD_DOWN, V::DOWN),
            (xb::DPAD_LEFT, V::LEFT),
            (xb::DPAD_RIGHT, V::RIGHT),
            (xb::START, V::START),
            (xb::BACK, V::BACK),
            (xb::LEFT_THUMB, V::LTHUMB),
            (xb::RIGHT_THUMB, V::RTHUMB),
            (xb::LEFT_SHOULDER, V::LB),
            (xb::RIGHT_SHOULDER, V::RB),
            (xb::GUIDE, V::GUIDE),
            (xb::A, V::A),
            (xb::B, V::B),
            (xb::X, V::X),
            (xb::Y, V::Y),
        ];
        for (ours, theirs) in pairs {
            assert_eq!(ours, theirs);
        }
        let s = XState { buttons: xb::A | xb::Y, lt: 200, rt: 1, lx: -5, ly: 6, rx: -7, ry: 8 };
        let g = vigem::to_xgamepad(&s);
        assert_eq!(
            (g.buttons.raw, g.left_trigger, g.right_trigger, g.thumb_lx, g.thumb_ly, g.thumb_rx, g.thumb_ry),
            (xb::A | xb::Y, 200, 1, -5, 6, -7, 8)
        );
    }

    /// Hardware check, run by hand on a Windows machine with ViGEmBus:
    /// `<lib test exe> --ignored vigem_xinput_readback --nocapture`.
    /// Plugs a pad, pushes A+Y / LT=200, reads it back through
    /// `XInputGetState`, unplugs, and checks the slot is gone.
    #[cfg(windows)]
    #[test]
    #[ignore = "needs ViGEmBus; plugs a real virtual pad"]
    fn vigem_xinput_readback() {
        use crate::virtual_pad::xbutton as xb;
        use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

        fn slots() -> Vec<(u32, u16, u8)> {
            (0..4)
                .filter_map(|i| {
                    let mut st: XINPUT_STATE = unsafe { std::mem::zeroed() };
                    (unsafe { XInputGetState(i, &mut st) } == 0)
                        .then(|| (i, st.Gamepad.wButtons, st.Gamepad.bLeftTrigger))
                })
                .collect()
        }

        let before = slots();
        let mut pad = VigemPad::default();
        let t0 = std::time::Instant::now();
        pad.plug().expect("plug (is ViGEmBus installed?)");
        println!("plug took {:?}, {} refused reports while waiting", t0.elapsed(), pad.plug_retries);
        // Right after plug the pad must take a real report on the first try.
        pad.update(&XState { buttons: xb::A | xb::Y, lt: 200, ..XState::default() })
            .expect("first report after plug accepted");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let during = slots();
        pad.unplug();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let after = slots();
        println!("XInput slots before={before:?} during={during:?} after={after:?}");
        assert!(
            during.iter().any(|&(_, b, lt)| b & (xb::A | xb::Y) == xb::A | xb::Y && lt == 200),
            "XInput must read back A+Y / LT=200"
        );
        assert_eq!(after.len(), before.len(), "unplug must remove the pad");
    }
}
