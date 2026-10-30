//! HidHide for the Xbox output (v3.6.0): a probe, and a one-click install of
//! the **pinned official installer**.
//!
//! HidHide hides the real DualSense / Switch Pro from games while the app
//! feeds them a virtual Xbox pad, so a game does not see the same input
//! twice. The app never ships the driver (iron rule 7). On the user's click
//! it downloads the official HidHide v1.5.230 installer from GitHub into
//! `%TEMP%`, checks its size and sha256 against the pin below, and only then
//! runs it; the installer asks for elevation itself. A file that fails the
//! check is deleted and nothing runs (`installer.rs`, shared with ViGEmBus).
//!
//! The pinned file is Authenticode-signed by "Nefarius Software Solutions
//! e.U." (checked 2026-10-31). Windows only; macOS is Phase 2 TBD.

use crate::installer::{self, Pin};
use std::path::Path;

pub const INSTALLER_URL: &str =
    "https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe";
pub const INSTALLER_SIZE: u64 = 8_078_016;
pub const INSTALLER_SHA256: &str = "f4bbbcb82e6258641b887c74bc81c4c5f66e4aa811808dfc304347687b7605f6";
const INSTALLER_NAME: &str = "HidHide_1.5.230_x64.exe";

const PIN: Pin = Pin { url: INSTALLER_URL, name: INSTALLER_NAME, size: INSTALLER_SIZE, sha256: INSTALLER_SHA256 };

/// Check `path` against the pin (size first, then sha256). On a mismatch the
/// file is deleted and the reason returned.
pub fn verify_installer(path: &Path) -> Result<(), String> {
    installer::verify(&PIN, path)
}

/// `"installed"` (the HidHide service is registered), `"missing"`, or
/// `"unsupported"` (not Windows). Read-only.
pub fn status() -> &'static str {
    imp::status()
}

/// Download, verify and run the official installer; returns when the
/// installer process exits. Blocking: call it off the UI thread.
pub fn install() -> Result<(), String> {
    imp::install()
}

#[cfg(not(windows))]
mod imp {
    pub fn status() -> &'static str {
        "unsupported"
    }
    pub fn install() -> Result<(), String> {
        Err("HidHide is Windows only".into())
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ};

    /// The driver's service key. Why not open the control device
    /// `\\.\HidHide` instead: HidHide is a filter driver, so it is loaded
    /// (and creates the control device, `OnDriverCreate` in the driver's
    /// Logic.c) only while a device it filters is running — e.g. not yet
    /// right after the installer, before the HID stacks restart — and the
    /// control device is exclusive (`WdfDeviceInitSetExclusive`, one handle
    /// at a time, ControlDevice.c). A failed open therefore says nothing
    /// about whether HidHide is installed.
    const SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\HidHide";

    pub fn status() -> &'static str {
        let key: Vec<u16> = SERVICE_KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey: HKEY = std::ptr::null_mut();
        // SAFETY: NUL-terminated key name that outlives the call; the key is
        // closed again when it was opened.
        let rc = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key.as_ptr(), 0, KEY_READ, &mut hkey) };
        if rc == ERROR_SUCCESS {
            unsafe { RegCloseKey(hkey) };
            "installed"
        } else {
            "missing"
        }
    }

    pub fn install() -> Result<(), String> {
        crate::installer::download_verify_run(&super::PIN)
    }
}
