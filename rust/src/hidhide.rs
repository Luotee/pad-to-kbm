//! HidHide client (v3.6.0): hides the real DualSense / Switch Pro from games
//! while the app feeds them a virtual Xbox pad, so a game does not see the
//! same input twice.
//!
//! HidHide is an externally installed Nefarius filter driver (iron rule 7:
//! never bundled; `hidhide_setup.rs` runs its pinned official installer on
//! the user's click). This module only talks to its control device
//! `\\.\HidHide` with the driver's own IOCTLs, from user mode, without
//! admin rights. Facts below are from the HidHide v1.5.230 source (MIT,
//! `HidHide/src/Logic.{h,c}`, `Config.c`, `HidHideCLI/src/FilterDriverProxy.cpp`,
//! `Volume.cpp`):
//!
//! - one handle at a time (`WdfDeviceInitSetExclusive`); a second open fails
//!   with access denied ([`Error::Busy`]);
//! - lists (blacklisted device instance paths, whitelisted full image names)
//!   are REG_MULTI_SZ-style UTF-16: each string NUL-terminated, one more NUL
//!   at the end; compared case-insensitively by the driver;
//! - Active / WhitelistedInverse are one-byte BOOLEANs.
//!
//! The pure helpers (MULTI_SZ, VID/PID parsing, list merging, the state file)
//! build and are tested on every platform; the driver, the device
//! enumeration and the NT image path are Windows only and return
//! [`Error::Unsupported`] (or nothing) elsewhere. The hider worker that
//! drives all this is `hider.rs`.

use crate::file_names::HIDHIDE_STATE_FILE;
use crate::gamepad::controller::ControllerKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ─── IOCTL codes ─────────────────────────────────────────────────────────────

/// `CTL_CODE` from winioctl.h.
pub const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

/// HidHide's custom device type (Logic.h `IoControlDeviceType`).
const IO_CONTROL_DEVICE_TYPE: u32 = 32769;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_DATA: u32 = 1;

const fn hidhide_ioctl(function: u32) -> u32 {
    ctl_code(IO_CONTROL_DEVICE_TYPE, function, METHOD_BUFFERED, FILE_READ_DATA)
}

pub const IOCTL_GET_WHITELIST: u32 = hidhide_ioctl(2048);
pub const IOCTL_SET_WHITELIST: u32 = hidhide_ioctl(2049);
pub const IOCTL_GET_BLACKLIST: u32 = hidhide_ioctl(2050);
pub const IOCTL_SET_BLACKLIST: u32 = hidhide_ioctl(2051);
pub const IOCTL_GET_ACTIVE: u32 = hidhide_ioctl(2052);
pub const IOCTL_SET_ACTIVE: u32 = hidhide_ioctl(2053);
pub const IOCTL_GET_WLINVERSE: u32 = hidhide_ioctl(2054);
pub const IOCTL_SET_WLINVERSE: u32 = hidhide_ioctl(2055);

// ─── MULTI_SZ ────────────────────────────────────────────────────────────────

