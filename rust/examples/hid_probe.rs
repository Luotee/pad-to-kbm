//! HID probe: list HID devices and record input reports (dev tool, not shipped).
//!
//!   hid_probe                          list every HID device
//!   hid_probe --dump 045e:xxxx         print each input report: `<ms> <hex>`
//!   hid_probe --dump V:P --label a     wait for Enter, record 3 s into
//!                                      <out>/a.hex, repeat
//!   --usage 0001/0006                  pick that collection for --dump (hex page/usage)
//!   --all                              open every collection of vid:pid, one reader
//!                                      thread each; lines are prefixed `<page>/<usage> `
//!   --out <dir>                        output dir (default ./xbox_nano)
//!   hid_probe --dump V:P --labels a,b,x   same, with the label list up front
//!
//! Stop with Ctrl-C.
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::thread;
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

/// Output dir: `--out <dir>` or `xbox_nano` under the current directory.
fn out_dir(args: &[String]) -> PathBuf {
    let p = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("xbox_nano"));
    if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p)
    }
}

/// Write one fixture file (creating the dir) and return its absolute path.
fn write_fixture(dir: &std::path::Path, label: &str, lines: &[String]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{label}.hex"));
    std::fs::write(&path, lines.join("\n") + "\n")?;
    let abs = std::fs::canonicalize(&path).unwrap_or(path);
    // Windows canonicalize gives `\\?\` verbatim paths; show the plain form.
    let t = abs.to_string_lossy().to_string();
    let t = if let Some(r) = t.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{r}")
    } else {
        t.strip_prefix(r"\\?\").map(String::from).unwrap_or(t)
    };
    Ok(PathBuf::from(t))
}

fn wait_enter(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    matches!(io::stdin().lock().read_line(&mut s), Ok(n) if n > 0)
}

type Opened = Vec<(String, HidDevice)>;

fn usage_str(d: &DeviceInfo) -> String {
    format!("{:04x}/{:04x}", d.usage_page(), d.usage())
}

fn record(devs: Opened, label: &str, dir: &std::path::Path, prefixed: bool) -> (Opened, io::Result<()>) {
    // Throw away whatever piled up while waiting for Enter.
    let mut buf = [0u8; 256];
    for (_, d) in &devs {
        while matches!(d.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    }
    println!("錄製 {RECORD_SECS} 秒，現在按「{label}」……");
    let start = Instant::now();
    let handles: Vec<_> = devs
        .into_iter()
        .map(|(u, d)| {
            thread::spawn(move || {
                let mut buf = [0u8; 256];
                let mut rows: Vec<(u128, String)> = Vec::new();
                while start.elapsed() < Duration::from_secs(RECORD_SECS) {
                    match d.read_timeout(&mut buf, 20) {
                        Ok(n) if n > 0 => {
                            let ms = start.elapsed().as_millis();
                            let line = if prefixed {
                                format!("{u} {ms} {}", hex(&buf[..n]))
                            } else {
                                hex(&buf[..n])
                            };
                            rows.push((ms, line));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("{u} 讀取失敗：{e}");
                            break;
                        }
                    }
                }
                (u, d, rows)
            })
        })
        .collect();
    let mut back = Vec::new();
    let mut all_rows = Vec::new();
    for h in handles {
        let (u, d, rows) = h.join().expect("reader thread");
        all_rows.extend(rows);
        back.push((u, d));
    }
    all_rows.sort_by_key(|r| r.0);
    let lines: Vec<String> = all_rows.into_iter().map(|r| r.1).collect();
    let res = write_fixture(dir, label, &lines).map(|path| {
        println!("已存 {} 份報告 -> {}", lines.len(), path.display());
    });
    (back, res)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let dir = out_dir(&args);
    if args.iter().any(|a| a == "--selftest-write") {
        match write_fixture(&dir, "selftest", &["00 11 22".to_string()]) {
            Ok(p) => println!("已存 -> {}", p.display()),
            Err(e) => {
                eprintln!("寫檔失敗：{e}");
                std::process::exit(1);
            }
        }
        return;
    }
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
    let all = args.iter().any(|a| a == "--all");
    let infos: Vec<&DeviceInfo> = if all {
        api.device_list()
            .filter(|d| d.vendor_id() == vid && d.product_id() == pid)
            .collect()
    } else if let Some(u) = arg("--usage") {
        let want = u.to_lowercase();
        api.device_list()
            .filter(|d| d.vendor_id() == vid && d.product_id() == pid && usage_str(d) == want)
            .take(1)
            .collect()
    } else {
        pick(&api, vid, pid).into_iter().collect()
    };
    if infos.is_empty() {
        eprintln!("找不到 {vid:04x}:{pid:04x}（或指定的 --usage），先不帶參數執行看清單");
        std::process::exit(1);
    }
    let prefixed = all;
    let mut devs: Opened = Vec::new();
    for info in &infos {
        println!("開啟 {:04x}:{:04x} usage={} {}", vid, pid, usage_str(info), info.path().to_string_lossy());
        match info.open_device(&api) {
            Ok(d) => devs.push((usage_str(info), d)),
            Err(e) => eprintln!("開啟 {} 失敗：{e}", usage_str(info)),
        }
    }
    if devs.is_empty() {
        std::process::exit(1);
    }

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
        let handles: Vec<_> = devs
            .into_iter()
            .map(|(u, d)| {
                thread::spawn(move || {
                    let mut buf = [0u8; 256];
                    loop {
                        match d.read_timeout(&mut buf, 100) {
                            Ok(n) if n > 0 => {
                                let ms = start.elapsed().as_millis();
                                if prefixed {
                                    println!("{u} {ms} {}", hex(&buf[..n]));
                                } else {
                                    println!("{ms} {}", hex(&buf[..n]));
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!("{u} 讀取失敗：{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
        return;
    }

    println!("輸出資料夾：{}", dir.display());
    let single = arg("--label").is_some() && arg("--labels").is_none();
    let mut queue = labels.into_iter();
    let mut cur = queue.next();
    while let Some(label) = cur.take() {
        if !wait_enter(&format!("按 Enter 後按「{label}」（Ctrl-C 結束）："))
            && !single
        {
            break;
        }
        let (back, res) = record(std::mem::take(&mut devs), &label, &dir, prefixed);
        devs = back;
        if let Err(e) = res {
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
