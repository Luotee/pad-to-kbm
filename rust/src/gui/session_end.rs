//! Windows logging off or shutting down while the window is open (v3.6.0).
//!
//! Then `run_return` never returns: the process ends once the windows have
//! answered `WM_ENDSESSION`, so the cleanup after the event loop never runs
//! and the HidHide entries would stay until the next start. A subclass on the
//! main window runs the same [`Cleanup`] synchronously, before it returns.
//! It is installed after tao's own subclass, so it sees the message first.
//! Windows only, like HidHide (macOS: Phase 2 TBD, CHANGELOG 3.6.0 Migration).

use crate::shutdown::Cleanup;
use std::sync::Arc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION};

/// Ours among the window's subclasses (tao uses 0 and 1).
const SUBCLASS_ID: usize = 0x5054_4B4D;

/// Hook `hwnd` (the main window). Returns whether the subclass went in.
pub fn install(hwnd: HWND, cleanup: Arc<Cleanup>) -> bool {
    let data = Arc::into_raw(cleanup);
    // SAFETY: `data` stays valid until WM_NCDESTROY takes it back below (or
    // right here when the call fails).
    let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, data as usize) } != 0;
    if !ok {
        unsafe { drop(Arc::from_raw(data)) };
    }
    ok
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    match msg {
        // Never hold the logoff / shutdown up.
        WM_QUERYENDSESSION => return 1,
        // wParam TRUE: the session really ends (FALSE: it was cancelled).
        WM_ENDSESSION if wparam != 0 => {
            let cleanup = &*(data as *const Cleanup);
            // A panic must not unwind across this extern fn (that aborts);
            // the panic hook has released the keys either way.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cleanup.run("session end")));
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            drop(Arc::from_raw(data as *const Cleanup));
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}