/// UTF-16 MULTI_SZ: every string NUL-terminated, then one more NUL. Empty
/// strings are skipped (they would end the list early). The empty list is
/// two NULs, the value the driver itself writes to clear a list.
pub fn encode_multi_sz(list: &[String]) -> Vec<u16> {
    let mut out = Vec::new();
    for item in list.iter().filter(|s| !s.is_empty()) {
        out.extend(item.encode_utf16());
        out.push(0);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    out
}

/// Inverse of [`encode_multi_sz`]. Stops at the first empty string: the
/// driver reports more room than it fills (it counts `UNICODE_STRING.Length`,
/// a byte count, as characters, `HidHideCollectionToMultiString` in
/// Config.c), so whatever follows the double NUL is slack. A last string
/// without its NUL is still taken.
pub fn decode_multi_sz(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .take_while(|part| !part.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

// ─── Device ids ──────────────────────────────────────────────────────────────

/// Vendor and product id from a device instance id, case-insensitive. Only
/// the device-id part (between the first and second backslash) is read.
///
/// - USB: `HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000`
/// - Bluetooth classic: `HID\{00001124-…}_VID&0002054C_PID&0CE6\9&…` (the
///   VID field carries the vendor-id source, `0002` = USB-IF, in front)
/// - Bluetooth LE: `HID\{00001812-…}_Dev_VID&02054C_PID&0CE6_REV&…\…`
pub fn parse_vid_pid(instance_id: &str) -> Option<(u16, u16)> {
    let device_id = instance_id.split('\\').nth(1).unwrap_or(instance_id).to_ascii_uppercase();
    let (vid_hex, rest) = hex_after_tag(&device_id, "VID")?;
    // The vendor id is the last four digits; longer fields carry a source prefix.
    if !matches!(vid_hex.len(), 4 | 6 | 8) {
        return None;
    }
    let (pid_hex, _) = hex_after_tag(rest, "PID")?;
    if pid_hex.len() != 4 {
        return None;
    }
    let vid = u16::from_str_radix(&vid_hex[vid_hex.len() - 4..], 16).ok()?;
    let pid = u16::from_str_radix(pid_hex, 16).ok()?;
    Some((vid, pid))
}

/// The run of hex digits after the first `<tag>_` or `<tag>&` in `s`, and
/// what follows it.
fn hex_after_tag<'a>(s: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let mut from = 0;
    while let Some(at) = s[from..].find(tag) {
        let start = from + at + tag.len();
        if matches!(s.as_bytes().get(start), Some(b'_') | Some(b'&')) {
            let digits = &s[start + 1..];
            let len = digits.bytes().take_while(u8::is_ascii_hexdigit).count();
            if len > 0 {
                return Some((&digits[..len], &digits[len..]));
            }
        }
        from = start;
    }
    None
}

/// The kind of pad a HID device is, if it is one the app would hide: a pad
/// the HID source drives (its own table, `hid_source::kind_for_vid_pid`)
/// that Windows has a native driver for. The Switch 2 Pro (0x057E / 0x2069)
/// is never hidden, and the DualSense Edge (0x0DF2) is not driven by the app.
pub fn kind_for(vid: u16, pid: u16) -> Option<ControllerKind> {
    crate::gamepad::hid_source::kind_for_vid_pid(vid, pid).filter(|k| k.has_native_driver())
}

/// `kind_for` a HID instance id, limited to the buses the HID source drives
/// (`hid_source::is_candidate`): a USB Switch Pro is never driven, so hiding
/// it would leave the game with no pad at all. Bluetooth HID ids carry the
/// HID (`00001124`) or HOGP (`00001812`) service UUID. The Xbox Nano lists
/// four collections under one VID / PID; only its gamepad collection
/// (`&Col01`, the one the HID source reads) is the Nano's to hide and
/// remember, in both id formats (`is_gamepad_collection`).
pub fn kind_for_instance(id: &str) -> Option<ControllerKind> {
    use crate::gamepad::hid_source::is_candidate;
    let (vid, pid) = parse_vid_pid(id)?;
    let bus = if is_bluetooth_instance(id) { hidapi::BusType::Bluetooth } else { hidapi::BusType::Usb };
    kind_for(vid, pid)
        .filter(|&k| is_candidate(vid, pid, bus) == Some(k))
        .filter(|&k| k != ControllerKind::XboxNano || is_gamepad_collection(id))
}

/// The device-id part of `id` (between the first and second backslash) has
/// a `&`-field `Col01`, any case: the Xbox Nano's gamepad collection. It
/// ends the device id without Microsoft's Xbox driver
/// (`…_REV&0609_<address>&Col01`) and is followed by `&IG_00` with it
/// (`…&REV_0619&<address>&COL01&IG_00`).
pub fn is_gamepad_collection(id: &str) -> bool {
    id.split('\\').nth(1).is_some_and(|d| d.split('&').any(|f| f.eq_ignore_ascii_case("col01")))
}

/// A Bluetooth HID instance id: it carries the HID (`00001124`) or HOGP
/// (`00001812`) service UUID. Anything else is USB.
pub fn is_bluetooth_instance(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    lower.contains("{00001124-") || lower.contains("{00001812-")
}

// ─── Bluetooth address ───────────────────────────────────────────────────────

/// The Bluetooth address in a Bluetooth HID device's parent instance id, as
/// 12 upper-case hex digits (`7C66EF8223D2`). The parent of a classic
/// Bluetooth HID pad is its BTHENUM device,
/// `BTHENUM\{00001124-…}_VID&0002054c_PID&0ce6\6&a71703f&0&7C66EF8223D2_C00000000`:
/// the address is the last `&` field up to its `_`. The parent of a Bluetooth
/// LE (HID-over-GATT) pad, the Xbox Nano, is its BTHLEDEVICE service device,
/// `BTHLEDEVICE\{00001812-…}_Dev_VID&02045e_PID&0b4f_REV&0609_FB3A7C7DF9A8\8&…`:
/// the address is the 12-hex-digit field of the middle part
/// (`ble_addr_in`). Anything else (a USB parent, `USB\VID_…\…`) has none.
pub fn bt_addr_from_parent(parent: &str) -> Option<String> {
    if parent.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("BTHENUM\\")) {
        let addr = parent.rsplit('&').next()?.split('_').next()?.to_ascii_uppercase();
        is_bt_addr(&addr).then_some(addr)
    } else if parent.get(..12).is_some_and(|p| p.eq_ignore_ascii_case("BTHLEDEVICE\\")) {
        ble_addr_in(parent.split('\\').nth(1)?)
    } else {
        None
    }
}

/// The Bluetooth address in a Bluetooth LE (HID-over-GATT) HID instance id
/// itself, the Xbox Nano's: the 12-hex-digit field of its device id, in
/// both formats, `…_REV&0609_FB3A7C7DF9A8&Col01` and (with Microsoft's Xbox
/// driver) `…&REV_0619&FB3A7C7DF9A8&COL01&IG_00`. Classic Bluetooth and USB
/// ids carry none (`bt_addr_from_parent` reads the parent's).
pub fn bt_addr_from_instance(id: &str) -> Option<String> {
    if !is_ble_instance(id) {
        return None;
    }
    ble_addr_in(id.split('\\').nth(1)?)
}

/// The Bluetooth address in a Bluetooth LE HID device's hidapi path
/// (`\\?\HID#{00001812-…}…&fb3a7c7df9a8&Col01#8&…#{…}`): the path is the
/// instance id with `#` for `\` (`bt_addr_from_instance`). The Xbox Nano's
/// GATT battery read takes it (`gamepad::nano_battery`).
pub fn bt_addr_from_hid_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\").unwrap_or(path);
    bt_addr_from_instance(&rest.replace('#', "\\"))
}

/// The first field of a Bluetooth LE device id, split at `&` and `_`, that
/// is 12 hex digits (the service UUID in braces is one field and never
/// matches), upper-cased.
fn ble_addr_in(device_id: &str) -> Option<String> {
    device_id
        .split(['&', '_'])
        .find(|f| f.len() == 12 && f.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|f| f.to_ascii_uppercase())
}

/// `id` is a Bluetooth LE (HID-over-GATT) HID device, the Xbox Nano or the
/// Switch 2 Pro. Its link is not one the automatic power-off drops
/// (`crate::platform::power_off_addrs` sends the classic-Bluetooth
/// disconnect), so it gets the reconnect hint instead.
pub fn is_ble_instance(id: &str) -> bool {
    id.to_ascii_lowercase().contains("{00001812-")
}

/// `addr` is a Bluetooth address as stored (`KnownPad::addr`): 12
/// upper-case hex digits, no separators.
pub fn is_bt_addr(addr: &str) -> bool {
    addr.len() == 12 && addr.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

/// The `BTH_ADDR` for a 12-hex-digit address (either case), the value
/// `IOCTL_BTH_DISCONNECT_DEVICE` takes (`crate::platform::power_off_addrs`).
pub fn bt_addr_value(addr: &str) -> Option<u64> {
    if addr.len() != 12 || !addr.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(addr, 16).ok()
}

// ─── List merging ────────────────────────────────────────────────────────────

fn contains_ci(list: &[String], item: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(item))
}

/// `list` with `ours` appended. Every entry already in `list` stays, in
/// order (foreign entries are the user's); an entry of `ours` already present
/// (case-insensitive, as the driver compares) is not added again.
pub fn merge(list: &[String], ours: &[String]) -> Vec<String> {
    let mut out = list.to_vec();
    for item in ours {
        if !contains_ci(&out, item) {
            out.push(item.clone());
        }
    }
    out
}

/// `list` without the entries of `ours` (case-insensitive). Everything else
/// stays, in order.
pub fn unmerge(list: &[String], ours: &[String]) -> Vec<String> {
    list.iter().filter(|x| !contains_ci(ours, x)).cloned().collect()
}

