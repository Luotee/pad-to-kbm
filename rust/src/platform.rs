//! Platform-specific controller power control.
//!
//! The DualSense HID protocol has **no** power-off / shutdown / disconnect
//! command. Verified against the nondebug/dualsense reverse-engineering
//! reference: every output report (0x02/0x05/0x31/0xa0/…) only drives
//! haptics, adaptive triggers, the light bar, the speaker, and the player
//! LEDs. The PS5 console powers a pad off at the **Bluetooth link layer**
//! (it terminates the ACL link), not via HID — the pad, having lost its
//! bonded host, then powers itself down.
//!
//! So "turn off the controller" from a PC means dropping the Bluetooth
//! *link* through the OS Bluetooth stack — **without** removing the
//! pairing. The right primitive is the `IOCTL_BTH_DISCONNECT_DEVICE`
//! control code (bthioctl.h) sent to the local Bluetooth radio: it tears
//! down the ACL link to one remote device but leaves the bond intact, so
//! the pad reconnects on the next PS-button press with no re-pairing. This
//! mirrors what the PS5 console does.
//!
//! (v2.3.0 wrongly used `BluetoothRemoveDevice`, which *unpairs* the pad —
//! far more destructive: the user had to re-pair every time. v2.3.1
//! replaced it with the link-level disconnect below.)
//!
//! This is user-mode Bluetooth *connection management* — not input
//! synthesis, not a driver, not process hooking. It stays within iron
//! rule #7.
//!
//! The same link-level disconnect serves the Switch Pro Controller (matched
//! by its "Pro Controller" Bluetooth name); see [`power_off_connected`].

/// `IOCTL_BTH_DISCONNECT_DEVICE` from bthioctl.h:
/// `CTL_CODE(FILE_DEVICE_BLUETOOTH=0x41, function=0x03, METHOD_BUFFERED=0,
/// FILE_ANY_ACCESS=0)` = `(0x41 << 16) | (0x03 << 2)` = `0x41000C`. Not
/// re-exported by windows-sys, so defined here.
#[cfg(target_os = "windows")]
const IOCTL_BTH_DISCONNECT_DEVICE: u32 = 0x0041_000C;

use crate::gamepad::ControllerKind;

/// Disconnect every currently-connected pad of `kind` at the Bluetooth-link
/// layer, keeping each pad's pairing intact. The pad, having lost its host
/// link, powers itself off; pressing PS reconnects it with no re-pairing.
/// Returns the number of controllers a disconnect was successfully issued
/// for.
///
/// USB note: this app only ever streams the Bluetooth 0x31 report, so a
/// connected pad is on Bluetooth by construction. A USB-only pad cannot be
/// powered off in software (it is bus-powered) and simply won't match.
#[cfg(target_os = "windows")]
pub fn power_off_connected(kind: ControllerKind) -> anyhow::Result<usize> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Devices::Bluetooth::{
        BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindFirstRadio,
        BluetoothFindNextDevice, BluetoothFindNextRadio, BluetoothFindRadioClose,
        BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let matches: fn(&str) -> bool = match kind {
        ControllerKind::DualSense => is_dualsense_name,
        ControllerKind::NsPro => is_ns_pro_name,
        // PR 3 handles the NS2 Pro (BLE); there is no classic-BT link to drop.
        ControllerKind::Ns2Pro => return Ok(0),
        // Nano is BLE HID-over-GATT; no classic-BT link to drop.
        ControllerKind::XboxNano => return Ok(0),
    };

    // SAFETY: every struct is zero-initialised then has its `dwSize` set
    // before the FFI call, exactly as the Win32 Bluetooth API requires.
    unsafe {
        // ── 1. Collect the BTH_ADDR (u64) of every connected DualSense ──
        let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
        params.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        params.fReturnAuthenticated = 1; // paired
        params.fReturnRemembered = 1; // previously paired
        params.fReturnConnected = 1; // currently linked
        params.fReturnUnknown = 0;
        params.fIssueInquiry = 0; // don't scan the air — only known devices
        params.cTimeoutMultiplier = 0;
        params.hRadio = std::ptr::null_mut(); // search across all radios

        let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

        let find = BluetoothFindFirstDevice(&params, &mut info);
        let mut targets: Vec<u64> = Vec::new();
        if !find.is_null() {
            loop {
                if info.fConnected != 0 && matches(&decode_name(&info.szName)) {
                    // BLUETOOTH_ADDRESS is a union; ullLong is the 48-bit
                    // device address packed into a u64 — the BTH_ADDR the
                    // disconnect IOCTL expects.
                    targets.push(info.Address.Anonymous.ullLong);
                }
                info = zeroed();
                info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
                if BluetoothFindNextDevice(find, &mut info) == 0 {
                    break;
                }
            }
            BluetoothFindDeviceClose(find);
        }

        if targets.is_empty() {
            return Ok(0);
        }

        // ── 2. Send IOCTL_BTH_DISCONNECT_DEVICE on each local radio ─────
        // A device belongs to exactly one radio; issuing the IOCTL on the
        // wrong radio just fails harmlessly, so we try every radio and
        // count an address as done on its first success.
        let mut done = std::collections::HashSet::new();
        let mut rparams: BLUETOOTH_FIND_RADIO_PARAMS = zeroed();
        rparams.dwSize = size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
        let mut radio: HANDLE = std::ptr::null_mut();
        let rfind = BluetoothFindFirstRadio(&rparams, &mut radio);
        if !rfind.is_null() {
            loop {
                for &addr in &targets {
                    let mut returned = 0u32;
                    let ok = DeviceIoControl(
                        radio,
                        IOCTL_BTH_DISCONNECT_DEVICE,
                        &addr as *const u64 as *const core::ffi::c_void,
                        size_of::<u64>() as u32,
                        std::ptr::null_mut(),
                        0,
                        &mut returned,
                        std::ptr::null_mut(),
                    );
                    if ok != 0 {
                        done.insert(addr);
                        tracing::info!("disconnected {} Bluetooth link (pairing kept)", kind.display_name());
                    }
                }
                CloseHandle(radio);
                radio = std::ptr::null_mut();
                if BluetoothFindNextRadio(rfind, &mut radio) == 0 {
                    break;
                }
            }
            BluetoothFindRadioClose(rfind);
        }

        if done.len() < targets.len() {
            tracing::warn!(
                wanted = targets.len(),
                done = done.len(),
                "some controller links could not be disconnected"
            );
        }
        Ok(done.len())
    }
}

