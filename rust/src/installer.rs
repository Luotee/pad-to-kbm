//! Download, verify and run a **pinned official installer** (v3.6.0, shared
//! by `vigem_setup` and `hidhide_setup`).
//!
//! The app never ships a driver (iron rule 7). On the user's click it
//! downloads the installer named by a [`Pin`] into `%TEMP%`, checks its size
//! and sha256 against the pin, and only then runs it; the installer asks for
//! elevation itself. A file that fails the check is deleted and nothing runs.
//! The downloaded file is deleted again once the installer exits.

use sha2::{Digest, Sha256};
use std::path::Path;

/// One pinned installer: where it comes from, the file name it is saved
/// under in `%TEMP%`, and the exact size + sha256 it must have.
#[derive(Debug, Clone, Copy)]
pub struct Pin {
    pub url: &'static str,
    pub name: &'static str,
    pub size: u64,
    /// Lower-case hex.
    pub sha256: &'static str,
}

/// Check `path` against `pin` (size first, then sha256). On a mismatch the
/// file is deleted and the reason returned.
pub fn verify(pin: &Pin, path: &Path) -> Result<(), String> {
    let reject = |why: String| {
        let _ = std::fs::remove_file(path);
        Err(why)
    };
    let bytes = std::fs::read(path).map_err(|e| format!("installer not readable: {e}"))?;
    if bytes.len() as u64 != pin.size {
        return reject(format!("installer size {} ≠ {}", bytes.len(), pin.size));
    }
    let hex: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if hex != pin.sha256 {
        return reject(format!("installer sha256 {hex} ≠ pinned"));
    }
    Ok(())
}

/// Download `pin.url` to `%TEMP%\<pin.name>`, [`verify`] it, run it and wait
/// for it to exit, then delete it. Blocking: call it off the UI thread.
#[cfg(windows)]
pub fn download_verify_run(pin: &Pin) -> Result<(), String> {
    imp::download_verify_run(pin)
}

#[cfg(windows)]
mod imp {
    use super::{verify, Pin};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Com::Urlmon::URLDownloadToFileW;
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn download_verify_run(pin: &Pin) -> Result<(), String> {
        let path = std::env::temp_dir().join(pin.name);
        let _ = std::fs::remove_file(&path);
        let url = wide(std::ffi::OsStr::new(pin.url));
        let file = wide(path.as_os_str());
        // SAFETY: plain Win32 calls with NUL-terminated wide strings that
        // outlive them; COM is initialised for this thread and released.
        let hr = unsafe {
            let co = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
            let hr = URLDownloadToFileW(std::ptr::null_mut(), url.as_ptr(), file.as_ptr(), 0, std::ptr::null_mut());
            if co >= 0 {
                CoUninitialize();
            }
            hr
        };
        if hr < 0 {
            let _ = std::fs::remove_file(&path);
            return Err(format!("下載失敗（0x{:08X}）", hr as u32));
        }
        verify(pin, &path)?;
        tracing::info!("running the verified installer {}", path.display());
        // SAFETY: SHELLEXECUTEINFOW is zero-initialisable; the strings live
        // until the call returns; the process handle is closed below.
        unsafe {
            let open = wide(std::ffi::OsStr::new("open"));
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = open.as_ptr();
            info.lpFile = file.as_ptr();
            info.nShow = SW_SHOWNORMAL;
            if ShellExecuteExW(&mut info) == 0 {
                return Err(format!("無法啟動安裝程式：{}", std::io::Error::last_os_error()));
            }
            if !info.hProcess.is_null() {
                WaitForSingleObject(info.hProcess, INFINITE);
                let mut code = 0u32;
                GetExitCodeProcess(info.hProcess, &mut code);
                CloseHandle(info.hProcess);
                tracing::info!(code, installer = pin.name, "installer exited");
            }
        }
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: Pin = Pin {
        url: "https://example.invalid/x.exe",
        name: "x.exe",
        size: 5,
        // sha256("hello")
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    };

    #[test]
    fn matching_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.exe");
        std::fs::write(&path, b"hello").unwrap();
        assert_eq!(verify(&PIN, &path), Ok(()));
        assert!(path.exists());
    }

    #[test]
    fn wrong_size_is_rejected_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("short.exe");
        std::fs::write(&path, b"hell").unwrap();
        let err = verify(&PIN, &path).unwrap_err();
        assert!(err.contains("size"), "{err}");
        assert!(!path.exists());
    }

    #[test]
    fn wrong_hash_is_rejected_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evil.exe");
        std::fs::write(&path, b"jello").unwrap();
        let err = verify(&PIN, &path).unwrap_err();
        assert!(err.contains("sha256"), "{err}");
        assert!(!path.exists());
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(verify(&PIN, &dir.path().join("none.exe")).is_err());
    }
}
