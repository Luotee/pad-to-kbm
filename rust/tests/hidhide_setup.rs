//! v3.6.0 HidHide setup: the installer is the pinned official file, checked
//! by size and sha256 before anything runs; a bad file is deleted.

use dualsense_mapper::hidhide_setup::{verify_installer, INSTALLER_SHA256, INSTALLER_SIZE, INSTALLER_URL};
use std::fs;

#[test]
fn pin_matches_the_official_release() {
    assert_eq!(
        INSTALLER_URL,
        "https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"
    );
    assert_eq!(INSTALLER_SIZE, 8_078_016);
    assert_eq!(INSTALLER_SHA256, "f4bbbcb82e6258641b887c74bc81c4c5f66e4aa811808dfc304347687b7605f6");
}

#[test]
fn installer_rejects_bad_sha_and_size() {
    let dir = tempfile::tempdir().unwrap();
    // Wrong size.
    let small = dir.path().join("small.exe");
    fs::write(&small, b"MZ not the installer").unwrap();
    let err = verify_installer(&small).unwrap_err();
    assert!(err.contains("size"), "{err}");
    assert!(!small.exists(), "wrong-size file deleted");
    // Right size, wrong bytes.
    let same = dir.path().join("same.exe");
    fs::write(&same, vec![0u8; INSTALLER_SIZE as usize]).unwrap();
    let err = verify_installer(&same).unwrap_err();
    assert!(err.contains("sha256"), "{err}");
    assert!(!same.exists(), "wrong-hash file deleted");
    // Missing file.
    assert!(verify_installer(&dir.path().join("none.exe")).is_err());
}

#[cfg(not(windows))]
#[test]
fn status_unsupported_off_windows() {
    assert_eq!(dualsense_mapper::hidhide_setup::status(), "unsupported");
    assert!(dualsense_mapper::hidhide_setup::install().is_err());
}

/// This machine has no HidHide: the probe says so (read-only).
#[cfg(windows)]
#[test]
fn status_is_a_known_value_on_windows() {
    let s = dualsense_mapper::hidhide_setup::status();
    println!("hidhide_setup::status() = {s}");
    assert!(s == "installed" || s == "missing", "{s}");
}

/// Downloads the real installer once and checks the pin. Network; run by
/// hand: `cargo test --test hidhide_setup -- --ignored`.
#[test]
#[ignore]
fn installer_accepts_pinned_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("HidHide.exe");
    let ok = std::process::Command::new("curl")
        .args(["-sSfL", "-o"])
        .arg(&path)
        .arg(INSTALLER_URL)
        .status()
        .expect("curl")
        .success();
    assert!(ok, "download failed");
    verify_installer(&path).expect("pinned installer verifies");
    assert!(path.exists());
}
