//! v3.8.0: the Xbox Nano's battery over Bluetooth GATT (Battery Service
//! 0x180F, Battery Level 0x2A19). The WinRT read is Windows-only; these are
//! the pure parts with a fake reader: byte → `Battery`, which readings are
//! sent, the read schedule, and that nothing is sent once the session ends.

use crossbeam_channel::unbounded;
use dualsense_mapper::gamepad::nano_battery::{self, battery_from_level, LevelReader, Tracker};
use dualsense_mapper::gamepad::{Battery, GamepadEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn b(pct: u8) -> Battery {
    Battery { pct, charging: false }
}

#[test]
fn battery_level_byte_is_the_percentage_without_a_charging_flag() {
    assert_eq!(battery_from_level(&[0x46]), Some(b(70)), "the user's Nano: 0x46 = 70 %");
    assert_eq!(battery_from_level(&[0]), Some(b(0)));
    assert_eq!(battery_from_level(&[100]), Some(b(100)));
    assert_eq!(battery_from_level(&[101]), None, "101..=255 are reserved");
    assert_eq!(battery_from_level(&[0xff]), None);
    assert_eq!(battery_from_level(&[]), None, "empty read");
    assert_eq!(battery_from_level(&[55, 1, 2]), Some(b(55)), "only the first byte counts");
}

#[test]
fn tracker_sends_the_first_reading_and_changes_only() {
    let mut t = Tracker::default();
    assert_eq!(t.next(Some(b(70))), Some(Some(b(70))), "first reading");
    assert_eq!(t.next(Some(b(70))), None, "same reading");
    assert_eq!(t.next(Some(b(60))), Some(Some(b(60))), "changed");
}

#[test]
fn tracker_keeps_the_last_reading_over_one_failed_read_and_drops_it_after_two() {
    let mut t = Tracker::default();
    assert_eq!(t.next(None), None, "no reading yet: nothing to drop");
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

#[test]
fn reads_at_session_start_then_every_period_and_sends_changes() {
    let count = Arc::new(AtomicUsize::new(0));
    let reader = FakeReader { reads: vec![Some(vec![70]), Some(vec![70]), Some(vec![69])], count: count.clone() };
    let (tx, rx) = unbounded();
    let guard = nano_battery::start_with(reader, Duration::from_millis(40), &tx);
    // The first read happens at once, not after a period.
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
    let guard = nano_battery::start_with(SlowReader { gate: gate.clone() }, Duration::from_secs(60), &tx);
    drop(guard);
    assert!(t0.elapsed() < Duration::from_millis(200), "start and end return at once");
    // The read finishes after the session ended: its reading is not sent
    // (it would land after the worker's Disconnected).
    drop(held);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rx.try_iter().count(), 0);
}

#[test]
fn ending_the_session_stops_the_schedule() {
    let count = Arc::new(AtomicUsize::new(0));
    let reader = FakeReader { reads: vec![Some(vec![70])], count: count.clone() };
    let (tx, rx) = unbounded();
    let guard = nano_battery::start_with(reader, Duration::from_millis(20), &tx);
    rx.recv_timeout(Duration::from_millis(500)).expect("first reading");
    drop(guard);
    std::thread::sleep(Duration::from_millis(50));
    let after = count.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(count.load(Ordering::SeqCst), after, "no read after the session");
}

#[test]
fn no_address_or_not_windows_reads_nothing() {
    let (tx, rx) = unbounded();
    let guard = nano_battery::start(None, &tx);
    std::thread::sleep(Duration::from_millis(50));
    drop(guard);
    assert_eq!(rx.try_iter().count(), 0);
}