/// macOS: not yet wired up. Phase 2 will route this through IOBluetooth
/// (`IOBluetoothDevice closeConnection`). Until then, fail loudly so the
/// GUI can surface "not supported yet" rather than silently no-op.
#[cfg(target_os = "macos")]
pub fn power_off_connected(_kind: ControllerKind) -> anyhow::Result<usize> {
    anyhow::bail!("controller power-off is not yet implemented on macOS (Phase 2 TBD)")
}

/// Other platforms (the Linux dev host): no Bluetooth management path.
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn power_off_connected(_kind: ControllerKind) -> anyhow::Result<usize> {
    anyhow::bail!("controller power-off is only supported on Windows")
}

/// Decode a Win32 `szName` (UTF-16, NUL-terminated, fixed-size buffer) into
/// a Rust `String`, stopping at the first NUL.
#[cfg(target_os = "windows")]
fn decode_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// True when a Bluetooth device name belongs to a DualSense pad. Windows
/// reports the DualSense as "DualSense Wireless Controller"; some stacks
/// shorten it to "Wireless Controller". Matched case-insensitively so a
/// stack that title-cases differently still hits.
///
/// Kept as a free function so it is unit-testable without any Bluetooth
/// hardware or Win32 calls.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_dualsense_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("dualsense") || n.contains("wireless controller")
}

/// True when a Bluetooth device name belongs to a Switch Pro Controller
/// ("Pro Controller", or "Nintendo Switch Pro Controller" on some stacks).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_ns_pro_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("pro controller")
}

#[cfg(test)]
mod tests {
    use super::{is_dualsense_name, is_ns_pro_name};

    #[test]
    fn matches_ns_pro_names() {
        assert!(is_ns_pro_name("Pro Controller"));
        assert!(is_ns_pro_name("Nintendo Switch Pro Controller"));
        assert!(is_ns_pro_name("PRO CONTROLLER"));
        assert!(!is_ns_pro_name("DualSense Wireless Controller"));
        assert!(!is_ns_pro_name("Wireless Controller"));
        assert!(!is_ns_pro_name(""));
    }

    #[test]
    fn dualsense_names_unchanged_vs_ns() {
        assert!(!is_dualsense_name("Pro Controller"));
    }

    #[test]
    fn matches_known_dualsense_names() {
        assert!(is_dualsense_name("DualSense Wireless Controller"));
        assert!(is_dualsense_name("Wireless Controller"));
        assert!(is_dualsense_name("dualsense edge"));
        assert!(is_dualsense_name("WIRELESS CONTROLLER"));
    }

    #[test]
    fn rejects_unrelated_names() {
        assert!(!is_dualsense_name("My Headphones"));
        assert!(!is_dualsense_name("Xbox Wireless"));
        assert!(!is_dualsense_name(""));
        assert!(!is_dualsense_name("Logitech Mouse"));
    }
}
