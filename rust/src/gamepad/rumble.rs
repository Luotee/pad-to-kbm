//! Rumble-on-connect: pure frame builders and the buzz/stop timeline.
//!
//! No I/O here. The byte layouts are facts taken from the research file
//! docs (DualSense BT output report 0x31, Switch Pro output report 0x10,
//! Switch 2 Pro vibration characteristic); no code was copied from any
//! source. The workers own the writes and the clock; this module only says
//! what to send and when.

use std::time::{Duration, Instant};

use super::ControllerKind;

/// One action on the rumble timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Buzz,
    Stop,
}

/// A fixed buzz-then-stop timeline for one pad, driven by the caller's clock.
///
/// The caller writes at most one step per `take_due` and reports a failed
/// write with `on_write_failed`; a step counts as written once the next
/// call on the plan finds no such report.
#[derive(Debug)]
pub struct RumblePlan {
    kind: ControllerKind,
    start: Instant,
    steps: Vec<(u64, Step)>,
    /// Index of the first step neither taken nor skipped.
    next: usize,
    /// The step `take_due` returned last, not yet known to be written.
    taken: Option<Step>,
    /// A buzz was written (or the plan this one replaced left one running).
    buzzed: bool,
    /// A stop was written.
    stopped: bool,
}

impl RumblePlan {
    pub fn new(kind: ControllerKind, start: Instant) -> Self {
        let mut steps: Vec<(u64, Step)> = Vec::new();
        match kind {
            // Stop is sent twice: a lost stop would leave the pad buzzing.
            ControllerKind::DualSense => {
                steps.push((0, Step::Buzz));
                steps.extend([(200, Step::Stop), (230, Step::Stop)]);
            }
            // Writes stay >= 30 ms apart (Switch Pro drops faster ones).
            ControllerKind::NsPro => {
                steps.extend((0..4).map(|i| (i * 50, Step::Buzz)));
                steps.extend([(200, Step::Stop), (240, Step::Stop), (280, Step::Stop)]);
            }
            // The vibration characteristic wants a ~12 ms frame stream.
            ControllerKind::Ns2Pro => {
                steps.extend((0..17).map(|i| (i * 12, Step::Buzz)));
                steps.extend([(204, Step::Stop), (216, Step::Stop), (228, Step::Stop)]);
            }
        }
        Self { kind, start, steps, next: 0, taken: None, buzzed: false, stopped: false }
    }

    /// The new plan when a `Rumble` arrives while this one runs: the same
    /// timeline from `now`. A buzz this plan left running still owes a
    /// stop, so the new plan's `abort` sends one even before its own buzz.
    pub fn restarted(mut self, now: Instant) -> RumblePlan {
        self.settle();
        let mut p = RumblePlan::new(self.kind, now);
        p.buzzed = self.buzzed && !self.stopped;
        p
    }

    /// At most one step to write now, marked as taken. Of several due
    /// steps the first due stop wins (earlier buzzes are skipped); with no
    /// stop due, the latest due buzz (earlier ones are skipped). A stop is
    /// never skipped.
    pub fn take_due(&mut self, now: Instant) -> Option<Step> {
        self.settle();
        let due = self.steps[self.next..]
            .iter()
            .take_while(|&&(ms, _)| now >= self.start + Duration::from_millis(ms))
            .count();
        if due == 0 {
            return None;
        }
        let window = self.next..self.next + due;
        let pick = window
            .clone()
            .find(|&i| self.steps[i].1 == Step::Stop)
            .unwrap_or(window.end - 1);
        self.next = pick + 1;
        self.taken = Some(self.steps[pick].1);
        self.taken
    }

    /// The step `take_due` returned last was not written. The remaining
    /// buzzes are skipped; the stops keep their times (a failed stop
    /// leaves the next one).
    pub fn on_write_failed(&mut self) {
        self.taken = None;
        let next = self.next;
        let mut i = 0;
        self.steps.retain(|s| {
            i += 1;
            i <= next || s.1 == Step::Stop
        });
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.steps.get(self.next).map(|&(ms, _)| self.start + Duration::from_millis(ms))
    }

    pub fn finished(&self) -> bool {
        self.next >= self.steps.len()
    }

    /// Cut the timeline short. `Some(Stop)` if a buzz was written and no
    /// stop has been yet, so the caller must send one; the plan is
    /// finished after.
    pub fn abort(&mut self) -> Option<Step> {
        self.settle();
        self.next = self.steps.len();
        (self.buzzed && !self.stopped).then_some(Step::Stop)
    }

    /// The last taken step was not reported failed: it was written.
    fn settle(&mut self) {
        match self.taken.take() {
            Some(Step::Buzz) => self.buzzed = true,
            Some(Step::Stop) => self.stopped = true,
            None => {}
        }
    }
}