// ─── State file ──────────────────────────────────────────────────────────────

/// What the app changed in HidHide, persisted next to the config
/// (`padtokbm.hidhide.json`) so a crash can be undone at the next start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HideState {
    /// Blacklist entries we added.
    #[serde(default)]
    pub ids: Vec<String>,
    /// We turned Active on (and turn it off again once our entries are gone).
    #[serde(default)]
    pub activated_by_us: bool,
    /// The NT image path we added to the whitelist (`None`: we added none,
    /// it was there already, or the whitelist is inverted). Removed again
    /// when Xbox output ends. Missing in state files written before v3.6.0
    /// was finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whitelisted: Option<String>,
}

impl HideState {
    /// Nothing to undo.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && !self.activated_by_us && self.whitelisted.is_none()
    }

    /// The state file beside `config_path`.
    pub fn path_beside(config_path: &Path) -> PathBuf {
        config_path.with_file_name(HIDHIDE_STATE_FILE)
    }

    /// The saved state; a missing or unreadable file is the empty state.
    pub fn load(path: &Path) -> HideState {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring unreadable {}: {e}", path.display());
                HideState::default()
            }),
            Err(_) => HideState::default(),
        }
    }

    /// Write the state atomically (temp file + rename in the same
    /// directory); an empty state deletes the file instead.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let body = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    }
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Error {
    /// No control device: HidHide is not installed, or its driver is not
    /// loaded yet (it loads with the HID stacks it filters).
    Absent,
    /// The control device is open elsewhere (HidHide's own client or CLI):
    /// it admits one handle at a time. Retry later.
    Busy,
    /// Not Windows.
    Unsupported,
    /// Any other OS error.
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Absent => f.write_str("HidHide control device not present"),
            Error::Busy => f.write_str("HidHide control device is in use"),
            Error::Unsupported => f.write_str("HidHide is Windows only"),
            Error::Io(e) => write!(f, "HidHide: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// What the hider needs from HidHide; [`Driver`] is the real one, tests
/// supply a fake.
pub trait Control {
    fn blacklist(&self) -> Result<Vec<String>, Error>;
    fn set_blacklist(&self, list: &[String]) -> Result<(), Error>;
    fn whitelist(&self) -> Result<Vec<String>, Error>;
    fn set_whitelist(&self, list: &[String]) -> Result<(), Error>;
    fn active(&self) -> Result<bool, Error>;
    fn set_active(&self, on: bool) -> Result<(), Error>;
    /// WhitelistedInverse: the whitelist is a list of apps that may NOT see
    /// hidden devices. The app never changes it.
    fn inverse(&self) -> Result<bool, Error>;
}

pub use imp::{bt_addr_of_instance, own_image_nt_path, present_hid_instance_ids, Driver};

// ─── Not Windows ─────────────────────────────────────────────────────────────

#[cfg(not(windows))]
mod imp {
    use super::{Control, Error};

    /// The HidHide control device. Never opens off Windows.
    pub struct Driver {
        never: std::convert::Infallible,
    }

    impl Driver {
        pub fn open() -> Result<Driver, Error> {
            Err(Error::Unsupported)
        }
    }

    impl Control for Driver {
        fn blacklist(&self) -> Result<Vec<String>, Error> {
            match self.never {}
        }
        fn set_blacklist(&self, _: &[String]) -> Result<(), Error> {
            match self.never {}
        }
        fn whitelist(&self) -> Result<Vec<String>, Error> {
            match self.never {}
        }
        fn set_whitelist(&self, _: &[String]) -> Result<(), Error> {
            match self.never {}
        }
        fn active(&self) -> Result<bool, Error> {
            match self.never {}
        }
        fn set_active(&self, _: bool) -> Result<(), Error> {
            match self.never {}
        }
        fn inverse(&self) -> Result<bool, Error> {
            match self.never {}
        }
    }

    pub fn present_hid_instance_ids() -> Vec<String> {
        Vec::new()
    }

    pub fn own_image_nt_path() -> Result<String, Error> {
        Err(Error::Unsupported)
    }

    /// No device tree off Windows.
    pub fn bt_addr_of_instance(_id: &str) -> Option<String> {
        None
    }
}

// ─── Windows ─────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::{decode_multi_sz, encode_multi_sz, Control, Error};
    use super::{
        IOCTL_GET_ACTIVE, IOCTL_GET_BLACKLIST, IOCTL_GET_WHITELIST, IOCTL_GET_WLINVERSE, IOCTL_SET_ACTIVE,
        IOCTL_SET_BLACKLIST, IOCTL_SET_WHITELIST,
    };
    use std::ffi::c_void;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_HID;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
        ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, QueryDosDeviceW, FILE_ATTRIBUTE_NORMAL,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> Error {
        Error::Io(std::io::Error::last_os_error())
    }

    /// An open handle to `\\.\HidHide`; closed on drop. Hold it only for one
    /// read-modify-write round: while it is open, HidHide's own client and
    /// CLI cannot open the device.
    pub struct Driver {
        handle: HANDLE,
    }

    // SAFETY: a kernel file handle may be used from any thread.
    unsafe impl Send for Driver {}

    impl Driver {
        /// Open the control device. [`Error::Absent`] when it does not exist,
        /// [`Error::Busy`] when another process holds it.
        pub fn open() -> Result<Driver, Error> {
            let name = wide(r"\\.\HidHide");
            // SAFETY: NUL-terminated name outliving the call; the handle is
            // owned by the returned Driver.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                // SAFETY: plain call.
                return Err(match unsafe { GetLastError() } {
                    ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Error::Absent,
                    ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION => Error::Busy,
                    code => Error::Io(std::io::Error::from_raw_os_error(code as i32)),
                });
            }
            Ok(Driver { handle })
        }

        /// One DeviceIoControl; returns the bytes the driver reported.
        fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, Error> {
            let mut returned = 0u32;
            let in_ptr = if input.is_empty() { std::ptr::null() } else { input.as_ptr() as *const c_void };
            let out_ptr = if output.is_empty() { std::ptr::null_mut() } else { output.as_mut_ptr() as *mut c_void };
            // SAFETY: the buffers are valid for the lengths passed; the
            // handle is open for the life of self; synchronous call.
            let ok = unsafe {
                DeviceIoControl(
                    self.handle,
                    code,
                    in_ptr,
                    input.len() as u32,
                    out_ptr,
                    output.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_error());
            }
            Ok(returned as usize)
        }

        /// GET list: ask for the size with no buffer, then fetch
        /// (FilterDriverProxy.cpp does the same).
        fn get_list(&self, code: u32) -> Result<Vec<String>, Error> {
            let needed = self.ioctl(code, &[], &mut [])?;
            if needed == 0 {
                return Ok(Vec::new());
            }
            let mut buf = vec![0u8; needed.next_multiple_of(2)];
            let got = self.ioctl(code, &[], &mut buf)?.min(buf.len());
            let wide: Vec<u16> = buf[..got].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Ok(decode_multi_sz(&wide))
        }

        fn set_list(&self, code: u32, list: &[String]) -> Result<(), Error> {
            let bytes: Vec<u8> = encode_multi_sz(list).iter().flat_map(|c| c.to_le_bytes()).collect();
            self.ioctl(code, &bytes, &mut [])?;
            Ok(())
        }

        fn get_bool(&self, code: u32) -> Result<bool, Error> {
            let mut b = [0u8; 1];
            let got = self.ioctl(code, &[], &mut b)?;
            if got != 1 {
                return Err(Error::Io(std::io::Error::other(format!("HidHide returned {got} bytes for a BOOLEAN"))));
            }
            Ok(b[0] != 0)
        }
    }

    impl Control for Driver {
        fn blacklist(&self) -> Result<Vec<String>, Error> {
            self.get_list(IOCTL_GET_BLACKLIST)
        }
        fn set_blacklist(&self, list: &[String]) -> Result<(), Error> {
            self.set_list(IOCTL_SET_BLACKLIST, list)
        }
        fn whitelist(&self) -> Result<Vec<String>, Error> {
            self.get_list(IOCTL_GET_WHITELIST)
        }
        fn set_whitelist(&self, list: &[String]) -> Result<(), Error> {
            self.set_list(IOCTL_SET_WHITELIST, list)
        }
        fn active(&self) -> Result<bool, Error> {
            self.get_bool(IOCTL_GET_ACTIVE)
        }
        fn set_active(&self, on: bool) -> Result<(), Error> {
            self.ioctl(IOCTL_SET_ACTIVE, &[on as u8], &mut [])?;
            Ok(())
        }
        fn inverse(&self) -> Result<bool, Error> {
            self.get_bool(IOCTL_GET_WLINVERSE)
        }
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateFileW and is closed once.
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Device instance ids of every present device with a HID interface
    /// (`HID\VID_…\…`, `HID\{…}_VID&…\…`), the form HidHide blacklists.
    /// Read-only; empty on any failure.
    pub fn present_hid_instance_ids() -> Vec<String> {
        let mut out = Vec::new();
        // SAFETY: SetupDi calls on a list we create and destroy here; the
        // id buffer is sized by the API's own required-size answer.
        unsafe {
            let set = SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_HID,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            );
            if set == INVALID_HANDLE_VALUE as isize {
                tracing::warn!("SetupDiGetClassDevsW(HID): {}", std::io::Error::last_os_error());
                return out;
            }
            let mut index = 0u32;
            loop {
                let mut info: SP_DEVINFO_DATA = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(set, index, &mut info) == 0 {
                    break; // ERROR_NO_MORE_ITEMS (or a failure: stop either way)
                }
                index += 1;
                let mut buf = vec![0u16; 256];
                let mut need = 0u32;
                let mut ok = SetupDiGetDeviceInstanceIdW(set, &info, buf.as_mut_ptr(), buf.len() as u32, &mut need);
                if ok == 0 && GetLastError() == ERROR_INSUFFICIENT_BUFFER && need as usize > buf.len() {
                    buf = vec![0u16; need as usize];
                    ok = SetupDiGetDeviceInstanceIdW(set, &info, buf.as_mut_ptr(), buf.len() as u32, &mut need);
                }
                if ok != 0 {
                    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    out.push(String::from_utf16_lossy(&buf[..len]));
                }
            }
            SetupDiDestroyDeviceInfoList(set);
        }
        out
    }

    /// The Bluetooth address of the pad whose HID device instance id is `id`
    /// ([`super::bt_addr_from_parent`] of its parent's id), present or not:
    /// the devnode of a pad that is off stays. A Bluetooth LE id carries the
    /// address itself ([`super::bt_addr_from_instance`]), read when the
    /// parent gives none. Read-only cfgmgr32 calls, no admin rights. `None`:
    /// not Bluetooth, or a classic Bluetooth devnode that is gone.
    pub fn bt_addr_of_instance(id: &str) -> Option<String> {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
            CM_Get_Device_IDW, CM_Get_Parent, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_PHANTOM, CR_SUCCESS,
            MAX_DEVICE_ID_LEN,
        };
        let wid = wide(id);
        let (mut node, mut parent) = (0u32, 0u32);
        let mut buf = vec![0u16; MAX_DEVICE_ID_LEN as usize + 1];
        // SAFETY: a NUL-terminated id in; out-params are plain u32s; the id
        // buffer holds MAX_DEVICE_ID_LEN characters plus the NUL.
        unsafe {
            if CM_Locate_DevNodeW(&mut node, wid.as_ptr(), CM_LOCATE_DEVNODE_PHANTOM) != CR_SUCCESS
                || CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS
                || CM_Get_Device_IDW(parent, buf.as_mut_ptr(), buf.len() as u32, 0) != CR_SUCCESS
            {
                return super::bt_addr_from_instance(id);
            }
        }
        // The parent's id; a Bluetooth LE id also carries the address itself.
        super::bt_addr_from_parent(&from_wide(&buf)).or_else(|| super::bt_addr_from_instance(id))
    }

    fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    /// The NT "full image name" of this exe, the form the driver records for
    /// a process (`PsSetLoadImageNotifyRoutine`'s FullImageName) and matches
    /// the whitelist against, e.g. `\Device\HarddiskVolume3\Games\PadToKBM.exe`.
    ///
    /// Same conversion as HidHide's CLI (`FileNameToFullImageName`,
    /// Volume.cpp): volume mount point of the path → volume GUID name →
    /// `QueryDosDeviceW` of `Volume{…}` → that device name + the rest of the
    /// path. This also covers volumes mounted on a folder. A UNC path
    /// (`\\server\share\…`) maps to `\Device\Mup\server\share\…`.
    pub fn own_image_nt_path() -> Result<String, Error> {
        let exe = std::env::current_exe().map_err(Error::Io)?;
        dos_to_nt_path(&exe.to_string_lossy())
    }

    fn dos_to_nt_path(path: &str) -> Result<String, Error> {
        let path = path.strip_prefix(r"\\?\UNC\").map(|p| format!(r"\\{p}")).unwrap_or_else(|| path.to_string());
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path).to_string();
        if let Some(unc) = path.strip_prefix(r"\\") {
            return Ok(format!(r"\Device\Mup\{unc}"));
        }
        let wpath = wide(&path);
        let mut mount = vec![0u16; 1024];
        let mut volume = vec![0u16; 64];
        let mut device = vec![0u16; 1024];
        // SAFETY: NUL-terminated inputs; output buffers sized as passed.
        unsafe {
            if GetVolumePathNameW(wpath.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) == 0 {
                return Err(last_error());
            }
            let mount_point = from_wide(&mount); // e.g. `C:\`
            if GetVolumeNameForVolumeMountPointW(wide(&mount_point).as_ptr(), volume.as_mut_ptr(), volume.len() as u32)
                == 0
            {
                return Err(last_error());
            }
            // `\\?\Volume{…}\` → `Volume{…}`
            let volume_name = from_wide(&volume);
            let dos_name = volume_name.trim_start_matches(r"\\?\").trim_end_matches('\\');
            if QueryDosDeviceW(wide(dos_name).as_ptr(), device.as_mut_ptr(), device.len() as u32) == 0 {
                return Err(last_error());
            }
            let device_name = from_wide(&device); // `\Device\HarddiskVolume3`
            let rest = path.get(mount_point.len()..).unwrap_or("");
            Ok(format!(r"{device_name}\{rest}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn w(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    // ── MULTI_SZ ──

    #[test]
    fn encode_terminates_each_string_and_the_list() {
        assert_eq!(encode_multi_sz(&s(&["ab", "c"])), w("ab\0c\0\0"));
    }

    #[test]
    fn encode_empty_list_is_two_nuls_like_the_driver_clear() {
        // Logic.c SET handlers clear a list with SetXxx(L"\0\0", 2).
        assert_eq!(encode_multi_sz(&[]), vec![0, 0]);
    }

    #[test]
    fn encode_skips_empty_strings_which_would_end_the_list() {
        assert_eq!(encode_multi_sz(&s(&["a", "", "b"])), w("a\0b\0\0"));
        assert_eq!(encode_multi_sz(&s(&[""])), vec![0, 0]);
    }

    #[test]
    fn encode_keeps_non_ascii() {
        let list = s(&[r"\Device\HarddiskVolume3\遊戲\PadToKBM.exe"]);
        assert_eq!(decode_multi_sz(&encode_multi_sz(&list)), list);
    }

    #[test]
    fn decode_round_trips() {
        let list = s(&[r"HID\VID_054C&PID_0CE6\1", r"HID\X\2"]);
        assert_eq!(decode_multi_sz(&encode_multi_sz(&list)), list);
    }

    #[test]
    fn decode_empty_forms() {
        assert_eq!(decode_multi_sz(&[]), Vec::<String>::new());
        // The driver's GET on an empty list returns one NUL.
        assert_eq!(decode_multi_sz(&[0]), Vec::<String>::new());
        assert_eq!(decode_multi_sz(&[0, 0]), Vec::<String>::new());
    }

    #[test]
    fn decode_stops_at_the_list_terminator() {
        // The driver over-reports the size it needs (it adds
        // UNICODE_STRING.Length, a byte count, as characters), so the buffer
        // carries slack after the double NUL that must be ignored.
        let mut buf = w("a\0b\0\0");
        buf.extend(w("junk\0more\0"));
        assert_eq!(decode_multi_sz(&buf), s(&["a", "b"]));
    }

    #[test]
    fn decode_tolerates_a_missing_final_terminator() {
        assert_eq!(decode_multi_sz(&w("a\0b")), s(&["a", "b"]));
        assert_eq!(decode_multi_sz(&w("a\0b\0")), s(&["a", "b"]));
    }

    // ── VID / PID ──

    #[test]
    fn parses_usb_hid_instance_id() {
        assert_eq!(parse_vid_pid(r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000"), Some((0x054C, 0x0CE6)));
        assert_eq!(parse_vid_pid(r"HID\VID_057E&PID_2009\7&3&0&0000"), Some((0x057E, 0x2009)));
    }

    #[test]
    fn parses_usb_parent_and_collection_suffixes() {
        assert_eq!(parse_vid_pid(r"USB\VID_054C&PID_0CE6&MI_03\6&2c&0&0003"), Some((0x054C, 0x0CE6)));
        assert_eq!(parse_vid_pid(r"HID\VID_046D&PID_C52B&MI_01&COL02\7&abc&0&0001"), Some((0x046D, 0xC52B)));
    }

    #[test]
    fn parses_bluetooth_classic_hid_instance_id() {
        assert_eq!(
            parse_vid_pid(r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000"),
            Some((0x054C, 0x0CE6))
        );
        assert_eq!(
            parse_vid_pid(r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\9&1&0&0000"),
            Some((0x057E, 0x2009))
        );
    }

    #[test]
    fn parses_bluetooth_le_hid_instance_id() {
        assert_eq!(
            parse_vid_pid(r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b13_REV&0509_c8f7&Col01\9&1&0&0000"),
            Some((0x045E, 0x0B13))
        );
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(parse_vid_pid(r"hid\vid_054c&pid_0ce6&mi_03\7&1&0&0000"), Some((0x054C, 0x0CE6)));
    }

    #[test]
    fn parse_rejects_ids_without_vid_pid() {
        assert_eq!(parse_vid_pid(r"HID\ConvertedDevice&Col01\5&1&0&0000"), None);
        assert_eq!(parse_vid_pid(r"ROOT\HidHide\0000"), None);
        assert_eq!(parse_vid_pid(r"HID\VID_054C\1"), None);
        assert_eq!(parse_vid_pid(r"HID\VID_05&PID_0CE6\1"), None);
        assert_eq!(parse_vid_pid(r"HID\VID_ZZZZ&PID_0CE6\1"), None);
        assert_eq!(parse_vid_pid(""), None);
    }

    #[test]
    fn parse_looks_only_at_the_device_id_part() {
        // A VID_ in the instance part (after the second backslash) is not
        // the device's.
        assert_eq!(parse_vid_pid(r"HID\ConvertedDevice\VID_054C&PID_0CE6"), None);
    }

    // ── kind ──

    #[test]
    fn kind_for_supported_pads_with_a_native_driver() {
        assert_eq!(kind_for(0x054C, 0x0CE6), Some(ControllerKind::DualSense));
        assert_eq!(kind_for(0x057E, 0x2009), Some(ControllerKind::NsPro));
    }

    #[test]
    fn kind_for_others_is_none() {
        // Switch 2 Pro: no native driver, never hidden.
        assert_eq!(kind_for(0x057E, 0x2069), None);
        // DualSense Edge: the app does not drive it.
        assert_eq!(kind_for(0x054C, 0x0DF2), None);
        assert_eq!(kind_for(0x046D, 0xC52B), None);
    }

    #[test]
    fn kind_for_instance_follows_the_buses_the_app_drives() {
        // DualSense: USB and Bluetooth.
        let ds_usb = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
        let ds_bt = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000";
        assert_eq!(kind_for_instance(ds_usb), Some(ControllerKind::DualSense));
        assert_eq!(kind_for_instance(ds_bt), Some(ControllerKind::DualSense));
        // Switch Pro: Bluetooth only; a USB one is not driven, so hiding it
        // would leave the game with nothing.
        let ns_bt = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\9&1&0&0000";
        assert_eq!(kind_for_instance(ns_bt), Some(ControllerKind::NsPro));
        assert_eq!(kind_for_instance(r"HID\VID_057E&PID_2009\7&3&0&0000"), None);
        assert_eq!(kind_for_instance(r"HID\VID_046D&PID_C52B\7&3&0&0000"), None);
        // Xbox Nano: Bluetooth LE (HOGP) only, and only its gamepad
        // collection, Col01 (the one the HID source reads).
        let nano = |col: &str| format!(r"HID\{{00001812-0000-1000-8000-00805f9b34fb}}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&{col}\8&113d3dd1&0&0000");
        assert_eq!(kind_for_instance(&nano("Col01")), Some(ControllerKind::XboxNano));
        assert_eq!(kind_for_instance(&nano("col01").to_uppercase()), Some(ControllerKind::XboxNano), "any case");
        // Keyboard, mouse and vendor collections are not the Nano's to hide
        // or remember.
        for col in ["Col02", "Col03", "Col04"] {
            assert_eq!(kind_for_instance(&nano(col)), None, "{col}");
        }
        assert_eq!(
            kind_for_instance(r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8\8&1&0&0000"),
            None,
            "no collection"
        );
        assert_eq!(kind_for_instance(r"HID\VID_045E&PID_0B4F\7&3&0&0000"), None, "USB Nano");
    }

    // ── merge / unmerge ──

    #[test]
    fn merge_appends_ours_after_foreign_entries_in_order() {
        let list = s(&["X", "Y"]);
        assert_eq!(merge(&list, &s(&["A", "B"])), s(&["X", "Y", "A", "B"]));
    }

    #[test]
    fn merge_skips_entries_already_present_case_insensitively() {
        let list = s(&[r"HID\VID_054C&PID_0CE6\7&A", "Y"]);
        assert_eq!(merge(&list, &s(&[r"hid\vid_054c&pid_0ce6\7&a", "B"])), s(&[r"HID\VID_054C&PID_0CE6\7&A", "Y", "B"]));
    }

    #[test]
    fn merge_does_not_duplicate_within_ours() {
        assert_eq!(merge(&[], &s(&["A", "a", "A"])), s(&["A"]));
    }

    #[test]
    fn merge_keeps_foreign_duplicates_untouched() {
        // Not ours to tidy.
        let list = s(&["X", "x"]);
        assert_eq!(merge(&list, &s(&["A"])), s(&["X", "x", "A"]));
    }

    #[test]
    fn unmerge_removes_only_ours_case_insensitively() {
        let list = s(&["X", "a", "Y", "B", "Z"]);
        assert_eq!(unmerge(&list, &s(&["A", "b"])), s(&["X", "Y", "Z"]));
    }

    #[test]
    fn unmerge_of_absent_entries_is_a_no_op() {
        let list = s(&["X", "Y"]);
        assert_eq!(unmerge(&list, &s(&["Q"])), list);
        assert_eq!(unmerge(&[], &s(&["Q"])), Vec::<String>::new());
    }

    #[test]
    fn merge_then_unmerge_restores_the_foreign_list() {
        let list = s(&["X", "Y"]);
        let ours = s(&["A", "B"]);
        assert_eq!(unmerge(&merge(&list, &ours), &ours), list);
    }

    // ── IOCTL codes ──

    #[test]
    fn ctl_code_matches_winioctl() {
        // CTL_CODE(t, f, m, a) = (t << 16) | (a << 14) | (f << 2) | m
        assert_eq!(ctl_code(0x22, 0x800, 0, 0), 0x0022_2000);
        assert_eq!(ctl_code(0x22, 1, 3, 2), 0x0022_8007);
    }

    #[test]
    fn ioctl_values_match_the_driver() {
        // Logic.h: CTL_CODE(32769, 2048..=2055, METHOD_BUFFERED, FILE_READ_DATA)
        assert_eq!(IOCTL_GET_WHITELIST, 0x8001_6000);
        assert_eq!(IOCTL_SET_WHITELIST, 0x8001_6004);
        assert_eq!(IOCTL_GET_BLACKLIST, 0x8001_6008);
        assert_eq!(IOCTL_SET_BLACKLIST, 0x8001_600C);
        assert_eq!(IOCTL_GET_ACTIVE, 0x8001_6010);
        assert_eq!(IOCTL_SET_ACTIVE, 0x8001_6014);
        assert_eq!(IOCTL_GET_WLINVERSE, 0x8001_6018);
        assert_eq!(IOCTL_SET_WLINVERSE, 0x8001_601C);
    }

    // ── state file ──

    #[test]
    fn state_path_sits_beside_the_config() {
        let p = HideState::path_beside(Path::new("/x/y/padtokbm.json"));
        assert_eq!(p, Path::new("/x/y/padtokbm.hidhide.json"));
    }

    #[test]
    fn state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        let st = HideState { ids: s(&[r"HID\VID_054C&PID_0CE6\7&A"]), activated_by_us: true, whitelisted: None };
        st.save(&path).unwrap();
        assert_eq!(HideState::load(&path), st);
        assert!(!dir.path().join("padtokbm.hidhide.json.tmp").exists(), "no tmp left");
    }

    #[test]
    fn state_load_missing_or_corrupt_is_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        assert_eq!(HideState::load(&path), HideState::default());
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(HideState::load(&path), HideState::default());
        std::fs::write(&path, b"{\"ids\": 5}").unwrap();
        assert_eq!(HideState::load(&path), HideState::default());
    }

    #[test]
    fn state_load_tolerates_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        std::fs::write(&path, b"{\"ids\": [\"A\"]}").unwrap();
        assert_eq!(HideState::load(&path), HideState { ids: s(&["A"]), activated_by_us: false, whitelisted: None });
    }

    #[test]
    fn saving_an_empty_state_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        HideState { ids: s(&["A"]), activated_by_us: false, whitelisted: None }.save(&path).unwrap();
        assert!(path.exists());
        HideState::default().save(&path).unwrap();
        assert!(!path.exists());
        // And again, with no file there: still fine.
        HideState::default().save(&path).unwrap();
    }

    #[test]
    fn state_round_trips_the_whitelisted_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        let st = HideState {
            ids: vec![],
            activated_by_us: false,
            whitelisted: Some(r"\Device\HarddiskVolume3\Games\PadToKBM.exe".into()),
        };
        st.save(&path).unwrap();
        assert!(path.exists(), "a whitelist entry alone must still be undone later");
        assert_eq!(HideState::load(&path), st);
    }

    #[test]
    fn state_from_t2a_without_whitelisted_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padtokbm.hidhide.json");
        std::fs::write(&path, b"{\"ids\": [\"A\"], \"activated_by_us\": true}").unwrap();
        assert_eq!(HideState::load(&path), HideState { ids: s(&["A"]), activated_by_us: true, whitelisted: None });
    }

    #[test]
    fn state_with_only_a_whitelisted_path_is_not_empty() {
        assert!(!HideState { whitelisted: Some("X".into()), ..Default::default() }.is_empty());
    }

    #[test]
    fn state_with_only_activated_by_us_is_not_empty() {
        // We turned Active on and must still turn it off later.
        assert!(!HideState { ids: vec![], activated_by_us: true, whitelisted: None }.is_empty());
        assert!(HideState::default().is_empty());
    }

    // ── Windows, read-only (HidHide is not installed on the dev machine) ──

    #[cfg(windows)]
    #[test]
    fn lists_present_hid_devices() {
        let ids = present_hid_instance_ids();
        for id in &ids {
            println!("hid: {id}  vid/pid={:04x?}  kind={:?}", parse_vid_pid(id), parse_vid_pid(id).and_then(|(v, p)| kind_for(v, p)));
        }
        assert!(!ids.is_empty(), "a keyboard / mouse is a HID device");
    }

    #[cfg(windows)]
    #[test]
    fn open_reports_absent_without_hidhide() {
        // Never call a set_* here: this must not touch a real HidHide setup.
        match Driver::open() {
            Err(Error::Absent) => println!("Driver::open() = Absent"),
            Err(e) => println!("Driver::open() = {e:?} (HidHide present or busy?)"),
            Ok(d) => {
                // Installed on this machine after all: read only.
                println!("HidHide present: active={:?} inverse={:?}", d.active(), d.inverse());
            }
        }
        if crate::hidhide_setup::status() == "missing" {
            assert!(matches!(Driver::open(), Err(Error::Absent)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn own_image_path_is_an_nt_device_path() {
        let p = own_image_nt_path().expect("NT image path");
        println!("own_image_nt_path() = {p}");
        assert!(p.starts_with(r"\Device\"), "{p}");
        assert!(p.to_ascii_lowercase().ends_with(".exe"), "{p}");
        assert!(!p.contains(":"), "{p}");
    }

    // ── platform stubs ──

    #[cfg(not(windows))]
    #[test]
    fn off_windows_everything_is_unsupported() {
        assert!(matches!(Driver::open(), Err(Error::Unsupported)));
        assert!(present_hid_instance_ids().is_empty());
        assert!(matches!(own_image_nt_path(), Err(Error::Unsupported)));
        assert_eq!(bt_addr_of_instance(DS_BT_REAL), None, "no address off Windows");
    }

    // ── Bluetooth address (the parent's instance id) ──

    /// Read on the maintainer's Windows (2026-11-08,
    /// `Get-PnpDeviceProperty DEVPKEY_Device_Parent`).
    const DS_BT_REAL: &str = r"HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0CE6\7&16990F05&2&0000";
    const DS_BT_PARENT: &str = r"BTHENUM\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\6&a71703f&0&7C66EF8223D2_C00000000";
    const NS2_USB_PARENT: &str = r"USB\VID_057E&PID_2069&MI_00\6&2356d3fd&0&0000";

    #[test]
    fn the_bluetooth_address_is_read_from_a_bthenum_parent() {
        assert_eq!(bt_addr_from_parent(DS_BT_PARENT).as_deref(), Some("7C66EF8223D2"));
        assert_eq!(bt_addr_from_parent(&DS_BT_PARENT.to_lowercase()).as_deref(), Some("7C66EF8223D2"), "upper-cased");
        assert_eq!(bt_addr_from_parent(NS2_USB_PARENT), None, "USB: no address");
        assert_eq!(bt_addr_from_parent(DS_BT_REAL), None, "a HID id is not a BTHENUM parent");
        assert_eq!(bt_addr_from_parent(r"BTHENUM\{x}\6&a71703f&0&7C66EF8223_C00000000"), None, "10 digits");
        assert_eq!(bt_addr_from_parent(r"BTHENUM\{x}\6&a71703f&0&7C66EF8223ZZ_C00000000"), None, "not hex");
        assert_eq!(bt_addr_from_parent(""), None);
    }

    #[test]
    fn the_nanos_address_is_read_from_its_hidapi_path() {
        // hidapi paths read on the maintainer's PC (2026-11-09), without and
        // with Microsoft's Xbox driver.
        let plain = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col01#8&113d3dd1&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let official = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}&Dev&VID_045e&PID_0b4f&REV_0619&fb3a7c7df9a8&Col01&IG_00#8&4ca2c2&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(bt_addr_from_hid_path(plain).as_deref(), Some("FB3A7C7DF9A8"));
        assert_eq!(bt_addr_from_hid_path(official).as_deref(), Some("FB3A7C7DF9A8"));
        assert_eq!(bt_addr_from_hid_path(official).as_deref().and_then(bt_addr_value), Some(0xFB3A7C7DF9A8));
        assert_eq!(
            bt_addr_from_hid_path(r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6#9&2d1c&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"),
            None,
            "classic Bluetooth path: no address in it"
        );
        assert_eq!(bt_addr_from_hid_path(""), None);
    }

    /// The Xbox Nano's HID-over-GATT service device (the maintainer's Nano,
    /// address FB:3A:7C:7D:F9:A8).
    const NANO_BLE_PARENT: &str =
        r"BTHLEDEVICE\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_FB3A7C7DF9A8\8&113d3dd1&0&0000";

    #[test]
    fn the_bluetooth_address_is_read_from_a_bthledevice_parent() {
        assert_eq!(bt_addr_from_parent(NANO_BLE_PARENT).as_deref(), Some("FB3A7C7DF9A8"));
        assert_eq!(bt_addr_from_parent(&NANO_BLE_PARENT.to_lowercase()).as_deref(), Some("FB3A7C7DF9A8"), "upper-cased");
        assert_eq!(bt_addr_from_parent(r"BTHLEDEVICE\{00001812-x}_Dev_VID&02045e_PID&0b4f_REV&0609\8&1&0&0000"), None, "no address");
        assert_eq!(bt_addr_from_parent(r"BTHLEDEVICE\{00001812-x}_Dev_VID&02045e_PID&0b4f_REV&0609_FB3A7C7DF9"), None, "10 digits");
        assert!(is_ble_instance(r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col01\8&1&0&0000"));
        assert!(!is_ble_instance(DS_BT_REAL), "classic Bluetooth");
        assert!(!is_ble_instance(r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000"), "USB");
    }

    /// The maintainer's Nano after Microsoft's Xbox driver was installed and
    /// it was paired again (2026-11-09): firmware REV 0619, a new id format
    /// (`&DEV&VID_…&<address>&COL01&IG_00`).
    const NANO_OFFICIAL: &str =
        r"HID\{00001812-0000-1000-8000-00805F9B34FB}&DEV&VID_045E&PID_0B4F&REV_0619&FB3A7C7DF9A8&COL01&IG_00\8&4ca2c2&0&0000";
    /// The same Nano without that driver (firmware REV 0609).
    const NANO_PLAIN: &str =
        r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col01\8&113d3dd1&0&0000";

    #[test]
    fn the_nano_is_known_in_both_id_formats() {
        assert_eq!(parse_vid_pid(NANO_OFFICIAL), Some((0x045E, 0x0B4F)));
        for id in [NANO_OFFICIAL, NANO_PLAIN] {
            assert_eq!(kind_for_instance(id), Some(ControllerKind::XboxNano), "{id}");
            assert_eq!(kind_for_instance(&id.to_ascii_lowercase()), Some(ControllerKind::XboxNano), "any case: {id}");
            assert!(is_gamepad_collection(id), "{id}");
            assert!(is_ble_instance(id) && is_bluetooth_instance(id), "{id}");
        }
        // Only the gamepad collection, in the new format too (every collection
        // carries `&IG_00` there: `…&COL02&IG_00`, read on the same PC).
        for col in ["COL02", "COL03", "COL04"] {
            let other = NANO_OFFICIAL.replace("COL01", col);
            assert_eq!(kind_for_instance(&other), None, "{other}");
            assert!(!is_gamepad_collection(&other), "{other}");
        }
        let no_col = NANO_OFFICIAL.replace("&COL01&IG_00", "");
        assert_eq!(kind_for_instance(&no_col), None, "no collection");
        // `&COL01` inside a longer field is not collection 1.
        assert!(!is_gamepad_collection(&NANO_OFFICIAL.replace("COL01", "COL012")));
    }

    #[test]
    fn the_bluetooth_le_address_is_read_from_the_nano_id_itself() {
        assert_eq!(bt_addr_from_instance(NANO_OFFICIAL).as_deref(), Some("FB3A7C7DF9A8"));
        assert_eq!(bt_addr_from_instance(NANO_PLAIN).as_deref(), Some("FB3A7C7DF9A8"), "old format, upper-cased");
        assert_eq!(bt_addr_from_instance(&NANO_OFFICIAL.replace("COL01", "COL02")).as_deref(), Some("FB3A7C7DF9A8"));
        // Classic Bluetooth and USB ids carry no address (their parent does).
        assert_eq!(bt_addr_from_instance(DS_BT_REAL), None);
        assert_eq!(bt_addr_from_instance(r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000"), None);
        assert_eq!(bt_addr_from_instance(&NANO_OFFICIAL.replace("FB3A7C7DF9A8&", "")), None, "no address field");
        assert_eq!(bt_addr_from_instance(""), None);
        // A BTHLEDEVICE parent in the new format reads the same way.
        let parent = r"BTHLEDEVICE\{00001812-0000-1000-8000-00805F9B34FB}&DEV&VID_045E&PID_0B4F&REV_0619&FB3A7C7DF9A8\8&1&0&0000";
        assert_eq!(bt_addr_from_parent(parent).as_deref(), Some("FB3A7C7DF9A8"));
    }

    #[test]
    fn a_bluetooth_address_is_the_bth_addr_the_disconnect_takes() {
        assert_eq!(bt_addr_value("7C66EF8223D2"), Some(0x7C66_EF82_23D2));
        assert_eq!(bt_addr_value("7c66ef8223d2"), Some(0x7C66_EF82_23D2));
        assert_eq!(bt_addr_value("7C66EF8223"), None);
        assert_eq!(bt_addr_value("7C66EF8223DZ"), None);
        assert!(is_bt_addr("7C66EF8223D2"));
        assert!(!is_bt_addr("7c66ef8223d2"), "stored upper-case");
        assert!(!is_bt_addr("7C:66:EF:82:23:D2"));
    }

    /// Read-only lookup of every present Bluetooth pad's address (the
    /// maintainer's DualSense: `7C66EF8223D2` when its devnode exists).
    #[cfg(windows)]
    #[test]
    fn looks_up_the_bluetooth_address_of_present_pads() {
        let real = bt_addr_of_instance(DS_BT_REAL);
        println!("bt_addr_of_instance({DS_BT_REAL}) = {real:?}");
        if let Some(a) = &real {
            assert_eq!(a, "7C66EF8223D2");
        } else {
            println!("that devnode is not on this machine: skipped");
        }
        for id in present_hid_instance_ids() {
            if kind_for_instance(&id).is_some() {
                let a = bt_addr_of_instance(&id);
                println!("pad {id}: {a:?}");
                let bt = is_bluetooth_instance(&id);
                assert_eq!(a.is_some(), bt, "a Bluetooth HID pad has an address, a USB one none: {id}");
            }
        }
    }
}
