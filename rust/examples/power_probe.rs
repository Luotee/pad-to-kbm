//! Dev-only probe: try ways to turn a Bluetooth pad off from the PC without
//! administrator rights. Not part of the app.
//!
//!   power_probe --list            XInput slots 0..3 that report a pad
//!   power_probe --xinput <slot>   XInputPowerOffController (xinput1_4 #103)
//!   power_probe --bth <ADDR12>    IOCTL_BTH_DISCONNECT_DEVICE on that address

#[cfg(windows)]
mod win {
    use windows_sys::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> Option<unsafe extern "system" fn(u32) -> u32>;
    }

    pub fn list() {
        for slot in 0..4u32 {
            let mut st: XINPUT_STATE = unsafe { std::mem::zeroed() };
            let rc = unsafe { XInputGetState(slot, &mut st) };
            println!("XInput slot {slot}: {}", if rc == 0 { "pad present" } else { "empty" });
        }
    }

    /// Poll XInput slot `slot` (plain + the Guide-aware ordinal 100) and print changes.
    pub fn xinput_watch(slot: u32, secs: u64) {
        let name: Vec<u16> = "xinput1_4.dll\0".encode_utf16().collect();
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        let ex = unsafe { GetProcAddress(module, 100usize as *const u8) };
        let ex: Option<unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32> = ex.map(|f| unsafe { std::mem::transmute(f) });
        println!("XInputGetStateEx (#100) {}", if ex.is_some() { "found" } else { "missing" });
        let t0 = std::time::Instant::now();
        let mut last = (u16::MAX, 0u8, 0u8);
        while t0.elapsed().as_secs() < secs {
            let mut st: XINPUT_STATE = unsafe { std::mem::zeroed() };
            let rc = match ex { Some(f) => unsafe { f(slot, &mut st) }, None => unsafe { XInputGetState(slot, &mut st) } };
            let g = st.Gamepad;
            let cur = (g.wButtons, g.bLeftTrigger, g.bRightTrigger);
            if rc == 0 && cur != last {
                println!("{:>6} ms buttons=0x{:04X} LT={} RT={} LX={} LY={}", t0.elapsed().as_millis(), g.wButtons, g.bLeftTrigger, g.bRightTrigger, g.sThumbLX, g.sThumbLY);
                last = cur;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub fn xinput_off(slot: u32) {
        let name: Vec<u16> = "xinput1_4.dll\0".encode_utf16().collect();
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module == 0 {
            println!("xinput1_4.dll not loaded");
            return;
        }
        // An ordinal is passed as the low word of the name pointer.
        let f = unsafe { GetProcAddress(module, 103usize as *const u8) };
        match f {
            None => println!("ordinal 103 not exported"),
            Some(f) => {
                let rc = unsafe { f(slot) };
                println!("XInputPowerOffController({slot}) returned {rc} (0 = success)");
            }
        }
    }

    /// The raw IOCTL on every radio, with the Win32 error of each attempt.
    pub fn bth_raw(addr: &str) {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::Devices::Bluetooth::{
            BluetoothFindFirstRadio, BluetoothFindNextRadio, BluetoothFindRadioClose, BLUETOOTH_FIND_RADIO_PARAMS,
        };
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
        use windows_sys::Win32::System::IO::DeviceIoControl;
        let Some(a) = dualsense_mapper::hidhide::bt_addr_value(addr) else {
            println!("bad address");
            return;
        };
        unsafe {
            let mut p: BLUETOOTH_FIND_RADIO_PARAMS = zeroed();
            p.dwSize = size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
            let mut radio: HANDLE = std::ptr::null_mut();
            let find = BluetoothFindFirstRadio(&p, &mut radio);
            if find.is_null() {
                println!("no radio: err {}", GetLastError());
                return;
            }
            let mut i = 0;
            loop {
                let mut ret = 0u32;
                let ok = DeviceIoControl(
                    radio,
                    0x0041_000C,
                    &a as *const u64 as *const _,
                    size_of::<u64>() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut ret,
                    std::ptr::null_mut(),
                );
                println!("radio {i}: ok={ok} err={}", if ok == 0 { GetLastError() } else { 0 });
                CloseHandle(radio);
                i += 1;
                if BluetoothFindNextRadio(find, &mut radio) == 0 {
                    break;
                }
            }
            BluetoothFindRadioClose(find);
        }
    }

    /// Every device the classic Bluetooth API lists (read-only).
    pub fn bt_list() {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::Devices::Bluetooth::{
            BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice, BLUETOOTH_DEVICE_INFO,
            BLUETOOTH_DEVICE_SEARCH_PARAMS,
        };
        unsafe {
            let mut p: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
            p.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
            p.fReturnAuthenticated = 1;
            p.fReturnRemembered = 1;
            p.fReturnConnected = 1;
            let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
            info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            let find = BluetoothFindFirstDevice(&p, &mut info);
            if find.is_null() {
                println!("no devices");
                return;
            }
            loop {
                let n = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
                println!(
                    "{:012X} connected={} remembered={} name={}",
                    info.Address.Anonymous.ullLong,
                    info.fConnected,
                    info.fRemembered,
                    String::from_utf16_lossy(&info.szName[..n])
                );
                info = zeroed();
                info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
                if BluetoothFindNextDevice(find, &mut info) == 0 {
                    break;
                }
            }
            BluetoothFindDeviceClose(find);
        }
    }

    // GameInput v0 (GameInput.h in the Windows SDK): vtable slots.
    type Vt = *const *const usize;
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> isize;
        #[link_name = "GetProcAddress"]
        fn GetProcAddressA(module: isize, name: *const u8) -> *const core::ffi::c_void;
    }
    unsafe fn slot(obj: Vt, i: usize) -> *const core::ffi::c_void {
        *(*obj).add(i) as *const core::ffi::c_void
    }

    /// GameInput: find the device behind the latest gamepad reading, print
    /// its info and battery, and (with `off`) call PowerOff if it says it can.
    pub fn gameinput(off: bool) {
        unsafe {
            let m = LoadLibraryA(b"GameInput.dll\0".as_ptr());
            if m == 0 {
                println!("GameInput.dll not loaded");
                return;
            }
            let create = GetProcAddressA(m, b"GameInputCreate\0".as_ptr());
            if create.is_null() {
                println!("GameInputCreate missing");
                return;
            }
            let create: unsafe extern "system" fn(*mut Vt) -> i32 = std::mem::transmute(create);
            let mut gi: Vt = std::ptr::null();
            let hr = create(&mut gi);
            println!("GameInputCreate hr=0x{hr:08X}");
            if hr < 0 || gi.is_null() {
                return;
            }
            // IGameInput::GetCurrentReading(kind, device, reading**) = slot 4.
            let get_reading: unsafe extern "system" fn(Vt, u32, Vt, *mut Vt) -> i32 = std::mem::transmute(slot(gi, 4));
            println!("按 Nano 任一鍵或推搖桿（10 秒內）…");
            let mut reading: Vt = std::ptr::null();
            let t0 = std::time::Instant::now();
            for kind in [0x0004_0000u32, 0x0000_000E].iter().cycle() {
                if get_reading(gi, *kind, std::ptr::null(), &mut reading) >= 0 && !reading.is_null() {
                    println!("reading kind 0x{kind:X}");
                    break;
                }
                if t0.elapsed().as_secs() > 10 {
                    println!("no gamepad/controller reading");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // IGameInputReading::GetDevice(device**) = slot 6.
            let get_device: unsafe extern "system" fn(Vt, *mut Vt) = std::mem::transmute(slot(reading, 6));
            let mut dev: Vt = std::ptr::null();
            get_device(reading, &mut dev);
            if dev.is_null() {
                println!("no device");
                return;
            }
            // IGameInputDevice: 3 GetDeviceInfo, 4 GetDeviceStatus, 5 GetBatteryState, 13 PowerOff.
            let info: unsafe extern "system" fn(Vt) -> *const u8 = std::mem::transmute(slot(dev, 3));
            let p = info(dev);
            let vid = u16::from_le_bytes([*p.add(4), *p.add(5)]);
            let pid = u16::from_le_bytes([*p.add(6), *p.add(7)]);
            let caps = u32::from_le_bytes([*p.add(100), *p.add(101), *p.add(102), *p.add(103)]);
            println!("device {vid:04X}:{pid:04X} capabilities=0x{caps:X} (PowerOff bit 0x4: {})", caps & 4 != 0);
            let status: unsafe extern "system" fn(Vt) -> u32 = std::mem::transmute(slot(dev, 4));
            println!("status=0x{:X}", status(dev));
            let mut batt = [0f32; 5];
            let get_batt: unsafe extern "system" fn(Vt, *mut [f32; 5]) = std::mem::transmute(slot(dev, 5));
            get_batt(dev, &mut batt);
            let st = (batt.as_ptr().add(4) as *const u32).read();
            println!(
                "battery: chargeRate={} maxChargeRate={} remaining={} full={} status={}",
                batt[0], batt[1], batt[2], batt[3], st
            );
            if off {
                let power_off: unsafe extern "system" fn(Vt) = std::mem::transmute(slot(dev, 13));
                println!("calling PowerOff…");
                power_off(dev);
                std::thread::sleep(std::time::Duration::from_secs(3));
                println!("status after=0x{:X}", status(dev));
            }
        }
    }

    fn locate(id: &str, phantom: bool) -> Option<u32> {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
            CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CM_LOCATE_DEVNODE_PHANTOM, CR_SUCCESS,
        };
        let w: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        let mut dn = 0u32;
        let flags = if phantom { CM_LOCATE_DEVNODE_PHANTOM } else { CM_LOCATE_DEVNODE_NORMAL };
        let cr = unsafe { CM_Locate_DevNodeW(&mut dn, w.as_ptr(), flags) };
        if cr != CR_SUCCESS {
            println!("CM_Locate_DevNodeW({id}) cr={cr}");
            return None;
        }
        Some(dn)
    }

    /// CM_Request_Device_EjectW on one device node (pairing untouched).
    pub fn eject(id: &str) {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::CM_Request_Device_EjectW;
        let Some(dn) = locate(id, false) else { return };
        let mut veto = 0i32;
        let mut name = [0u16; 260];
        let cr = unsafe { CM_Request_Device_EjectW(dn, &mut veto, name.as_mut_ptr(), name.len() as u32, 0) };
        let n = name.iter().position(|&c| c == 0).unwrap_or(0);
        println!("CM_Request_Device_EjectW cr={cr} veto={veto} vetoName={}", String::from_utf16_lossy(&name[..n]));
    }

    /// CM_Query_And_Remove_SubTreeW (no UI) on one device node.
    pub fn remove(id: &str) {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{CM_Query_And_Remove_SubTreeW, CM_REMOVE_NO_RESTART};
        let Some(dn) = locate(id, false) else { return };
        let mut veto = 0i32;
        let mut name = [0u16; 260];
        let cr = unsafe { CM_Query_And_Remove_SubTreeW(dn, &mut veto, name.as_mut_ptr(), name.len() as u32, CM_REMOVE_NO_RESTART) };
        let n = name.iter().position(|&c| c == 0).unwrap_or(0);
        println!("CM_Query_And_Remove_SubTreeW cr={cr} veto={veto} vetoName={}", String::from_utf16_lossy(&name[..n]));
    }

    /// CM_Reenumerate_DevNode on a (parent) node so ejected children come back.
    pub fn reenum(id: &str) {
        use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{CM_Reenumerate_DevNode, CM_REENUMERATE_SYNCHRONOUS};
        let Some(dn) = locate(id, false) else { return };
        let cr = unsafe { CM_Reenumerate_DevNode(dn, CM_REENUMERATE_SYNCHRONOUS) };
        println!("CM_Reenumerate_DevNode cr={cr}");
    }

    /// Raw Input sink for keyboard, mouse, consumer control and system control;
    /// prints every event whose source device name contains `filter` (or all).
    pub fn raw_watch(secs: u64, filter: &str) {
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::Input::{
            GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
            RAWINPUTHEADER, RIDEV_INPUTSINK, RIDI_DEVICENAME, RID_INPUT, RIM_TYPEHID, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, PeekMessageW,
            HWND_MESSAGE, MSG, PM_REMOVE, WM_INPUT,
        };
        static mut FILTER: String = String::new();
        unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            if m == WM_INPUT {
                let mut size = 0u32;
                let hs = std::mem::size_of::<RAWINPUTHEADER>() as u32;
                GetRawInputData(l as HRAWINPUT, RID_INPUT, std::ptr::null_mut(), &mut size, hs);
                let mut buf = vec![0u8; size as usize];
                if GetRawInputData(l as HRAWINPUT, RID_INPUT, buf.as_mut_ptr() as *mut _, &mut size, hs) == size {
                    let ri = &*(buf.as_ptr() as *const RAWINPUT);
                    let mut n = 0u32;
                    GetRawInputDeviceInfoW(ri.header.hDevice, RIDI_DEVICENAME, std::ptr::null_mut(), &mut n);
                    let mut name = vec![0u16; n as usize + 1];
                    GetRawInputDeviceInfoW(ri.header.hDevice, RIDI_DEVICENAME, name.as_mut_ptr() as *mut _, &mut n);
                    let name = String::from_utf16_lossy(&name).trim_end_matches('\0').to_string();
                    #[allow(static_mut_refs)]
                    let f = FILTER.to_ascii_lowercase();
                    if f.is_empty() || name.to_ascii_lowercase().contains(&f) {
                        match ri.header.dwType {
                            t if t == RIM_TYPEKEYBOARD => {
                                let k = ri.data.keyboard;
                                println!("KEYBOARD make=0x{:X} flags=0x{:X} vkey=0x{:X} msg=0x{:X} dev={name}", k.MakeCode, k.Flags, k.VKey, k.Message);
                            }
                            t if t == RIM_TYPEMOUSE => {
                                let mo = ri.data.mouse;
                                println!("MOUSE buttons=0x{:X} x={} y={} dev={name}", mo.Anonymous.Anonymous.usButtonFlags, mo.lLastX, mo.lLastY);
                            }
                            t if t == RIM_TYPEHID => {
                                let hid = ri.data.hid;
                                let bytes = std::slice::from_raw_parts(hid.bRawData.as_ptr(), (hid.dwSizeHid * hid.dwCount) as usize);
                                let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
                                println!("HID {} dev={name}", hex.join(" "));
                            }
                            _ => {}
                        }
                    }
                }
            }
            DefWindowProcW(h, m, w, l)
        }
        unsafe {
            FILTER = filter.to_string();
            let cls: Vec<u16> = "Message\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(0, cls.as_ptr(), cls.as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            let mut devs = Vec::new();
            for (page, usage) in [(1u16, 6u16), (1, 2), (0x0C, 1), (1, 0x80), (1, 5)] {
                devs.push(RAWINPUTDEVICE { usUsagePage: page, usUsage: usage, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd });
            }
            let ok = RegisterRawInputDevices(devs.as_ptr(), devs.len() as u32, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            println!("registered={ok}; 現在按 Nano 的 Share（{secs} 秒）…");
            let t0 = std::time::Instant::now();
            let mut msg: MSG = std::mem::zeroed();
            while t0.elapsed().as_secs() < secs {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_INPUT {
                        proc_(msg.hwnd, msg.message, msg.wParam, msg.lParam);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            println!("done");
        }
    }

    pub fn bth(addr: &str) {
        match dualsense_mapper::hidhide::bt_addr_value(addr) {
            None => println!("not a 12-hex address: {addr}"),
            Some(a) => match dualsense_mapper::platform::power_off_addrs(&[a]) {
                Ok(n) => println!("IOCTL_BTH_DISCONNECT_DEVICE: {n} link(s) dropped"),
                Err(e) => println!("error: {e:#}"),
            },
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(windows)]
    {
        match args.get(1).map(String::as_str) {
            Some("--list") => win::list(),
            Some("--xinput-watch") => win::xinput_watch(0, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30)),
            Some("--xinput") => win::xinput_off(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0)),
            Some("--bth") => win::bth(args.get(2).map(String::as_str).unwrap_or("")),
            Some("--bt-list") => win::bt_list(),
            Some("--eject") => win::eject(args.get(2).map(String::as_str).unwrap_or("")),
            Some("--remove") => win::remove(args.get(2).map(String::as_str).unwrap_or("")),
            Some("--reenum") => win::reenum(args.get(2).map(String::as_str).unwrap_or("")),
            Some("--raw-watch") => win::raw_watch(
                args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20),
                args.get(3).map(String::as_str).unwrap_or("0b4f"),
            ),
            Some("--gi-info") => win::gameinput(false),
            Some("--gi-off") => win::gameinput(true),
            Some("--bth-raw") => win::bth_raw(args.get(2).map(String::as_str).unwrap_or("")),
            _ => println!("usage: power_probe --list | --xinput <slot> | --bth <ADDR12>"),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        println!("Windows only");
    }
}