/// Both DualSense motors at 25 %: gentle but clearly felt.
pub const DS_BUZZ_MOTOR: u8 = 0x40;

/// DualSense BT output report 0x31 (78 bytes) that only touches the motors.
/// LED, audio and trigger flags stay 0 so those states are left alone.
pub fn ds_report(motor: u8, v2: bool) -> [u8; 78] {
    let mut r = [0u8; 78];
    r[0] = 0x31;
    r[2] = 0x10;
    // New firmware: HAPTICS_SELECT + VIBRATION2. Old: plus COMPATIBLE_VIBRATION.
    r[3] = if v2 { 0x02 } else { 0x03 };
    r[5] = motor;
    r[6] = motor;
    if v2 {
        r[41] = 0x04;
    }
    super::lightbar::seal(&mut r);
    r
}

/// Firmware "update version" is a u16 LE at bytes 44..45 of feature report
/// 0x20; v2 rumble from 0x0224. Unreadable or short counts as new.
pub fn ds_firmware_is_v2(feature_0x20: Option<&[u8]>) -> bool {
    match feature_0x20 {
        Some(b) if b.len() >= 46 => u16::from_le_bytes([b[44], b[45]]) >= 0x0224,
        _ => true,
    }
}

/// Switch Pro rumble payload for a moderate buzz (both motors).
pub const NS_BUZZ: [u8; 8] = [0x00, 0x75, 0x40, 0x5d, 0x00, 0x75, 0x40, 0x5d];

/// Switch Pro output report 0x10 (rumble only), padded to the 49-byte length.
pub fn ns_rumble_report(counter: u8, motors: [u8; 8]) -> [u8; 49] {
    let mut r = [0u8; 49];
    r[0] = 0x10;
    r[1] = counter & 0x0F;
    r[2..10].copy_from_slice(&motors);
    r
}

/// Switch 2 Pro vibration opcodes (5 bytes, repeated in both halves).
pub const NS2_BUZZ_OP: [u8; 5] = [0x87, 0x21, 0x23, 0x11, 0x32];
pub const NS2_STOP_OP: [u8; 5] = [0x87, 0x01, 0x20, 0x11, 0x00];

