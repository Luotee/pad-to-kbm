//! Xbox Nano Share button through Raw Input (Windows only).
//!
//! The Nano's Share is not in its gamepad report: the pad sends it as a
//! keystroke on its keyboard collection (0001/0006), which Windows owns, so
//! hidapi cannot open it. On Windows a thread with a message-only window
//! registers for keyboard Raw Input (`RIDEV_INPUTSINK`, so it hears keys
//! while the app is in the background) and forwards the key events whose
//! source device is the Nano's keyboard (`is_nano_keyboard`) as `NanoKey`s.
//! Every other keyboard's event is dropped on the spot: not stored, not
//! forwarded, not logged. Raw Input only reads; it does not hook, intercept
//! or change what Windows does with the key (iron rule 7).
//!
//! The HID worker owns what a `NanoKey` means: while a Nano session is open
//! it folds them through `ShareKeys` into button 29 of the Nano's state, so
//! Share takes the same path as every other Nano button (`SourceHub`,
//! mapper, `safety.rs`) and is released with the session on disconnect.
//! Other sessions drop them. macOS: Phase 2 TBD (no Share on the Nano).

/// One key event from the Nano's keyboard collection. `key` identifies the
/// key within the event stream (scan code plus E0 / E1 prefix, or the
/// virtual key when there is no scan code); it means nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NanoKey {
    pub key: u16,
    pub down: bool,
}

/// Whether a Raw Input device name (`RIDI_DEVICENAME`) is the Nano's:
/// Bluetooth LE names carry `VID&02045E_PID&0B4F`, USB names
/// `VID_045E&PID_0B4F` (case-insensitive).
pub fn is_nano_keyboard(device_name: &str) -> bool {
    let name = device_name.to_ascii_uppercase();
    name.contains("VID&02045E_PID&0B4F") || name.contains("VID_045E&PID_0B4F")
}

/// The keys of the Nano's keyboard collection held right now: Share is
/// pressed while any is down. One per Nano session (a new session starts
/// empty, so a key lost with a dropped link never sticks).
#[derive(Debug, Default)]
pub struct ShareKeys {
    held: Vec<u16>,
}

impl ShareKeys {
    /// Apply one key event. `Some(true)`: the first key went down (Share
    /// pressed); `Some(false)`: the last key came up (Share released);
    /// `None`: no change (another key, an auto-repeat, or the release of a
    /// key never seen down).
    pub fn key(&mut self, k: NanoKey) -> Option<bool> {
        let was = self.pressed();
        let at = self.held.iter().position(|&h| h == k.key);
        match (k.down, at) {
            (true, None) => self.held.push(k.key),
            (false, Some(i)) => {
                self.held.swap_remove(i);
            }
            _ => {}
        }
        let now = self.pressed();
        (now != was).then_some(now)
    }

    pub fn pressed(&self) -> bool {
        !self.held.is_empty()
    }
}

/// The running Raw Input reader. Dropping it ends its thread.
pub struct RawShare {
    #[cfg(windows)]
    thread_id: u32,
}

/// Start the Raw Input reader and return the receiver of the Nano's key
/// events. Off Windows (and if the reader cannot start) nothing is started
/// and the receiver never yields.
pub fn start() -> (crossbeam_channel::Receiver<NanoKey>, Option<RawShare>) {
    #[cfg(windows)]
    {
        let (tx, rx) = crossbeam_channel::unbounded();
        match win::spawn(tx) {
            Ok(thread_id) => return (rx, Some(RawShare { thread_id })),
            Err(e) => tracing::warn!(error = %e, "Nano Share: Raw Input reader did not start"),
        }
    }
    (crossbeam_channel::never(), None)
}

#[cfg(windows)]
impl Drop for RawShare {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        // SAFETY: posting to a thread id is valid even if it has exited.
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}

