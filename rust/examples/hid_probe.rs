//! HID probe: list HID devices and record input reports (dev tool, not shipped).
//!
//!   hid_probe                          list every HID device
//!   hid_probe --dump 045e:xxxx         print each input report: `<ms> <hex>`
//!   hid_probe --dump V:P --label a     wait for Enter, record 3 s into
//!                                      rust/tests/fixtures/xbox_nano/a.hex, repeat
//!   hid_probe --dump V:P --labels a,b,x   same, with the label list up front
//!
//! Stop with Ctrl-C.
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hidapi::{BusType, DeviceInfo, HidApi, HidDevice};

const RECORD_SECS: u64 = 3;

fn bus_name(b: BusType) -> &'static str {
    match b {
        BusType::Usb => "usb",
        BusType::Bluetooth => "bluetooth",
        BusType::I2c => "i2c",
        BusType::Spi => "spi",
        _ => "unknown",
    }
}

fn hex(buf: &[u8]) -> String {
    buf.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

fn parse_vidpid(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.split_once(':')?;
    let f = |x: &str| u16::from_str_radix(x.trim_start_matches("0x"), 16).ok();
    Some((f(v)?, f(p)?))
}

fn list(api: &HidApi) {
    let mut n = 0;
    for d in api.device_list() {
        n += 1;
        println!(
            "{:04x}:{:04x}  bus={:<9} usage={:04x}/{:04x}  product={:?}  path={}",
            d.vendor_id(),
            d.product_id(),
            bus_name(d.bus_type()),
            d.usage_page(),
            d.usage(),
            d.product_string().unwrap_or(""),
            d.path().to_string_lossy()
        );
    }
    println!("共 {n} 個 HID 裝置");
}

/// Prefer the gamepad collection (usage page 1, usage 5) among matches.
fn pick<'a>(api: &'a HidApi, vid: u16, pid: u16) -> Option<&'a DeviceInfo> {
    let mut m = api
        .device_list()
        .filter(|d| d.vendor_id() == vid && d.product_id() == pid);
    let all: Vec<&DeviceInfo> = m.by_ref().collect();
    all.iter()
        .find(|d| d.usage_page() == 1 && d.usage() == 5)
        .or(all.first())
        .copied()
}

fn fixture_dir() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.join("tests").join("fixtures").join("xbox_nano")
}

fn wait_enter(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    matches!(io::stdin().lock().read_line(&mut s), Ok(n) if n > 0)
}

fn record(dev: &HidDevice, label: &str) -> io::Result<()> {
    // Throw away whatever piled up while waiting for Enter.
    let mut buf = [0u8; 256];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    println!("錄製 {RECORD_SECS} 秒，現在按「{label}」……");
    let start = Instant::now();
    let mut lines = Vec::new();
    while start.elapsed() < Duration::from_secs(RECORD_SECS) {
        match dev.read_timeout(&mut buf, 20) {
            Ok(n) if n > 0 => lines.push(hex(&buf[..n])),
            Ok(_) => {}
            Err(e) => {
                eprintln!("讀取失敗：{e}");
                break;
            }
        }
    }
    let dir = fixture_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{label}.hex"));
    std::fs::write(&path, lines.join("\n") + "\n")?;
    println!("已存 {} 份報告 -> {}", lines.len(), path.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("hidapi 初始化失敗：{e}");
            std::process::exit(1);
        }
    };
    let Some(target) = arg("--dump") else {
        list(&api);
        return;
    };
    let Some((vid, pid)) = parse_vidpid(target) else {
        eprintln!("--dump 參數格式應為 <vid:pid>（十六進位），例如 045e:0b13");
        std::process::exit(2);
    };
    let Some(info) = pick(&api, vid, pid) else {
        eprintln!("找不到 {vid:04x}:{pid:04x}，先不帶參數執行看清單");
        std::process::exit(1);
    };
    println!(
        "開啟 {:04x}:{:04x} usage={:04x}/{:04x} {}",
        vid,
        pid,
        info.usage_page(),
        info.usage(),
        info.path().to_string_lossy()
    );
    let dev = match info.open_device(&api) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("開啟失敗：{e}");
            std::process::exit(1);
        }
    };

    let labels: Vec<String> = if let Some(l) = arg("--labels") {
        l.split(',').filter(|s| !s.is_empty()).map(String::from).collect()
    } else if let Some(l) = arg("--label") {
        vec![l.clone()]
    } else {
        Vec::new()
    };

    if labels.is_empty() {
        println!("印出輸入報告，Ctrl-C 結束");
        let start = Instant::now();
        let mut buf = [0u8; 256];
        loop {
            match dev.read_timeout(&mut buf, 100) {
                Ok(n) if n > 0 => println!("{} {}", start.elapsed().as_millis(), hex(&buf[..n])),
                Ok(_) => {}
                Err(e) => {
                    eprintln!("讀取失敗：{e}");
                    std::process::exit(1);
                }
            }
        }
    }

    println!("輸出資料夾：{}", fixture_dir().display());
    let single = arg("--label").is_some() && arg("--labels").is_none();
    let mut queue = labels.into_iter();
    let mut cur = queue.next();
    while let Some(label) = cur.take() {
        if !wait_enter(&format!("按 Enter 後按「{label}」（Ctrl-C 結束）："))
            && !single
        {
            break;
        }
        if let Err(e) = record(&dev, &label) {
            eprintln!("寫檔失敗：{e}");
            std::process::exit(1);
        }
        cur = queue.next();
        if cur.is_none() && single {
            // --label: keep prompting for the next name.
            print!("下一個標籤名稱（空白結束）：");
            let _ = io::stdout().flush();
            let mut s = String::new();
            if io::stdin().lock().read_line(&mut s).is_ok() {
                let s = s.trim();
                if !s.is_empty() {
                    cur = Some(s.to_string());
                }
            }
        }
    }
    println!("完成");
}