/// 42-byte write to the Switch 2 Pro vibration characteristic: the same
/// `0x50 | seq` + opcode record at offsets 0x01 and 0x11.
pub fn ns2_vibration_frame(seq: u8, op: [u8; 5]) -> [u8; 42] {
    let mut f = [0u8; 42];
    for at in [0x01usize, 0x11] {
        f[at] = 0x50 | (seq & 0x0F);
        f[at + 1..at + 6].copy_from_slice(&op);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::ControllerKind;
    use std::time::{Duration, Instant};

    fn crc(b: &[u8]) -> [u8; 4] { b[74..78].try_into().unwrap() }

    /// Drive a plan with a fake clock, collect (ms, step).
    fn run(kind: ControllerKind) -> Vec<(u64, Step)> {
        let t0 = Instant::now();
        let mut p = RumblePlan::new(kind, t0);
        let mut out = vec![];
        for ms in 0..400u64 {
            while let Some(s) = p.take_due(t0 + Duration::from_millis(ms)) { out.push((ms, s)); }
        }
        assert!(p.finished());
        out
    }

    #[test] fn ds_v2_buzz_matches_vector() {
        let r = ds_report(DS_BUZZ_MOTOR, true);
        assert_eq!((r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[41]), (0x31, 0, 0x10, 0x02, 0, 0x40, 0x40, 0x04));
        assert_eq!(crc(&r), [0xd6, 0x65, 0x7a, 0x40]);
    }
    #[test] fn ds_v2_stop_matches_vector() { assert_eq!(crc(&ds_report(0, true)), [0xe8, 0xf9, 0xdd, 0x97]); }
    #[test] fn ds_old_buzz_matches_vector() {
        let r = ds_report(DS_BUZZ_MOTOR, false);
        assert_eq!((r[3], r[5], r[6], r[41]), (0x03, 0x40, 0x40, 0x00)); // no halving in old mode
        assert_eq!(crc(&r), [0xf2, 0xf8, 0x89, 0x41]);
    }
    #[test] fn ds_old_stop_matches_vector() { assert_eq!(crc(&ds_report(0, false)), [0xcc, 0x64, 0x2e, 0x96]); }
    #[test] fn ds_report_leaves_leds_and_triggers_alone() {
        let r = ds_report(DS_BUZZ_MOTOR, true);
        assert!(r[7..41].iter().all(|&b| b == 0) && r[42..74].iter().all(|&b| b == 0) && r[4] == 0);
    }
    #[test] fn ds_firmware_threshold_follows_sdl() {
        let fw = |v: u16| { let mut b = vec![0u8; 64]; b[0] = 0x20; b[44..46].copy_from_slice(&v.to_le_bytes()); b };
        assert!(!ds_firmware_is_v2(Some(&fw(0x0223))));
        assert!(ds_firmware_is_v2(Some(&fw(0x0224))));
        assert!(ds_firmware_is_v2(None));
        assert!(ds_firmware_is_v2(Some(&[0x20; 40])));  // short read
    }
    #[test] fn ns_rumble_report_layout() {
        let r = ns_rumble_report(0x0B, NS_BUZZ);
        assert_eq!(&r[..10], &[0x10, 0x0B, 0x00, 0x75, 0x40, 0x5d, 0x00, 0x75, 0x40, 0x5d]);
        assert!(r[10..].iter().all(|&b| b == 0));
        assert_eq!(&ns_rumble_report(3, crate::gamepad::ns_protocol::NEUTRAL_RUMBLE)[2..10],
                   &[0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40]);
    }
    #[test] fn ns2_frame_matches_spike_dry_run() {
        let f = ns2_vibration_frame(1, NS2_BUZZ_OP);
        let mut want = [0u8; 42];
        want[0x01..0x07].copy_from_slice(&[0x51, 0x87, 0x21, 0x23, 0x11, 0x32]);
        want[0x11..0x17].copy_from_slice(&[0x51, 0x87, 0x21, 0x23, 0x11, 0x32]);
        assert_eq!(f, want);
        assert_eq!(ns2_vibration_frame(0x13, NS2_STOP_OP)[1], 0x53); // seq wraps into the low nibble
    }
    #[test] fn plan_timelines_fire_each_step_once() {
        use Step::*;
        assert_eq!(run(ControllerKind::DualSense), vec![(0, Buzz), (200, Stop), (230, Stop)]);
        assert_eq!(run(ControllerKind::NsPro),
            vec![(0, Buzz), (50, Buzz), (100, Buzz), (150, Buzz), (200, Stop), (240, Stop), (280, Stop)]);
        let ns2 = run(ControllerKind::Ns2Pro);
        assert_eq!(ns2.iter().filter(|s| s.1 == Buzz).count(), 17);
        assert_eq!(ns2[16], (192, Buzz));
        assert_eq!(&ns2[17..], &[(204, Stop), (216, Stop), (228, Stop)]);
    }
    #[test] fn ns_pro_steps_are_at_least_30ms_apart() {
        let steps = run(ControllerKind::NsPro);
        assert!(steps.len() > 1);
        for w in steps.windows(2) {
            assert!(w[1].0 - w[0].0 >= 30, "gap {} -> {} under 30 ms", w[0].0, w[1].0);
        }
    }
    #[test] fn abort_returns_stop_only_while_buzzing() {
        let t0 = Instant::now();
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.abort(), None);                       // nothing sent yet
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.take_due(t0), Some(Step::Buzz));     // Buzz sent
        assert_eq!(p.abort(), Some(Step::Stop));
        assert!(p.finished());
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        while p.take_due(t0 + Duration::from_millis(500)).is_some() {}
        assert_eq!(p.abort(), None);                       // already stopped
    }
    #[test] fn next_due_tracks_the_next_unsent_step() {
        let t0 = Instant::now();
        let mut p = RumblePlan::new(ControllerKind::NsPro, t0);
        assert_eq!(p.next_due(), Some(t0));
        assert_eq!(p.take_due(t0), Some(Step::Buzz));
        assert_eq!(p.next_due(), Some(t0 + Duration::from_millis(50)));
    }

    #[test] fn take_due_returns_one_step_per_call() {
        use Step::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // On time: one step per call, and nothing twice.
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        assert_eq!(p.take_due(at(0)), None);
        assert_eq!(p.take_due(at(230)), Some(Stop));
        assert_eq!(p.take_due(at(230)), Some(Stop));
        assert_eq!(p.take_due(at(230)), None);
        assert!(p.finished());
        // First poll at 300 ms: every step is due; the overdue buzz is
        // skipped and the two stops still come one per call.
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.take_due(at(300)), Some(Stop));
        assert_eq!(p.take_due(at(300)), Some(Stop));
        assert_eq!(p.take_due(at(300)), None);
        assert!(p.finished());
        assert_eq!(p.abort(), None, "the skipped buzz was never written");
    }
    #[test] fn take_due_collapses_overdue_buzzes_but_never_skips_a_stop() {
        use Step::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut p = RumblePlan::new(ControllerKind::Ns2Pro, t0);
        // First poll at 100 ms: buzzes 0..=96 are due, only one goes out.
        assert_eq!(p.take_due(at(100)), Some(Buzz));
        assert_eq!(p.take_due(at(100)), None, "the earlier due buzzes are skipped");
        assert_eq!(p.next_due(), Some(at(108)));
        // Poll at 210 ms: buzzes 108..=192 and Stop@204 are due → the stop.
        assert_eq!(p.take_due(at(210)), Some(Stop));
        assert_eq!(p.take_due(at(210)), None, "Stop@216 is not due yet");
        assert_eq!(p.take_due(at(216)), Some(Stop));
        // Long stall: the last stop still goes out.
        assert_eq!(p.take_due(at(500)), Some(Stop));
        assert_eq!(p.take_due(at(500)), None);
        assert!(p.finished());
        // A stall in the middle of the buzz: the latest due buzz goes out.
        let mut p = RumblePlan::new(ControllerKind::NsPro, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        assert_eq!(p.take_due(at(120)), Some(Buzz)); // Buzz@100; Buzz@50 skipped
        assert_eq!(p.next_due(), Some(at(150)));
    }
    #[test] fn failed_buzz_write_still_sends_the_stops() {
        use Step::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        p.on_write_failed();
        assert_eq!(p.take_due(at(199)), None);
        assert_eq!(p.take_due(at(200)), Some(Stop));
        assert_eq!(p.take_due(at(230)), Some(Stop));
        assert!(p.finished());
        // The remaining buzzes are skipped; the stops keep their times.
        let mut p = RumblePlan::new(ControllerKind::NsPro, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        assert_eq!(p.take_due(at(50)), Some(Buzz));
        p.on_write_failed();
        assert_eq!(p.next_due(), Some(at(200)));
        assert_eq!(p.take_due(at(150)), None, "no buzz after a failed write");
        assert_eq!(p.take_due(at(200)), Some(Stop));
        assert_eq!(p.abort(), None, "Stop@200 went out");
        // Buzz@0 went out before Buzz@50 failed: the pad still owes a stop.
        let mut p = RumblePlan::new(ControllerKind::NsPro, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        assert_eq!(p.take_due(at(50)), Some(Buzz));
        p.on_write_failed();
        assert_eq!(p.abort(), Some(Stop));
        // A failed buzz is not a written buzz.
        let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.take_due(at(0)), Some(Buzz));
        p.on_write_failed();
        assert_eq!(p.abort(), None);
    }
    #[test] fn failed_stop_moves_to_the_next_stop() {
        use Step::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let buzzed_then_failed_stop = || {
            let mut p = RumblePlan::new(ControllerKind::DualSense, t0);
            assert_eq!(p.take_due(at(0)), Some(Buzz));
            assert_eq!(p.take_due(at(200)), Some(Stop));
            p.on_write_failed();
            p
        };
        let mut p = buzzed_then_failed_stop();
        assert_eq!(p.take_due(at(229)), None);
        assert_eq!(p.take_due(at(230)), Some(Stop));
        assert!(p.finished());
        // A failed stop is not a written stop: the pad is still buzzing.
        assert_eq!(buzzed_then_failed_stop().abort(), Some(Stop));
        // Every stop failed: the plan ends anyway.
        let mut p = buzzed_then_failed_stop();
        assert_eq!(p.take_due(at(230)), Some(Stop));
        p.on_write_failed();
        assert!(p.finished());
    }
    #[test] fn restarted_plan_owes_the_stop_of_a_buzzing_plan() {
        use Step::*;
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut old = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(old.take_due(at(0)), Some(Buzz));
        let mut p = old.restarted(at(100));
        assert_eq!(p.abort(), Some(Stop), "the old buzz is still running");
        // The new timeline starts at the restart time.
        let mut old = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(old.take_due(at(0)), Some(Buzz));
        let mut p = old.restarted(at(100));
        assert_eq!(p.take_due(at(99)), None);
        assert_eq!(p.take_due(at(100)), Some(Buzz));
        assert_eq!(p.take_due(at(300)), Some(Stop));
        assert_eq!(p.abort(), None, "a stop went out");
        // Nothing owed: nothing written yet, or already stopped.
        let p = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(p.restarted(at(100)).abort(), None);
        let mut old = RumblePlan::new(ControllerKind::DualSense, t0);
        assert_eq!(old.take_due(at(0)), Some(Buzz));
        assert_eq!(old.take_due(at(200)), Some(Stop));
        assert_eq!(old.restarted(at(210)).abort(), None);
        // Restarted twice before the new buzz went out: still owed.
        let mut old = RumblePlan::new(ControllerKind::NsPro, t0);
        assert_eq!(old.take_due(at(0)), Some(Buzz));
        assert_eq!(old.restarted(at(10)).restarted(at(20)).abort(), Some(Stop));
    }
}
