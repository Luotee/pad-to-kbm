//! v3.10.0: the Xbox Nano's companion session thread (replaces v3.8.0's
//! battery-only poller). The battery percentage still comes from Battery
//! Level 0x2A19; the companion channel adds charging and the power-off.
//! WinRT is Windows-only; these drive the thread with a scripted link and
//! a fake Battery Level reader.

use crossbeam_channel::unbounded;
use dualsense_mapper::gamepad::nano_companion::{
    self, battery_from_level, CompanionLink, CompanionOpener, LevelReader, Timing, Tracker,
};
use dualsense_mapper::gamepad::nano_companion_proto::{ack, parse, Frame, T_BATT, T_HELLO5, T_HELLO6, T_RST};
use dualsense_mapper::gamepad::{Battery, GamepadEvent};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn b(pct: u8) -> Battery {
    Battery { pct, charging: false }
}

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

const HELLO6_REPLY: &str = "5e 04 4c 0b 06 0c aa 03 e3 29 8e 0d 4a 7e 43 35 e4 b3 17 19 a6 d2 30 44 33 34 58 51 54 32 \
    36 33 32 42 42 50 01 06 00 00 00 ff 03 00 00 03 00 00 00 01 00 06 00 13 00 32 00 00 00 00 00 00 00 00 00 00 00 00 \
    00 00 00 00 00 00 00 00 01 5a";

fn fast() -> Timing {
    Timing {
        charge: Duration::from_millis(30),
        level: Duration::from_millis(40),
        hello_retry: Duration::from_millis(50),
        hello_tries: 3,
        reply: Duration::from_millis(150),
    }
}

// ── the v3.8.0 Battery Level parts, unchanged ──

#[test]
fn battery_level_byte_is_the_percentage_without_a_charging_flag() {
    assert_eq!(battery_from_level(&[0x46]), Some(b(70)), "the user's Nano: 0x46 = 70 %");
    assert_eq!(battery_from_level(&[0]), Some(b(0)));
    assert_eq!(battery_from_level(&[100]), Some(b(100)));
    assert_eq!(battery_from_level(&[101]), None, "reserved");
    assert_eq!(battery_from_level(&[]), None);
}

#[test]
fn tracker_sends_the_first_reading_and_changes_only() {
    let mut t = Tracker::default();
    assert_eq!(t.next(Some(b(70))), Some(Some(b(70))));
    assert_eq!(t.next(Some(b(70))), None);
    assert_eq!(t.next(Some(b(69))), Some(Some(b(69))));
}

#[test]
fn tracker_keeps_the_last_reading_over_one_failed_read_and_drops_it_after_two() {
    let mut t = Tracker::default();
    assert_eq!(t.next(None), None);
    t.next(Some(b(70)));
    assert_eq!(t.next(None), None, "one failed read keeps 70 %");
    assert_eq!(t.next(Some(b(70))), None, "back, unchanged: nothing");
    assert_eq!(t.next(None), None);
    assert_eq!(t.next(None), Some(None), "two in a row: the tab goes back to the lamp");
    assert_eq!(t.next(None), None, "already dropped");
    assert_eq!(t.next(Some(b(65))), Some(Some(b(65))), "a new reading shows again");
}

/// Scripted reads (the last one repeats), counted.
struct FakeReader {
    reads: Vec<Option<Vec<u8>>>,
    count: Arc<AtomicUsize>,
}

impl LevelReader for FakeReader {
    fn read(&mut self) -> Option<Vec<u8>> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        self.reads.get(n).or(self.reads.last()).cloned().flatten()
    }
}

fn level(pct: u8) -> FakeReader {
    FakeReader { reads: vec![Some(vec![pct])], count: Arc::new(AtomicUsize::new(0)) }
}

/// No companion channel (Microsoft's driver, an old pad, no address).
struct NoLink;
impl CompanionOpener for NoLink {
    fn open(&mut self) -> Option<Box<dyn CompanionLink>> {
        None
    }
}

#[test]
fn reads_at_session_start_then_every_period_and_sends_changes() {
    let count = Arc::new(AtomicUsize::new(0));
    let reader = FakeReader { reads: vec![Some(vec![70]), Some(vec![70]), Some(vec![69])], count: count.clone() };
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(NoLink, reader, fast(), &tx);
    let first = rx.recv_timeout(Duration::from_millis(500)).expect("first reading");
    assert_eq!(first, GamepadEvent::Battery(Some(b(70))));
    let second = rx.recv_timeout(Duration::from_millis(500)).expect("the change");
    assert_eq!(second, GamepadEvent::Battery(Some(b(69))), "70 again sent nothing");
    assert!(count.load(Ordering::SeqCst) >= 3);
    drop(guard);
}

/// A reader that blocks until released (a GATT read that takes its time).
struct SlowReader {
    gate: Arc<Mutex<()>>,
}

impl LevelReader for SlowReader {
    fn read(&mut self) -> Option<Vec<u8>> {
        let _held = self.gate.lock().unwrap();
        Some(vec![50])
    }
}

