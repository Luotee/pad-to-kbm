//! ViGEmBus for the Xbox output (v3.2.0): a probe, and a one-click install
//! of the **pinned official installer**.
//!
//! The app never ships a driver (iron rule 7). On the user's click it
//! downloads the official ViGEmBus v1.22.0 installer from GitHub into
//! `%TEMP%`, checks its size and sha256 against the pin below, and only then
//! runs it; the installer asks for elevation itself. A file that fails the
//! check is deleted and nothing runs (`installer.rs`, shared with HidHide).
//! Windows only; macOS is Phase 2 TBD.

use crate::installer::{self, Pin};
use std::path::Path;

pub const INSTALLER_URL: &str =
    "https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/ViGEmBus_1.22.0_x64_x86_arm64.exe";
pub const INSTALLER_SIZE: u64 = 6_278_576;
pub const INSTALLER_SHA256: &str = "89220a7865076b342892f98865f3499fb7c4cfd673159e89d352c360fd014c6a";
const INSTALLER_NAME: &str = "ViGEmBus_1.22.0_x64_x86_arm64.exe";

const PIN: Pin = Pin { url: INSTALLER_URL, name: INSTALLER_NAME, size: INSTALLER_SIZE, sha256: INSTALLER_SHA256 };

/// Check `path` against the pin (size first, then sha256). On a mismatch the
/// file is deleted and the reason returned.
pub fn verify_installer(path: &Path) -> Result<(), String> {
    installer::verify(&PIN, path)
}

/// `"installed"` (the bus answers), `"missing"`, or `"unsupported"` (not
/// Windows). Connects and disconnects; plugs nothing.
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
        Err("ViGEmBus is Windows only".into())
    }
}

#[cfg(windows)]
mod imp {
    pub fn status() -> &'static str {
        match vigem_client::Client::connect() {
            Ok(_) => "installed",
            Err(_) => "missing",
        }
    }

    pub fn install() -> Result<(), String> {
        crate::installer::download_verify_run(&super::PIN)
    }
}