#[cfg(windows)]
mod win {
    use super::{is_nano_keyboard, NanoKey};
    use crossbeam_channel::Sender;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HANDLE, HWND};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, GetRegisteredRawInputDevices,
        RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK,
        RIDI_DEVICENAME, RID_INPUT, RIM_TYPEKEYBOARD,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, SetTimer, HWND_MESSAGE,
        MSG, RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1, WM_INPUT, WM_TIMER,
    };

    const USAGE_PAGE_GENERIC: u16 = 0x01;
    const USAGE_KEYBOARD: u16 = 0x06;
    /// How often the thread checks that keyboard Raw Input still comes to
    /// its window. Registration is per process and the last one wins; the
    /// GUI's event loop (tao) registers keyboard Raw Input for its own
    /// window when it starts, after the engine, and tauri does it again
    /// when the app is built. Nothing in the app uses tao's keyboard device
    /// events, so the reader takes the registration back.
    const RECHECK_MS: u32 = 2000;

    /// Start the reader thread; returns its thread id once its window is
    /// up and registered.
    pub(super) fn spawn(tx: Sender<NanoKey>) -> anyhow::Result<u32> {
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<u32, String>>(1);
        std::thread::Builder::new()
            .name("nano-share-rawinput".into())
            .spawn(move || run(tx, ready_tx))?;
        ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("reader thread ended before it started"))?
            .map_err(|e| anyhow::anyhow!(e))
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn run(tx: Sender<NanoKey>, ready: Sender<Result<u32, String>>) {
        // SAFETY: plain Win32 calls on this thread's own message-only
        // window; every pointer passed points at a live local.
        unsafe {
            let class = wide("Message");
            let hwnd = CreateWindowExW(
                0, class.as_ptr(), null(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), null_mut(),
                null(),
            );
            if hwnd.is_null() {
                let _ = ready.send(Err(format!("CreateWindowExW: {}", std::io::Error::last_os_error())));
                return;
            }
            if !register(hwnd) {
                let _ = ready.send(Err(format!(
                    "RegisterRawInputDevices: {}",
                    std::io::Error::last_os_error()
                )));
                DestroyWindow(hwnd);
                return;
            }
            SetTimer(hwnd, 1, RECHECK_MS, None);
            let _ = ready.send(Ok(GetCurrentThreadId()));
            tracing::info!("Nano Share: Raw Input reader started");

            let mut msg: MSG = zeroed();
            // 0: WM_QUIT (RawShare dropped); -1: error.
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                match msg.message {
                    WM_INPUT => {
                        if let Some(k) = nano_key(msg.lParam as _) {
                            let _ = tx.send(k);
                        }
                    }
                    WM_TIMER if !registered_to(hwnd) => {
                        tracing::info!("Nano Share: keyboard Raw Input taken over; registering again");
                        register(hwnd);
                    }
                    _ => {}
                }
                // WM_INPUT must reach DefWindowProc for its cleanup.
                DispatchMessageW(&msg);
            }
            DestroyWindow(hwnd);
        }
    }

    fn device() -> RAWINPUTDEVICE {
        RAWINPUTDEVICE {
            usUsagePage: USAGE_PAGE_GENERIC,
            usUsage: USAGE_KEYBOARD,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: null_mut(),
        }
    }

    unsafe fn register(hwnd: HWND) -> bool {
        let d = RAWINPUTDEVICE { hwndTarget: hwnd, ..device() };
        RegisterRawInputDevices(&d, 1, size_of::<RAWINPUTDEVICE>() as u32) != 0
    }

    /// Whether this process's keyboard Raw Input still goes to `hwnd`.
    unsafe fn registered_to(hwnd: HWND) -> bool {
        let mut list = [device(); 16];
        let mut n = list.len() as u32;
        let got = GetRegisteredRawInputDevices(
            list.as_mut_ptr(),
            &mut n,
            size_of::<RAWINPUTDEVICE>() as u32,
        );
        if got == u32::MAX {
            // More than 16 registrations, or an error: assume all is well
            // rather than fight over it every two seconds.
            return true;
        }
        list[..got as usize].iter().any(|d| {
            d.usUsagePage == USAGE_PAGE_GENERIC && d.usUsage == USAGE_KEYBOARD && d.hwndTarget == hwnd
        })
    }

    /// The `NanoKey` in a WM_INPUT, if it is a key event from the Nano's
    /// keyboard. Anything else is dropped here.
    unsafe fn nano_key(h: windows_sys::Win32::UI::Input::HRAWINPUT) -> Option<NanoKey> {
        let mut data: RAWINPUT = zeroed();
        let mut size = size_of::<RAWINPUT>() as u32;
        let got = GetRawInputData(
            h,
            RID_INPUT,
            &mut data as *mut RAWINPUT as *mut _,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if got == u32::MAX || got == 0 || data.header.dwType != RIM_TYPEKEYBOARD {
            return None;
        }
        if !device_is_nano(data.header.hDevice) {
            return None;
        }
        let kb = data.data.keyboard;
        let flags = kb.Flags as u32;
        let key = if kb.MakeCode != 0 {
            kb.MakeCode
                | if flags & RI_KEY_E0 != 0 { 0x100 } else { 0 }
                | if flags & RI_KEY_E1 != 0 { 0x200 } else { 0 }
        } else {
            0x400 | kb.VKey
        };
        Some(NanoKey { key, down: flags & RI_KEY_BREAK == 0 })
    }

    /// Read the device's name (no cache: device handles can be reused
    /// after a removal, and keystrokes are rare enough).
    unsafe fn device_is_nano(dev: HANDLE) -> bool {
        if dev.is_null() {
            // Injected input (SendInput) has no device.
            return false;
        }
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let got = GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, name.as_mut_ptr() as *mut _, &mut len);
        if got == u32::MAX || got == 0 {
            return false;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(got as usize).min(name.len());
        is_nano_keyboard(&String::from_utf16_lossy(&name[..end]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Nano's keyboard collection as Windows names it (hardware, 2026-11-08).
    const NANO_KBD: &str = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col02#8&113d3dd1&0&0001#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}\KBD";

    #[test]
    fn the_nano_keyboard_is_recognised_by_name() {
        assert!(is_nano_keyboard(NANO_KBD));
        assert!(is_nano_keyboard(&NANO_KBD.to_ascii_uppercase()));
        // The USB naming form.
        assert!(is_nano_keyboard(r"\\?\HID#VID_045E&PID_0B4F&MI_01&Col02#7&1&0&0001#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"));
    }

    #[test]
    fn other_keyboards_and_pads_are_not_the_nano() {
        for name in [
            // An ordinary USB keyboard.
            r"\\?\HID#VID_046D&PID_C52B&MI_00#8&2a7c&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}",
            // A DualSense over Bluetooth.
            r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6#9&2d1c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}",
            // An Xbox Series pad over Bluetooth LE (another PID).
            r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b13_REV&0509_c8f7&Col02#9&1&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}\KBD",
            // PID 0B4F under another vendor.
            r"\\?\HID#VID_046D&PID_0B4F#8&1&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}",
            "",
        ] {
            assert!(!is_nano_keyboard(name), "{name}");
        }
    }

    fn k(key: u16, down: bool) -> NanoKey {
        NanoKey { key, down }
    }

    #[test]
    fn interleaved_keys_make_one_press_and_one_release() {
        let mut s = ShareKeys::default();
        assert_eq!(s.key(k(0x5B, true)), Some(true), "first key down presses Share");
        assert_eq!(s.key(k(0x37, true)), None, "second key: still pressed");
        assert_eq!(s.key(k(0x5B, false)), None, "one key still down");
        assert!(s.pressed());
        assert_eq!(s.key(k(0x37, false)), Some(false), "last key up releases Share");
        assert!(!s.pressed());
    }

    #[test]
    fn repeats_and_unknown_releases_change_nothing() {
        let mut s = ShareKeys::default();
        assert_eq!(s.key(k(0x37, false)), None, "release of a key never seen down");
        assert_eq!(s.key(k(0x37, true)), Some(true));
        assert_eq!(s.key(k(0x37, true)), None, "auto-repeat");
        assert_eq!(s.key(k(0x37, true)), None, "auto-repeat");
        assert_eq!(s.key(k(0x5B, false)), None, "other key up");
        assert_eq!(s.key(k(0x37, false)), Some(false));
        assert_eq!(s.key(k(0x37, false)), None);
    }

    #[test]
    fn start_off_windows_starts_nothing() {
        if cfg!(windows) {
            return;
        }
        let (rx, guard) = start();
        assert!(guard.is_none());
        assert!(rx.try_recv().is_err());
    }
}