#[test]
fn a_slow_read_never_blocks_the_caller_and_nothing_is_sent_after_the_session() {
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let (tx, rx) = unbounded();
    let t0 = Instant::now();
    let guard = nano_companion::start_with(NoLink, SlowReader { gate: gate.clone() }, fast(), &tx);
    drop(guard);
    assert!(t0.elapsed() < Duration::from_millis(200), "start and end return at once");
    drop(held);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rx.try_iter().count(), 0);
}

#[test]
fn ending_the_session_stops_the_schedule() {
    let count = Arc::new(AtomicUsize::new(0));
    let reader = FakeReader { reads: vec![Some(vec![70])], count: count.clone() };
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(NoLink, reader, fast(), &tx);
    rx.recv_timeout(Duration::from_millis(500)).expect("first reading");
    drop(guard);
    std::thread::sleep(Duration::from_millis(80));
    let after = count.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(count.load(Ordering::SeqCst), after, "no read after the session");
}

#[test]
fn no_address_or_not_windows_reads_nothing() {
    let (tx, rx) = unbounded();
    let guard = nano_companion::start(None, &tx);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!guard.power_off(), "no session: nothing to power off");
    drop(guard);
    assert_eq!(rx.try_iter().count(), 0);
}

// ── the companion channel ──

/// The pad's side, scripted: answers our link hello, app hello, battery
/// requests (flags from `flags`) and power-off, records every write.
#[derive(Clone)]
struct Pad {
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
    inbox: Arc<Mutex<VecDeque<Vec<u8>>>>,
    flags: Arc<AtomicU8>,
    echo: bool,
    answer_batt: Arc<AtomicBool>,
    hold: Arc<Mutex<()>>,
    /// Held: every write blocks (a GATT write that takes its time).
    write_hold: Arc<Mutex<()>>,
    /// Each write of a power-off frame takes this long (ms).
    slow_rst_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl Pad {
    fn new(flags: u8) -> Self {
        Pad {
            writes: Arc::default(),
            inbox: Arc::default(),
            flags: Arc::new(AtomicU8::new(flags)),
            echo: true,
            answer_batt: Arc::new(AtomicBool::new(true)),
            hold: Arc::default(),
            write_hold: Arc::default(),
            slow_rst_ms: Arc::default(),
        }
    }
    fn data_types(&self) -> Vec<u8> {
        self.writes.lock().unwrap().iter().filter_map(|w| match parse(w) {
            Frame::Data { ty, .. } => Some(ty),
            _ => None,
        }).collect()
    }
    fn link_hellos(&self) -> usize {
        self.writes.lock().unwrap().iter().filter(|w| w[0] == 0xff).count()
    }
}

impl CompanionLink for Pad {
    fn write(&mut self, f: &[u8]) -> bool {
        drop(self.write_hold.lock().unwrap());
        if f.len() > 2 && f[0] == 5 && f[2] == T_RST {
            std::thread::sleep(Duration::from_millis(self.slow_rst_ms.load(Ordering::SeqCst)));
        }
        self.writes.lock().unwrap().push(f.to_vec());
        let mut q = self.inbox.lock().unwrap();
        if f[0] == 0xff && self.echo {
            let mut e = f.to_vec();
            e[1] = 4;
            q.push_back(e);
        }
        if let Frame::Data { ch, ty, seq, .. } = parse(f) {
            q.push_back(ack(ch, ty, seq).to_vec());
            let reply = |p: &[u8]| {
                let mut r = vec![5, ch, ty, seq.wrapping_add(4), p.len() as u8];
                r.extend_from_slice(p);
                r
            };
            match ty {
                T_HELLO6 | T_HELLO5 => q.push_back(reply(&hex(HELLO6_REPLY))),
                T_BATT if self.answer_batt.load(Ordering::SeqCst) => {
                    q.push_back(reply(&[self.flags.load(Ordering::SeqCst), 0x5a]))
                }
                _ => {}
            }
        }
        true
    }
    fn recv(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        let _held = self.hold.lock().unwrap();
        let f = self.inbox.lock().unwrap().pop_front();
        if f.is_none() {
            std::thread::sleep(timeout.min(Duration::from_millis(5)));
        }
        f
    }
    fn frag_max(&self) -> usize {
        239
    }
}

struct Opens(Option<Pad>);
impl CompanionOpener for Opens {
    fn open(&mut self) -> Option<Box<dyn CompanionLink>> {
        self.0.take().map(|p| Box::new(p) as Box<dyn CompanionLink>)
    }
}

fn wait_for(rx: &crossbeam_channel::Receiver<GamepadEvent>, want: GamepadEvent) -> Vec<GamepadEvent> {
    let mut seen = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(2) {
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(20)) {
            seen.push(e);
            if e == want {
                return seen;
            }
        }
    }
    panic!("never saw {want:?}; saw {seen:?}");
}

#[test]
fn ready_emits_power_off_and_charging() {
    let pad = Pad::new(0x03);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    wait_for(&rx, GamepadEvent::Battery(Some(Battery { pct: 90, charging: true })));
    assert_eq!(pad.data_types()[0], T_HELLO6, "the app hello first");
    drop(guard);
}

#[test]
fn charging_follows_replies() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::Battery(Some(Battery { pct: 90, charging: false })));
    pad.flags.store(0x03, Ordering::SeqCst);
    let seen = wait_for(&rx, GamepadEvent::Battery(Some(Battery { pct: 90, charging: true })));
    let batts = seen.iter().filter(|e| matches!(e, GamepadEvent::Battery(_))).count();
    assert_eq!(batts, 1, "one Battery per change: {seen:?}");
    drop(guard);
}

#[test]
fn no_link_falls_back_to_2a19_only() {
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(NoLink, level(90), fast(), &tx);
    std::thread::sleep(Duration::from_millis(200));
    let evs: Vec<_> = rx.try_iter().collect();
    assert_eq!(evs, vec![GamepadEvent::Battery(Some(b(90)))]);
    assert!(!guard.power_off());
    drop(guard);
}

#[test]
fn no_echo_falls_back() {
    let mut pad = Pad::new(0x03);
    pad.echo = false;
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    std::thread::sleep(Duration::from_millis(400));
    let evs: Vec<_> = rx.try_iter().collect();
    assert!(!evs.iter().any(|e| matches!(e, GamepadEvent::PowerOff(_))), "{evs:?}");
    assert!(evs.contains(&GamepadEvent::Battery(Some(b(90)))), "{evs:?}");
    assert_eq!(pad.link_hellos(), 3, "hello_tries link hellos, then give up");
    assert!(pad.data_types().is_empty(), "nothing sent without a link");
    assert!(!guard.power_off());
    drop(guard);
}

#[test]
fn power_off_writes_the_rst_frame() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    assert!(guard.power_off(), "ACKed");
    let w = pad.writes.lock().unwrap().clone();
    let rst = w.iter().rev().find(|f| f.len() > 2 && f[2] == T_RST).expect("rst frame").clone();
    assert_eq!((rst[0], rst[1], rst[4], rst[5]), (5, 4, 1, 0), "05 ch 24 seq 01 00: {rst:02x?}");
    let prev = w.iter().rev().filter(|f| f[0] == 5 && f[2] != T_RST).next().unwrap();
    assert_eq!(rst[3], prev[3].wrapping_add(4), "a new message: seq + 4");
    for ty in pad.data_types() {
        assert!([T_HELLO6, T_HELLO5, T_BATT, T_RST].contains(&ty), "type 0x{ty:02x} written");
    }
    drop(guard);
}

#[test]
fn power_off_not_ready_returns_false() {
    let (tx, _rx) = unbounded();
    let guard = nano_companion::start_with(NoLink, level(90), fast(), &tx);
    assert!(!guard.power_off());
    drop(guard);
}

#[test]
fn lost_session_sends_power_off_false() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    pad.answer_batt.store(false, Ordering::SeqCst);
    wait_for(&rx, GamepadEvent::PowerOff(false));
    assert!(!guard.power_off(), "lost: no power-off any more");
    drop(guard);
}

#[test]
fn drop_mid_request_emits_nothing_more() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    // The link stalls (a reply that takes its time) while the session ends.
    let held = pad.hold.lock().unwrap();
    pad.flags.store(0x03, Ordering::SeqCst);
    let t0 = Instant::now();
    drop(guard);
    assert!(t0.elapsed() < Duration::from_millis(200), "drop returns at once");
    drop(held);
    std::thread::sleep(Duration::from_millis(150));
    let late: Vec<_> = rx.try_iter().collect();
    assert!(late.iter().all(|e| !matches!(e, GamepadEvent::Battery(Some(Battery { charging: true, .. })))), "{late:?}");
    let n = pad.writes.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(pad.writes.lock().unwrap().len(), n, "no write after the session");
}

#[test]
fn drop_never_waits_for_a_write_in_flight() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    // The next battery request's write stalls.
    let held = pad.write_hold.lock().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        drop(guard);
        let _ = done_tx.send(());
    });
    let quick = done_rx.recv_timeout(Duration::from_millis(300)).is_ok();
    drop(held);
    assert!(quick, "drop waited for the write in flight (it delays the worker's Disconnected)");
    std::thread::sleep(Duration::from_millis(100));
    assert!(rx.try_iter().all(|e| !matches!(e, GamepadEvent::PowerOff(_) | GamepadEvent::Battery(_))), "nothing after the session");
}

#[test]
fn power_off_waits_for_a_slow_write() {
    let pad = Pad::new(0x01);
    let (tx, rx) = unbounded();
    let guard = nano_companion::start_with(Opens(Some(pad.clone())), level(90), fast(), &tx);
    wait_for(&rx, GamepadEvent::PowerOff(true));
    pad.slow_rst_ms.store(2000, Ordering::SeqCst);
    assert!(guard.power_off(), "a power-off written in 2 s is still answered, not given up on");
    drop(guard);
}
