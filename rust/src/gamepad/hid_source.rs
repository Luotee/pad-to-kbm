//! Raw Bluetooth HID source (DualSense 0x31, Switch Pro 0x30).
//!
//! Spawns a worker thread that owns the hidapi `HidDevice`, drives the
//! `Searching → Handshaking → Streaming` state machine, decodes each
//! report with the connected kind's decoder, diffs against the previous
//! snapshot, and pushes `GamepadEvent`s to a channel the engine drains
//! via `poll()`.
//!
//! Constructors:
//! - `new()`             — production hidapi worker
//! - `fake(rx)`          — direct GamepadEvent injection (engine tests)
//! - `new_from_byte_stream(rx)` / `new_from_byte_stream_for(kind, rx)` —
//!   raw report injection (state-machine tests)

use super::cursor_params::CursorParams;
use super::ds_protocol::{build_handshake_buffer, decode_31, REPORT_LEN_BT};
use super::ns_handshake::{ns_handshake_counted, send_input_mode_full, send_turn_off};
use super::ns_protocol::{decode_30, StickCal, NEUTRAL_RUMBLE};
use super::pad::{emit_pad_diff, PadState, BUTTON_SLOTS};
use super::rumble::{
    ds_firmware_is_v2, ds_report, ns_rumble_report, RumblePlan, Step as RumbleStep,
    DS_BUZZ_MOTOR, NS_BUZZ,
};
use super::controller::ControllerKind;
use super::events::GamepadEvent;
use super::lightbar::{
    ds_lightbar_report, ds_lightbar_setup_report, LightWrite, LightbarLive, LightbarPlayer,
};
use super::SourceCmd;
use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Per-frame raw-coordinate delta upper bound. A real finger at 250 Hz
/// frame rate over the 1920-wide pad cannot move more than ~150 raw px
/// in one frame. Larger jumps come from the DualSense reporting one
/// stale (pre-touch) frame at touch-down — the very first contact
/// frame can carry the x/y of the previous touch even though `active`
/// has flipped to true. We re-anchor on these without emitting so the
/// cursor stays still rather than teleporting halfway across the screen.
const CURSOR_TELEPORT_GUARD: i32 = 250;
/// Button ids 25..=28 = Touchpad TL/TR/BL/BR. Matches the touchpad rows
/// of `config::ADDED_IDS`; duplicated here to avoid pulling the config
/// crate into the gamepad layer.
const QUAD_TL: u32 = 25;
const QUAD_TR: u32 = 26;
const QUAD_BL: u32 = 27;
const QUAD_BR: u32 = 28;
/// Sentinel value emitted in `GamepadEvent::TouchpadHover.quadrant`
/// when the finger lifts. Out of band — valid quadrant ids are 25..=28.
const HOVER_QUADRANT_NONE: u32 = 255;

/// Bug 1: time window after click rising edge during which cursor
/// delta is suppressed to mask finger-press lateral roll. Matches
/// libinput tap timeout order of magnitude (~100ms). Tuned conservatively
/// short so press-and-drag becomes responsive after the press settles.
const CLICK_FREEZE_MS: u64 = 80;
/// Bug 1: cumulative raw-pixel motion threshold from click anchor.
/// If finger moves > this from anchor during the freeze window, user
/// intent is dragging — exit freeze and let cursor follow. 15 raw px
/// ≈ 1% of touchpad width, well above press-roll drift (5-10 px).
const CLICK_DRAG_EXIT_PX: u32 = 15;
/// Bug 2: rolling position buffer depth (frames). 4ms/frame × 10 = 40ms,
/// enough to reach back to a stable pre-press position (~30ms target).
const POS_BUFFER_DEPTH: usize = 10;
/// Bug 2: target lookback for click quadrant pos — pick the buffer entry
/// closest to (now - this) when the click rising edge fires.
const CLICK_POS_LOOKBACK_MS: u64 = 30;

/// Per-worker touchpad tracking. Reset on each new device connect.
#[derive(Default)]
struct TouchpadState {
    last_finger_pos: Option<(u16, u16)>,
    /// Position recorded at the moment finger 0 first became active in
    /// the current contact. The quadrant for the touchpad click uses
    /// this — the user's *intent* is where they put their finger, not
    /// where the finger has drifted to by the time the physical click
    /// switch fires (the press motion shifts the contact point).
    touchdown_pos: Option<(u16, u16)>,
    last_click_quadrant: Option<u32>,
    prev_touchpad_btn: bool,
    /// Mirrors current frame's touchpad button state (buf[11] bit 1).
    /// Updated every frame in `process_touchpad`. Read by the cursor
    /// filter so L1 (click-freeze) can suppress cursor deltas while
    /// the button is physically held.
    click_btn_held: bool,
    /// Rolling buffer of last 3 frames' squared motion magnitudes,
    /// used by L2 (stationary deadzone). Populated by the filter; the
    /// state outlives a single frame so consecutive sub-radius motion
    /// is suppressed only after the radius has been crossed back.
    recent_mag_sq: std::collections::VecDeque<u32>,
    /// Last quadrant the finger 0 was over (None if not currently in
    /// any quadrant — i.e., finger lifted). Used to dedupe per-frame
    /// hover emits so only quadrant CHANGES produce events.
    last_hover_quadrant: Option<u32>,
    /// Time the touchpad button transitioned false→true (Bug 1: click drift).
    /// `None` = button currently up. Set on rising edge.
    click_btn_down_at: Option<Instant>,
    /// Finger position captured at click rising edge (Bug 1).
    /// Cumulative motion from this anchor gates the L1 freeze early-exit.
    click_anchor_pos: Option<(u16, u16)>,
    /// Rolling buffer of recent (x, y, timestamp) for stable pre-press
    /// position lookup. Bug 2: click quadrant uses entry from ~30ms ago,
    /// not the live finger position (which has press-drift) and not
    /// `touchdown_pos` (which is stale across multi-click drag sessions).
    pos_buffer: std::collections::VecDeque<(u16, u16, Instant)>,
}

fn quadrant_for(x: u16, y: u16, mid_x: u16, mid_y: u16) -> u32 {
    match (x < mid_x, y < mid_y) {
        (true,  true)  => QUAD_TL,
        (false, true)  => QUAD_TR,
        (true,  false) => QUAD_BL,
        (false, false) => QUAD_BR,
    }
}

/// Per-frame hover quadrant emit with dedupe-on-change.
///
/// While the finger is active, emits `TouchpadHover` only when the
/// quadrant computed from the current (raw_x, raw_y) differs from the
/// last emitted quadrant. On finger lift, emits a sentinel
/// `TouchpadHover { quadrant: 255, raw_x: 0, raw_y: 0 }` to signal
/// "clear hover".
///
/// Uses the same `quadrant_for` axis-rect logic as click handling so
/// hover preview and click outcome are guaranteed consistent.
fn process_touchpad_hover<F: FnMut(GamepadEvent)>(
    finger_active: bool,
    cur_x: u16,
    cur_y: u16,
    state: &mut TouchpadState,
    params: &CursorParams,
    mut emit: F,
) {
    if finger_active {
        let q = quadrant_for(cur_x, cur_y, params.midpoint_x(), params.midpoint_y());
        if state.last_hover_quadrant != Some(q) {
            state.last_hover_quadrant = Some(q);
            emit(GamepadEvent::TouchpadHover { raw_x: cur_x, raw_y: cur_y, quadrant: q });
        }
    } else if state.last_hover_quadrant.is_some() {
        state.last_hover_quadrant = None;
        emit(GamepadEvent::TouchpadHover { raw_x: 0, raw_y: 0, quadrant: HOVER_QUADRANT_NONE });
    }
}

/// Three-layer cursor delta filter for the DualSense touchpad. Each
/// frame's raw motion is passed through:
///   L1 — Click freeze: while the user is physically holding the
///        touchpad button, suppress all cursor motion. Matches the
///        Synaptics PalmCheck / libinput thumb-detect behaviour for
///        Clickpads. Avoids the 5–10 px lateral drift caused by the
///        finger rolling forward as the user presses the button.
///   L2 — Stationary deadzone (added in Task 9).
///   L3 — Acceleration curve     (added in Task 10).
///
/// Returns the filtered (dx, dy) to emit, or `None` to suppress.
pub(crate) fn filter_cursor_delta(
    raw_dx: i32,
    raw_dy: i32,
    state: &mut TouchpadState,
    params: &CursorParams,
) -> Option<(i32, i32)> {
    // L1: motion-gated click freeze. Mask press-roll lateral drift in
    // the first CLICK_FREEZE_MS window OR until cumulative motion from
    // the click anchor exceeds CLICK_DRAG_EXIT_PX (drag detected).
    // After either gate exits, cursor follows finger normally so
    // press-and-drag works.
    if state.click_btn_held
        && params.click_freeze_enabled()
        && state.click_btn_down_at.is_some()
        && state.click_anchor_pos.is_some()
    {
        let down_at = state.click_btn_down_at.unwrap();
        let (anchor_x, anchor_y) = state.click_anchor_pos.unwrap();
        let elapsed_ms = down_at.elapsed().as_millis() as u64;
        // Cumulative motion from anchor — computed via the pos_buffer's
        // latest entry (updated BEFORE filter_cursor_delta is called).
        let cur_pos = state.pos_buffer.back().map(|(x, y, _)| (*x, *y));
        let cum_mag_sq = if let Some((cx, cy)) = cur_pos {
            let ddx = (cx as i32) - (anchor_x as i32);
            let ddy = (cy as i32) - (anchor_y as i32);
            (ddx * ddx + ddy * ddy) as u32
        } else {
            0
        };
        let exit_sq = CLICK_DRAG_EXIT_PX * CLICK_DRAG_EXIT_PX;
        if elapsed_ms < CLICK_FREEZE_MS && cum_mag_sq < exit_sq {
            return None;
        }
    }
    // L2: stationary deadzone — rolling 3-frame magnitude window
    let mag_sq = (raw_dx * raw_dx + raw_dy * raw_dy) as u32;
    state.recent_mag_sq.push_back(mag_sq);
    if state.recent_mag_sq.len() > 3 {
        state.recent_mag_sq.pop_front();
    }
    let dz = params.deadzone_radius();
    let dz_sq = (dz * dz) as u32;
    if state.recent_mag_sq.iter().all(|m| *m < dz_sq) {
        return None;
    }
    // L3: acceleration curve — linear interp gain between slow / fast thresholds
    let mag_px = (mag_sq as f32).sqrt();
    let slow = params.accel_slow_threshold() as f32;
    let fast = params.accel_fast_threshold() as f32;
    let g_slow = params.accel_gain_slow();
    let g_fast = params.accel_gain_fast();
    let gain = if mag_px < slow {
        g_slow
    } else if mag_px > fast {
        g_fast
    } else {
        let t = (mag_px - slow) / (fast - slow);
        g_slow + t * (g_fast - g_slow)
    };
    let sens = params.sensitivity();
    let total = sens * gain;
    Some(((raw_dx as f32 * total) as i32, (raw_dy as f32 * total) as i32))
}

/// Push current finger position into the rolling buffer; pop the oldest
/// entry if at depth. Called every frame in `process_touchpad`.
fn record_finger_pos(state: &mut TouchpadState, x: u16, y: u16, now: Instant) {
    state.pos_buffer.push_back((x, y, now));
    if state.pos_buffer.len() > POS_BUFFER_DEPTH {
        state.pos_buffer.pop_front();
    }
}

/// Look up the buffered position closest to `target = now - lookback`.
/// Returns the entry whose timestamp is nearest the target. Falls back
/// to the oldest available entry if the buffer is shorter than lookback.
fn stable_pos_for_click(
    state: &TouchpadState,
    now: Instant,
    lookback: Duration,
) -> Option<(u16, u16)> {
    let target = now.checked_sub(lookback)?;
    let mut best: Option<&(u16, u16, Instant)> = None;
    let mut best_diff_ms = u128::MAX;
    for entry in state.pos_buffer.iter() {
        let diff_ms = if entry.2 > target {
            (entry.2 - target).as_millis()
        } else {
            (target - entry.2).as_millis()
        };
        if diff_ms < best_diff_ms {
            best_diff_ms = diff_ms;
            best = Some(entry);
        }
    }
    best.map(|(x, y, _)| (*x, *y))
}

/// Cursor delta + touchpad click → 4-quadrant button events. Mutates
/// `state` across frames. Called per decoded `PadState`.
fn process_touchpad(
    state: &mut TouchpadState,
    cur: &PadState,
    params: &CursorParams,
    tx: &Sender<GamepadEvent>,
) {
    let now = Instant::now();
    // Update rolling position buffer before any filter/click logic so
    // filter_cursor_delta and stable_pos_for_click see the latest data.
    if cur.finger0_active {
        record_finger_pos(state, cur.finger0_x, cur.finger0_y, now);
    } else {
        state.pos_buffer.clear();
    }

    // Click button state for L1 freeze — set every frame so the filter
    // sees the current button state, not the previous frame's.
    state.click_btn_held = cur.touchpad_btn;
    // Cursor: relative motion from the previous finger 0 position.
    if cur.finger0_active {
        match state.last_finger_pos {
            None => {
                // Touch-down — record the start position without emitting.
                state.last_finger_pos = Some((cur.finger0_x, cur.finger0_y));
                state.touchdown_pos = Some((cur.finger0_x, cur.finger0_y));
            }
            Some((lx, ly)) => {
                let dx_raw = cur.finger0_x as i32 - lx as i32;
                let dy_raw = cur.finger0_y as i32 - ly as i32;
                if dx_raw.abs() > CURSOR_TELEPORT_GUARD
                    || dy_raw.abs() > CURSOR_TELEPORT_GUARD
                {
                    // Stale touch-down frame — re-anchor silently. The
                    // previous touchdown was garbage; replace it with the
                    // new real coordinates so the next click sees the
                    // settled position.
                    state.last_finger_pos = Some((cur.finger0_x, cur.finger0_y));
                    state.touchdown_pos = Some((cur.finger0_x, cur.finger0_y));
                } else {
                    // Always advance the anchor so deltas don't accumulate stale,
                    // but only emit if the 3-layer filter (freeze/deadzone/curve)
                    // approves the frame. Filter handles sensitivity scaling too.
                    state.last_finger_pos = Some((cur.finger0_x, cur.finger0_y));
                    // touchdown_pos intentionally NOT updated on motion — it
                    // represents the user's intent at touch-down.
                    if params.enabled() {
                        if let Some((dx, dy)) = filter_cursor_delta(dx_raw, dy_raw, state, params) {
                            let _ = tx.send(GamepadEvent::MouseDelta { dx, dy });
                        }
                    }
                }
            }
        }
    } else {
        // Finger lifted — clear references so the next touch-down
        // doesn't synthesise a jump from the old position.
        state.last_finger_pos = None;
        state.touchdown_pos = None;
    }

    // Hover preview — emit per-frame quadrant change for live UI feedback.
    process_touchpad_hover(
        cur.finger0_active,
        cur.finger0_x,
        cur.finger0_y,
        state,
        params,
        |ev| { let _ = tx.send(ev); },
    );

    // Click: rising edge captures the quadrant using a stable pre-press
    // position (~30ms ago) to avoid press-drift and stale touchdown_pos.
    // Falling edge releases the same id (so a drag across quadrant
    // boundaries does not re-emit).
    if cur.touchpad_btn && !state.prev_touchpad_btn {
        // Bug 1: record press timestamp + anchor for the motion-gate freeze.
        state.click_btn_down_at = Some(now);
        state.click_anchor_pos = if cur.finger0_active {
            Some((cur.finger0_x, cur.finger0_y))
        } else {
            None
        };
        // Bug 2: click quadrant uses stable position from ~30ms ago,
        // NOT touchdown_pos (which is stale across multi-click drags).
        let stable = stable_pos_for_click(
            state,
            now,
            Duration::from_millis(CLICK_POS_LOOKBACK_MS),
        );
        let click_pos = stable
            .or(state.click_anchor_pos)
            .or(state.touchdown_pos)
            .or_else(|| if cur.finger0_active {
                Some((cur.finger0_x, cur.finger0_y))
            } else {
                None
            });
        let mid_x = params.midpoint_x();
        let mid_y = params.midpoint_y();
        let (raw_x, raw_y) = click_pos.unwrap_or((mid_x, mid_y));
        let q = quadrant_for(raw_x, raw_y, mid_x, mid_y);
        tracing::info!(
            x = raw_x,
            y = raw_y,
            mid_x,
            mid_y,
            quadrant = q,
            "touchpad click captured (stable pre-press)"
        );
        state.last_click_quadrant = Some(q);
        let _ = tx.send(GamepadEvent::TouchpadClick { raw_x, raw_y, quadrant: q });
        let _ = tx.send(GamepadEvent::ButtonDown(q));
    } else if !cur.touchpad_btn && state.prev_touchpad_btn {
        // Click falling edge — clear freeze state.
        state.click_btn_down_at = None;
        state.click_anchor_pos = None;
        if let Some(q) = state.last_click_quadrant.take() {
            let _ = tx.send(GamepadEvent::ButtonUp(q));
        }
    }
    state.prev_touchpad_btn = cur.touchpad_btn;
}

/// Supported HID devices: (vendor id, product id, controller kind).
/// 0x054c / 0x0ce6 = Sony DualSense (standard PS5 pad).
/// 0x057e / 0x2009 = Nintendo Switch Pro Controller.
const SUPPORTED_HID: &[(u16, u16, ControllerKind)] = &[
    (0x054c, 0x0ce6, ControllerKind::DualSense),
    (0x057e, 0x2009, ControllerKind::NsPro),
];

/// Whether a HID device is one this worker can drive, and as which kind.
/// The DualSense matches on any bus (unchanged from v2.4.0). The Switch
/// Pro matches only on Bluetooth: over USB it needs a separate 0x80
/// handshake (SDL `BTrySetupUSB`) we don't send, so it would never stream
/// and would hold the worker forever. `BusType::Unknown` (hidapi could not
/// classify the parent devnode) counts as not Bluetooth.
fn is_candidate(vid: u16, pid: u16, bus: hidapi::BusType) -> Option<ControllerKind> {
    let kind = SUPPORTED_HID
        .iter()
        .find(|(v, p, _)| vid == *v && pid == *p)
        .map(|(_, _, kind)| *kind)?;
    match kind {
        ControllerKind::NsPro if !matches!(bus, hidapi::BusType::Bluetooth) => None,
        _ => Some(kind),
    }
}

/// First device in enumeration order that `is_candidate` accepts.
/// Non-candidates are skipped, so a USB Switch Pro listed before a
/// Bluetooth DualSense doesn't hide the DualSense.
fn first_candidate<T>(
    mut devices: impl Iterator<Item = T>,
    ids: impl Fn(&T) -> (u16, u16, hidapi::BusType),
) -> Option<(T, ControllerKind)> {
    devices.find_map(|d| {
        let (vid, pid, bus) = ids(&d);
        is_candidate(vid, pid, bus).map(|kind| (d, kind))
    })
}

/// Sleep between enumeration attempts in Searching state.
const ENUM_RETRY_INTERVAL: Duration = Duration::from_millis(1000);

/// Back-off before the next enumeration after a device session ends.
/// A session that never decoded a report and ended by timeout means an
/// opened device that doesn't stream; wait `ENUM_RETRY_INTERVAL` before
/// re-opening it instead of looping on it.
fn backoff_after(outcome: &str, connected: bool) -> Option<Duration> {
    (outcome == "timeout" && !connected).then_some(ENUM_RETRY_INTERVAL)
}

/// Blocking read timeout per HID frame. 4 ms matches BT 250 Hz cadence.
const READ_TIMEOUT_MS: i32 = 4;
/// No decoded report for this long → the DualSense is gone. Matches the
/// shipped v2.4 behaviour: 50 × read_timeout(4) ≈ 780 ms on Windows due to
/// the 15.6 ms timer tick.
const DS_DISCONNECT_AFTER: Duration = Duration::from_millis(800);
/// No decoded report for this long → the Switch Pro is gone (~60 Hz stream).
const NS_DISCONNECT_AFTER: Duration = Duration::from_millis(1000);
/// Switch Pro simple-HID input report: the pad left full-report mode.
const NS_REPORT_SIMPLE: u8 = 0x3F;
/// At most one `0x03 0x30` re-init per this interval.
const NS_REINIT_INTERVAL: Duration = Duration::from_millis(1000);

/// Minimal HID device I/O, so the Switch Pro handshake and the read loop
/// run against a fake device in tests.
pub trait HidIo {
    fn write(&self, data: &[u8]) -> Result<usize>;
    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize>;
}

impl HidIo for hidapi::HidDevice {
    fn write(&self, data: &[u8]) -> Result<usize> {
        Ok(hidapi::HidDevice::write(self, data)?)
    }

    fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize> {
        Ok(hidapi::HidDevice::read_timeout(self, buf, ms)?)
    }
}

/// Switch Pro drops output reports that come faster than this; every
/// output write on a session (rumble, 0x3F re-init) keeps this gap.
const NS_MIN_WRITE_GAP: Duration = Duration::from_millis(30);

/// Per-device-session protocol state: the kind's decoder, the Switch Pro
/// stick calibration and subcommand counter (one per session), the time of
/// the last 0x3F re-init and of the last output write, the DualSense
/// rumble flag set (firmware dependent) and the DualSense lightbar.
struct Session {
    kind: ControllerKind,
    cal: StickCal,
    ns_counter: u8,
    last_reinit: Option<Instant>,
    ds_rumble_v2: bool,
    last_write: Option<Instant>,
    /// DualSense only: the live settings and this session's scheduler.
    lightbar: Option<(LightbarLive, LightbarPlayer)>,
    /// A lightbar write failure was already logged this session.
    lightbar_warned: bool,
}

impl Session {
    fn new(kind: ControllerKind) -> Self {
        Session {
            kind,
            cal: StickCal::default(),
            ns_counter: 0,
            last_reinit: None,
            ds_rumble_v2: true,
            last_write: None,
            lightbar: None,
            lightbar_warned: false,
        }
    }

    /// Give a DualSense session its lightbar scheduler; other kinds have no
    /// lightbar this worker drives.
    fn attach_lightbar(&mut self, live: &LightbarLive) {
        if self.kind == ControllerKind::DualSense {
            self.lightbar = Some((live.clone(), LightbarPlayer::new(Instant::now())));
        }
    }

    fn decode(&self, frame: &[u8]) -> Option<PadState> {
        match self.kind {
            ControllerKind::DualSense => decode_31(frame),
            ControllerKind::NsPro => decode_30(frame, &self.cal),
            // Not in SUPPORTED_HID.
            ControllerKind::Ns2Pro => None,
        }
    }

    fn disconnect_after(&self) -> Duration {
        match self.kind {
            ControllerKind::DualSense => DS_DISCONNECT_AFTER,
            ControllerKind::NsPro => NS_DISCONNECT_AFTER,
            // Never opened by this worker (not in SUPPORTED_HID, `decode`
            // returns None). PR 3 sets its real deadline; until then it
            // shares the Switch Pro value.
            ControllerKind::Ns2Pro => NS_DISCONNECT_AFTER,
        }
    }

    /// Switch Pro only: a 0x3F report while streaming means the pad fell
    /// back to simple-HID mode; re-send `0x03 0x30`, rate-limited.
    fn on_simple_report(&mut self, d: &dyn HidIo) {
        let now = Instant::now();
        if self.last_reinit.is_some_and(|t| now.duration_since(t) < NS_REINIT_INTERVAL) {
            return;
        }
        // Too close to the last write; the pad keeps sending 0x3F, so a
        // later report retries.
        if self.too_soon_to_write(now) {
            return;
        }
        self.last_reinit = Some(now);
        tracing::info!("Switch Pro sent 0x3F; re-sending input mode 0x30");
        send_input_mode_full(d, &mut self.ns_counter);
        self.last_write = Some(Instant::now());
    }

    /// Switch Pro only: whether a write at `now` would come within
    /// `NS_MIN_WRITE_GAP` of the last one.
    fn too_soon_to_write(&self, now: Instant) -> bool {
        self.kind == ControllerKind::NsPro
            && self.last_write.is_some_and(|t| now.saturating_duration_since(t) < NS_MIN_WRITE_GAP)
    }

    /// Sleep out the rest of the Switch Pro write gap (no-op otherwise).
    fn wait_write_gap(&self) {
        if let Some(t) = self.last_write.filter(|_| self.kind == ControllerKind::NsPro) {
            thread::sleep(NS_MIN_WRITE_GAP.saturating_sub(t.elapsed()));
        }
    }

    /// Write one rumble step in this kind's output report.
    fn write_rumble(&mut self, d: &dyn HidIo, step: RumbleStep) -> Result<()> {
        let buzz = step == RumbleStep::Buzz;
        let res = match self.kind {
            ControllerKind::DualSense => {
                d.write(&ds_report(if buzz { DS_BUZZ_MOTOR } else { 0 }, self.ds_rumble_v2))
            }
            ControllerKind::NsPro => {
                let counter = self.ns_counter;
                self.ns_counter = (counter + 1) & 0x0F;
                d.write(&ns_rumble_report(counter, if buzz { NS_BUZZ } else { NEUTRAL_RUMBLE }))
            }
            // Not in SUPPORTED_HID; the BLE worker plays its rumble.
            ControllerKind::Ns2Pro => return Ok(()),
        };
        self.last_write = Some(Instant::now());
        res.map(|_| ())
    }
}

/// Write at most one due rumble step. On a Switch Pro a step that would
/// land within `NS_MIN_WRITE_GAP` of the last write waits for a later loop
/// iteration (the 4 ms read timeout bounds the delay). A failed write skips
/// the rest of the buzz; the stops are still tried. Returns whether a step
/// was written (or tried) this iteration.
fn play_rumble(d: &dyn HidIo, session: &mut Session, plan: &mut Option<RumblePlan>) -> bool {
    let Some(p) = plan.as_mut() else { return false };
    if session.too_soon_to_write(Instant::now()) {
        return false;
    }
    let mut wrote = false;
    if let Some(step) = p.take_due(Instant::now()) {
        wrote = true;
        if let Err(e) = session.write_rumble(d, step) {
            // Read errors decide liveness; the session carries on.
            tracing::warn!(error = %e, ?step, "rumble write failed; dropping it");
            p.on_write_failed();
        }
    }
    if p.finished() {
        tracing::info!(kind = ?session.kind, "rumble: done");
        *plan = None;
    }
    wrote
}

/// Write at most one due lightbar report (DualSense sessions only). An
/// iteration that wrote a rumble step belongs to the rumble. A failed write
/// is offered again after a backoff (`LightbarPlayer::on_write_failed`) and
/// never ends the session (iron rule 11); it is logged once per session.
fn play_lightbar(d: &dyn HidIo, session: &mut Session, wrote_rumble: bool) {
    if wrote_rumble {
        return;
    }
    let Some((live, player)) = session.lightbar.as_mut() else { return };
    let (cfg, kbm) = live.snapshot();
    let Some(w) = player.take_due(Instant::now(), &cfg, kbm) else { return };
    let report = match w {
        LightWrite::Setup => ds_lightbar_setup_report(),
        LightWrite::Color(c) => ds_lightbar_report(c),
    };
    if let Err(e) = d.write(&report) {
        // After the write returned: a failing write can block for a second.
        player.on_write_failed(Instant::now());
        if !session.lightbar_warned {
            session.lightbar_warned = true;
            tracing::warn!(error = %e, "lightbar write failed; retrying (logged once per session)");
        }
    }
}

/// The app is closing: leave the exit frame on the bar, best effort.
fn write_exit_frame(d: &dyn HidIo, session: &Session) {
    let Some((live, player)) = session.lightbar.as_ref() else { return };
    let (cfg, _) = live.snapshot();
    if let Some(c) = player.exit_frame(Instant::now(), &cfg) {
        let _ = d.write(&ds_lightbar_report(c));
    }
}

/// The session is ending on purpose (manual disconnect, app stop): a pad
/// mid-buzz gets one stop first, best effort, or it keeps vibrating.
fn stop_rumble(d: &dyn HidIo, session: &mut Session, plan: &mut Option<RumblePlan>) {
    if let Some(step) = plan.take().and_then(|mut p| p.abort()) {
        session.wait_write_gap();
        let _ = session.write_rumble(d, step);
        tracing::info!(kind = ?session.kind, "rumble: stopped early");
    }
}

/// How long dropping the production source waits for its worker to finish
/// (and write the lightbar's exit frame).
const SHUTDOWN_WAIT: Duration = Duration::from_millis(200);

pub struct HidSource {
    rx: Receiver<GamepadEvent>,
    stop: Arc<AtomicBool>,
    /// Production worker only: signalled just before the worker thread exits.
    done_rx: Option<Receiver<()>>,
}

impl HidSource {
    /// Spawn the production hidapi worker. The worker thread starts in
    /// `Searching` and emits no events until the first decoded report.
    /// `params` carries the live touchpad cursor sensitivity and on/off
    /// flag; the engine mutates them through `set_settings`.
    /// `lightbar` carries the live lightbar settings for DualSense sessions.
    /// `cmd_rx` receives the hub's commands: manual disconnect from the GUI,
    /// and rumble requests.
    pub fn new(
        params: CursorParams,
        lightbar: LightbarLive,
        cmd_rx: crossbeam_channel::Receiver<SourceCmd>,
    ) -> Result<Self> {
        let (tx, rx) = unbounded::<GamepadEvent>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
        thread::Builder::new()
            .name("dualsense-hid".into())
            .spawn(move || {
                worker_real(tx, stop_for_thread, params, lightbar, cmd_rx);
                let _ = done_tx.send(());
            })
            .map_err(|e| anyhow!("spawning HID worker thread: {e}"))?;
        Ok(Self { rx, stop, done_rx: Some(done_rx) })
    }

    /// Test-only constructor for engine integration tests. Drains
    /// `GamepadEvent`s directly from the supplied channel — no HID
    /// pipeline, no state machine.
    #[doc(hidden)]
    pub fn fake(rx: Receiver<GamepadEvent>) -> Self {
        Self { rx, stop: Arc::new(AtomicBool::new(false)), done_rx: None }
    }

    /// Test-only constructor for state-machine tests. Decodes
    /// `Vec<u8>` frames as if they came from hidapi and runs the full
    /// diff pipeline. Uses default cursor params (1.5, enabled=true).
    #[doc(hidden)]
    pub fn new_from_byte_stream(byte_rx: Receiver<Vec<u8>>) -> Self {
        Self::new_from_byte_stream_with_params(byte_rx, CursorParams::default())
    }

    /// Same as `new_from_byte_stream` but with explicit cursor params.
    #[doc(hidden)]
    pub fn new_from_byte_stream_with_params(
        byte_rx: Receiver<Vec<u8>>,
        params: CursorParams,
    ) -> Self {
        Self::spawn_byte_stream(ControllerKind::DualSense, byte_rx, params)
    }

    /// Byte-stream test worker for any kind: decodes with that kind's
    /// decoder (Switch Pro with default stick calibration). No handshake,
    /// no writes.
    #[doc(hidden)]
    pub fn new_from_byte_stream_for(kind: ControllerKind, byte_rx: Receiver<Vec<u8>>) -> Self {
        Self::spawn_byte_stream(kind, byte_rx, CursorParams::default())
    }

    fn spawn_byte_stream(
        kind: ControllerKind,
        byte_rx: Receiver<Vec<u8>>,
        params: CursorParams,
    ) -> Self {
        let (tx, rx) = unbounded::<GamepadEvent>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let (dummy_tx, dummy_rx) = crossbeam_channel::unbounded::<()>();
        thread::Builder::new()
            .name("dualsense-hid-fake".into())
            // Move dummy_tx into the closure so the disconnect channel's sender
            // stays alive for the lifetime of the worker thread. Without this,
            // the sender would be dropped immediately and the closed channel
            // would be selected on every iteration.
            .spawn(move || {
                let _keep_alive = dummy_tx;
                worker_byte_stream(kind, byte_rx, tx, stop_for_thread, params, dummy_rx)
            })
            .expect("spawning fake HID worker");
        Self { rx, stop, done_rx: None }
    }

    /// Drain pending events into `out`. Non-blocking.
    pub fn poll(&mut self, out: &mut Vec<GamepadEvent>) {
        while let Ok(ev) = self.rx.try_recv() {
            out.push(ev);
        }
    }
}

impl Drop for HidSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Give the worker a moment to write the exit frame; never hang.
        if let Some(done) = &self.done_rx {
            let _ = done.recv_timeout(SHUTDOWN_WAIT);
        }
    }
}

fn worker_real(
    tx: Sender<GamepadEvent>,
    stop: Arc<AtomicBool>,
    params: CursorParams,
    lightbar: LightbarLive,
    cmd_rx: crossbeam_channel::Receiver<SourceCmd>,
) {
    let mut api = match hidapi::HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "hidapi init failed");
            return;
        }
    };
    while !stop.load(Ordering::SeqCst) {
        // device_list() returns a cached snapshot — must refresh
        // explicitly each iteration or a pad turned on after app
        // startup is invisible forever.
        if let Err(e) = api.refresh_devices() {
            tracing::warn!(error = %e, "hidapi refresh_devices failed");
        }
        let device = first_candidate(api.device_list(), |info| {
            (info.vendor_id(), info.product_id(), info.bus_type())
        })
        .and_then(|(info, kind)| info.open_device(&api).ok().map(|d| (d, kind)));
        match device {
            Some((d, kind)) => {
                tracing::info!("{} opened, attempting handshake", kind.display_name());
                // A Rumble or Disconnect queued for an earlier session must
                // not act on this one.
                drain_commands(&cmd_rx);
                let mut session = Session::new(kind);
                match kind {
                    ControllerKind::DualSense => {
                        let _ = try_handshake(&d);
                        session.ds_rumble_v2 = ds_firmware_is_v2(read_ds_firmware(&d).as_deref());
                        tracing::info!(v2 = session.ds_rumble_v2, "DualSense rumble mode");
                    }
                    ControllerKind::NsPro => {
                        session.cal = ns_handshake_counted(
                            &d,
                            &mut |t: Duration| thread::sleep(t),
                            &mut session.ns_counter,
                        );
                        // The handshake's last subcommand counts for the write gap.
                        session.last_write = Some(Instant::now());
                    }
                    ControllerKind::Ns2Pro => {}
                }
                session.attach_lightbar(&lightbar);
                let mut prev_buttons = [false; BUTTON_SLOTS];
                let mut last_state: Option<PadState> = None;
                let mut touchpad = TouchpadState::default();
                let outcome = read_loop(
                    &d, &mut session, &tx, &mut last_state, &mut prev_buttons, &mut touchpad,
                    &params, &stop, &cmd_rx,
                );
                // Switch Pro: ask the pad to sleep while the handle is still
                // open, then drop it below.
                if outcome == "manual-disconnect" && kind == ControllerKind::NsPro {
                    session.wait_write_gap();
                    send_turn_off(&d, &mut session.ns_counter);
                }
                // Drop the HID handle so Windows can cleanly drop the
                // Bluetooth bond.
                drop(d);
                // A user-requested disconnect means "turn the pad off": drop
                // the Bluetooth link (see `crate::platform`). The DualSense
                // has no HID power-off command; the Switch Pro got its
                // subcmd 0x06 above and the link drop makes it certain.
                if outcome == "manual-disconnect" {
                    match crate::platform::power_off_connected(kind) {
                        Ok(n) => tracing::info!(removed = n, "controller power-off requested"),
                        Err(e) => tracing::warn!(error = %e, "controller power-off failed"),
                    }
                }
                tracing::info!(?outcome, "read loop exited; back to Searching");
                let _ = tx.send(GamepadEvent::Disconnected);
                // `last_state` is set only by a decoded report, i.e. exactly
                // when `Connected` was emitted.
                if let Some(t) = backoff_after(outcome, last_state.is_some()) {
                    tracing::info!("{} never streamed; backing off", kind.display_name());
                    thread::sleep(t);
                }
            }
            None => {
                // While Searching with no pad available, still respect
                // commands (drain — a no-op, but it consumes a Disconnect or
                // a Rumble meant for a pad that is gone) and respect stop.
                drain_commands(&cmd_rx);
                thread::sleep(ENUM_RETRY_INTERVAL);
            }
        }
    }
}

/// Consume every queued command without acting on it.
fn drain_commands(rx: &Receiver<SourceCmd>) {
    while rx.try_recv().is_ok() {}
}

/// DualSense feature report 0x20 (firmware info). Any error → `None`.
fn read_ds_firmware(d: &hidapi::HidDevice) -> Option<Vec<u8>> {
    let mut buf = [0u8; 64];
    buf[0] = 0x20;
    let n = d.get_feature_report(&mut buf).ok()?;
    Some(buf[..n].to_vec())
}

fn try_handshake(d: &hidapi::HidDevice) -> Result<()> {
    let mut buf = build_handshake_buffer();
    d.get_feature_report(&mut buf)
        .context("requesting calibration feature 0x05 to unlock 0x31 mode")?;
    Ok(())
}

fn read_loop(
    d: &dyn HidIo,
    session: &mut Session,
    tx: &Sender<GamepadEvent>,
    last_state: &mut Option<PadState>,
    prev_buttons: &mut [bool; BUTTON_SLOTS],
    touchpad: &mut TouchpadState,
    params: &CursorParams,
    stop: &Arc<AtomicBool>,
    cmd_rx: &crossbeam_channel::Receiver<SourceCmd>,
) -> &'static str {
    let mut buf = [0u8; REPORT_LEN_BT];
    let disconnect_after = session.disconnect_after();
    // Reset only by a decoded report of this kind (iron rule 11).
    let mut deadline = Instant::now() + disconnect_after;
    let mut emitted_connected = false;
    let mut plan: Option<RumblePlan> = None;
    while !stop.load(Ordering::SeqCst) {
        // Manual disconnect check — drop device and return to Searching.
        // A `Rumble` (re)starts the buzz and never ends the session.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                SourceCmd::Disconnect => {
                    stop_rumble(d, session, &mut plan);
                    return "manual-disconnect";
                }
                SourceCmd::Rumble => {
                    let now = Instant::now();
                    plan = Some(match plan.take() {
                        Some(old) => old.restarted(now),
                        None => RumblePlan::new(session.kind, now),
                    });
                    tracing::info!(kind = ?session.kind, "rumble: start");
                }
            }
        }
        let wrote_rumble = play_rumble(d, session, &mut plan);
        play_lightbar(d, session, wrote_rumble);
        match d.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => {}
            Ok(n) => {
                let frame = &buf[..n];
                if session.kind == ControllerKind::NsPro && frame[0] == NS_REPORT_SIMPLE {
                    session.on_simple_report(d);
                }
                if handle_frame(
                    session, frame, &mut emitted_connected, last_state, prev_buttons,
                    touchpad, params, tx,
                ) {
                    deadline = Instant::now() + disconnect_after;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "HID read error");
                return "read-error";
            }
        }
        if Instant::now() >= deadline {
            return "timeout";
        }
    }
    stop_rumble(d, session, &mut plan);
    write_exit_frame(d, session);
    "stop-signal"
}

/// Decode one raw report with the session kind's decoder and emit its
/// events (`Connected` on the first decoded report). Touchpad processing
/// is DualSense-only. Returns whether the report decoded.
#[allow(clippy::too_many_arguments)]
fn handle_frame(
    session: &Session,
    frame: &[u8],
    emitted_connected: &mut bool,
    last_state: &mut Option<PadState>,
    prev_buttons: &mut [bool; BUTTON_SLOTS],
    touchpad: &mut TouchpadState,
    params: &CursorParams,
    tx: &Sender<GamepadEvent>,
) -> bool {
    let Some(s) = session.decode(frame) else {
        return false;
    };
    if !*emitted_connected {
        let _ = tx.send(GamepadEvent::Connected(session.kind));
        *emitted_connected = true;
    }
    emit_pad_diff(last_state.as_ref(), &s, prev_buttons, tx);
    if session.kind == ControllerKind::DualSense {
        process_touchpad(touchpad, &s, params, tx);
    }
    *last_state = Some(s);
    true
}

fn worker_byte_stream(
    kind: ControllerKind,
    byte_rx: Receiver<Vec<u8>>,
    tx: Sender<GamepadEvent>,
    stop: Arc<AtomicBool>,
    params: CursorParams,
    disconnect_rx: crossbeam_channel::Receiver<()>,
) {
    use crossbeam_channel::select;
    let session = Session::new(kind);
    let mut last_state: Option<PadState> = None;
    let mut prev_buttons = [false; BUTTON_SLOTS];
    let mut touchpad = TouchpadState::default();
    let mut emitted_connected = false;
    let mut disconnect_alive = true;

    while !stop.load(Ordering::SeqCst) {
        if disconnect_alive {
            select! {
                recv(disconnect_rx) -> msg => {
                    if msg.is_err() {
                        // Sender dropped (e.g. test stub) — channel permanently
                        // closed; stop multiplexing it.
                        disconnect_alive = false;
                        continue;
                    }
                    let _ = tx.send(GamepadEvent::Disconnected);
                    emitted_connected = false;
                    last_state = None;
                    touchpad = TouchpadState::default();
                }
                recv(byte_rx) -> result => match result {
                    Ok(buf) => {
                        handle_frame(
                            &session, &buf, &mut emitted_connected, &mut last_state,
                            &mut prev_buttons, &mut touchpad, &params, &tx,
                        );
                    }
                    Err(_) => {
                        let _ = tx.send(GamepadEvent::Disconnected);
                        return;
                    }
                }
            }
        } else {
            // disconnect_rx is closed — fall back to plain blocking recv on byte_rx.
            match byte_rx.recv() {
                Ok(buf) => {
                    handle_frame(
                        &session, &buf, &mut emitted_connected, &mut last_state,
                        &mut prev_buttons, &mut touchpad, &params, &tx,
                    );
                }
                Err(_) => {
                    let _ = tx.send(GamepadEvent::Disconnected);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadrant_for_4_corners_axis_rect() {
        // Standard midpoint (960, 540) — 1920×1080 touchpad space.
        assert_eq!(quadrant_for(100,  100, 960, 540), QUAD_TL, "upper-left corner → TL (25)");
        assert_eq!(quadrant_for(1800, 100, 960, 540), QUAD_TR, "upper-right corner → TR (26)");
        assert_eq!(quadrant_for(100, 1000, 960, 540), QUAD_BL, "lower-left corner → BL (27)");
        assert_eq!(quadrant_for(1800, 1000, 960, 540), QUAD_BR, "lower-right corner → BR (28)");
    }

    #[test]
    fn quadrant_for_boundary_deterministic() {
        // Boundary at mid_x / mid_y. `x < mid_x` is false when x == mid_x → right column.
        // `y < mid_y` is false when y == mid_y → bottom row.
        assert_eq!(quadrant_for(960, 540, 960, 540), QUAD_BR, "exact midpoint → BR (right + bottom)");
        assert_eq!(quadrant_for(960, 100, 960, 540), QUAD_TR, "on X midline, upper → TR");
        assert_eq!(quadrant_for(100, 540, 960, 540), QUAD_BL, "on Y midline, left → BL");
    }

    #[test]
    fn hover_dedupe_same_quadrant_emits_once() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        let mut emitted: Vec<GamepadEvent> = Vec::new();
        for _ in 0..5 {
            process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));
        }
        let hovers: Vec<_> = emitted.iter()
            .filter(|e| matches!(e, GamepadEvent::TouchpadHover { .. }))
            .collect();
        assert_eq!(hovers.len(), 1, "expected 1 emit for 5 same-quadrant frames");
    }

    #[test]
    fn hover_dedupe_quadrant_change_emits_again() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        let mut emitted: Vec<GamepadEvent> = Vec::new();
        process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));  // TR
        process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));  // same — skip
        process_touchpad_hover(true,  200, 200, &mut state, &params, |ev| emitted.push(ev));  // TL
        let hovers: Vec<_> = emitted.iter()
            .filter(|e| matches!(e, GamepadEvent::TouchpadHover { .. }))
            .collect();
        assert_eq!(hovers.len(), 2, "expected 2 emits: enter TR, then enter TL");
    }

    #[test]
    fn hover_lift_emits_sentinel() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        let mut emitted: Vec<GamepadEvent> = Vec::new();
        process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));  // enter TR
        process_touchpad_hover(false,    0,   0, &mut state, &params, |ev| emitted.push(ev)); // lift
        let last_quadrant = emitted.iter().filter_map(|e| match e {
            GamepadEvent::TouchpadHover { quadrant, .. } => Some(*quadrant),
            _ => None,
        }).last().unwrap();
        assert_eq!(last_quadrant, HOVER_QUADRANT_NONE, "lift should emit sentinel quadrant");
    }

    #[test]
    fn hover_lift_then_reentry_emits_again() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        let mut emitted: Vec<GamepadEvent> = Vec::new();
        // Enter TR
        process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));
        // Lift
        process_touchpad_hover(false,    0,   0, &mut state, &params, |ev| emitted.push(ev));
        // Re-enter same quadrant TR → must re-emit (proves dedupe state was reset)
        process_touchpad_hover(true, 1500, 200, &mut state, &params, |ev| emitted.push(ev));
        let hovers: Vec<_> = emitted.iter()
            .filter(|e| matches!(e, GamepadEvent::TouchpadHover { .. }))
            .collect();
        assert_eq!(hovers.len(), 3, "expected 3 emits: enter TR, sentinel, re-enter TR");
    }

    #[test]
    fn filter_cursor_delta_click_freeze_within_window_suppresses() {
        let mut state = TouchpadState::default();
        state.click_btn_held = true;
        state.click_btn_down_at = Some(Instant::now());
        state.click_anchor_pos = Some((500, 500));
        state.pos_buffer.push_back((501, 501, Instant::now()));  // 1px from anchor
        let params = CursorParams::default();
        let result = filter_cursor_delta(2, 2, &mut state, &params);
        assert_eq!(result, None, "within 80ms + < 15px → freeze");
    }

    #[test]
    fn filter_cursor_delta_click_freeze_exits_after_window() {
        let mut state = TouchpadState::default();
        state.click_btn_held = true;
        state.click_btn_down_at = Some(Instant::now() - Duration::from_millis(100));
        state.click_anchor_pos = Some((500, 500));
        state.pos_buffer.push_back((501, 501, Instant::now()));
        let params = CursorParams::default();
        params.set_deadzone_radius(0);  // bypass L2
        let result = filter_cursor_delta(5, 5, &mut state, &params);
        assert!(result.is_some(), "after 80ms window → unfreeze (drag enabled)");
    }

    #[test]
    fn filter_cursor_delta_click_freeze_exits_on_drag() {
        let mut state = TouchpadState::default();
        state.click_btn_held = true;
        state.click_btn_down_at = Some(Instant::now());
        state.click_anchor_pos = Some((500, 500));
        // Cumulative motion 20 px > CLICK_DRAG_EXIT_PX (15) → exit freeze
        state.pos_buffer.push_back((520, 500, Instant::now()));
        let params = CursorParams::default();
        params.set_deadzone_radius(0);
        let result = filter_cursor_delta(5, 0, &mut state, &params);
        assert!(result.is_some(), "cumulative > 15px → drag detected, unfreeze");
    }

    #[test]
    fn filter_cursor_delta_no_freeze_when_disabled() {
        let mut state = TouchpadState::default();
        state.click_btn_held = true;
        state.click_btn_down_at = Some(Instant::now());
        state.click_anchor_pos = Some((500, 500));
        state.pos_buffer.push_back((500, 500, Instant::now()));
        let params = CursorParams::default();
        params.set_click_freeze_enabled(false);
        params.set_deadzone_radius(0);
        let result = filter_cursor_delta(10, 10, &mut state, &params);
        assert!(result.is_some(), "click_freeze_enabled=false → pass through");
    }

    #[test]
    fn filter_deadzone_3_frames_below_radius_suppresses() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();  // deadzone_radius = 2
        // 3 consecutive frames at dx=1, dy=1 → mag² = 2 < 4 → all suppressed.
        assert_eq!(filter_cursor_delta(1, 1, &mut state, &params), None);
        assert_eq!(filter_cursor_delta(1, 1, &mut state, &params), None);
        assert_eq!(filter_cursor_delta(1, 1, &mut state, &params), None);
    }

    #[test]
    fn filter_deadzone_one_frame_above_radius_passes() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        assert_eq!(filter_cursor_delta(1, 1, &mut state, &params), None);
        let result = filter_cursor_delta(5, 5, &mut state, &params);
        assert!(result.is_some(), "5,5 above deadzone radius 2 should pass; got None");
    }

    #[test]
    fn filter_curve_slow_uses_slow_gain() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        params.set_deadzone_radius(0);  // disable L2 so 1 frame survives
        params.set_sensitivity(1.0);    // isolate gain
        let (dx, dy) = filter_cursor_delta(3, 0, &mut state, &params).unwrap();
        // mag = 3 < slow_threshold 5 → gain = 0.5 → 3 * 1.0 * 0.5 = 1.5 → 1 (truncated)
        assert_eq!(dx, 1, "slow region 3*0.5=1.5 → 1");
        assert_eq!(dy, 0);
    }

    #[test]
    fn filter_curve_fast_uses_fast_gain() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        params.set_deadzone_radius(0);
        params.set_sensitivity(1.0);
        let (dx, dy) = filter_cursor_delta(30, 0, &mut state, &params).unwrap();
        // mag = 30 > fast_threshold 20 → gain = 1.5 → 30 * 1.5 = 45
        assert_eq!(dx, 45);
        assert_eq!(dy, 0);
    }

    #[test]
    fn filter_curve_mid_uses_linear_interp() {
        let mut state = TouchpadState::default();
        let params = CursorParams::default();
        params.set_deadzone_radius(0);
        params.set_sensitivity(1.0);
        // dx=10 dy=0 → mag = 10, t = (10-5)/15 = 1/3 → gain = 0.5 + (1/3)*1 = 0.833
        // dx_out = 10 * 0.833 = 8.33 → 8
        let (dx, dy) = filter_cursor_delta(10, 0, &mut state, &params).unwrap();
        assert_eq!(dx, 8, "mid region 10 * 0.833 = 8.33 → 8");
        assert_eq!(dy, 0);
    }

    #[test]
    fn worker_byte_stream_honors_disconnect_signal() {
        use std::sync::atomic::AtomicBool;
        use std::time::Duration;
        let (_byte_tx, byte_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
        let (evt_tx, evt_rx) = crossbeam_channel::unbounded::<GamepadEvent>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let (disc_tx, disc_rx) = crossbeam_channel::unbounded::<()>();
        let params = CursorParams::default();
        let h = std::thread::spawn(move || {
            super::worker_byte_stream(ControllerKind::DualSense, byte_rx, evt_tx, stop_clone, params, disc_rx);
        });
        // Send disconnect immediately
        disc_tx.send(()).expect("send disconnect");
        // Worker should emit Disconnected within 500ms
        let ev = evt_rx.recv_timeout(Duration::from_millis(500))
            .expect("worker did not emit Disconnected within 500ms");
        assert!(matches!(ev, GamepadEvent::Disconnected),
            "expected Disconnected, got {:?}", ev);
        // Worker still alive — verify by stopping it now and joining cleanly
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Send something to unblock the select! so it observes `stop`
        disc_tx.send(()).ok();
        h.join().expect("worker thread should finish cleanly");
    }

    #[test]
    fn stable_pos_picks_closest_to_lookback_target() {
        let mut state = TouchpadState::default();
        let t0 = Instant::now();
        // Push positions at t0, t0+10ms, t0+25ms, t0+40ms, t0+60ms
        state.pos_buffer.push_back((100, 100, t0));
        state.pos_buffer.push_back((110, 100, t0 + Duration::from_millis(10)));
        state.pos_buffer.push_back((130, 100, t0 + Duration::from_millis(25)));
        state.pos_buffer.push_back((160, 100, t0 + Duration::from_millis(40)));
        state.pos_buffer.push_back((200, 100, t0 + Duration::from_millis(60)));
        // Lookback 30ms from t0+60ms → target t0+30ms → closest entry t0+25ms = (130, 100)
        let now = t0 + Duration::from_millis(60);
        let stable = stable_pos_for_click(&state, now, Duration::from_millis(30));
        assert_eq!(stable, Some((130, 100)));
    }

    const ALL_BUSES: [hidapi::BusType; 5] = [
        hidapi::BusType::Unknown,
        hidapi::BusType::Usb,
        hidapi::BusType::Bluetooth,
        hidapi::BusType::I2c,
        hidapi::BusType::Spi,
    ];

    #[test]
    fn is_candidate_ds_any_bus() {
        // DualSense matching is unchanged from v2.4.0: no bus filter.
        for bus in ALL_BUSES {
            assert_eq!(is_candidate(0x054c, 0x0ce6, bus), Some(ControllerKind::DualSense), "{bus:?}");
        }
    }

    #[test]
    fn is_candidate_ns_bluetooth_only() {
        // A USB-attached 057e:2009 needs the 0x80 USB handshake we don't
        // send; Unknown is treated as not-Bluetooth.
        for bus in ALL_BUSES {
            let want = matches!(bus, hidapi::BusType::Bluetooth).then_some(ControllerKind::NsPro);
            assert_eq!(is_candidate(0x057e, 0x2009, bus), want, "{bus:?}");
        }
        assert_eq!(is_candidate(0x057e, 0x2009, hidapi::BusType::Usb), None);
        assert_eq!(is_candidate(0x057e, 0x2009, hidapi::BusType::Unknown), None);
        // Unrelated devices never match.
        assert_eq!(is_candidate(0x046d, 0xc52b, hidapi::BusType::Usb), None);
        assert_eq!(is_candidate(0x057e, 0x2006, hidapi::BusType::Bluetooth), None);
    }

    #[test]
    fn first_candidate_skips_usb_switch_pro() {
        // A USB Switch Pro enumerated before a Bluetooth DualSense must not
        // starve the DualSense (v2.4.0 regression guard).
        let devs = vec![
            (0x057e, 0x2009, hidapi::BusType::Usb),
            (0x046d, 0xc52b, hidapi::BusType::Usb),
            (0x054c, 0x0ce6, hidapi::BusType::Bluetooth),
        ];
        let got = first_candidate(devs.into_iter(), |d| *d);
        assert_eq!(got.map(|(d, k)| (d.0, k)), Some((0x054c, ControllerKind::DualSense)));

        let devs = vec![(0x057e, 0x2009, hidapi::BusType::Usb)];
        assert!(first_candidate(devs.into_iter(), |d| *d).is_none());

        let devs = vec![
            (0x057e, 0x2009, hidapi::BusType::Bluetooth),
            (0x054c, 0x0ce6, hidapi::BusType::Bluetooth),
        ];
        let got = first_candidate(devs.into_iter(), |d| *d);
        assert_eq!(got.map(|(_, k)| k), Some(ControllerKind::NsPro), "first candidate wins");
    }

    #[test]
    fn backoff_only_after_a_dead_session_timeout() {
        // Opened, never decoded a report, timed out: back off before the
        // next enumeration so a dead device isn't re-opened in a tight loop.
        assert_eq!(backoff_after("timeout", false), Some(ENUM_RETRY_INTERVAL));
        // A pad that streamed and then went quiet is searched for at once.
        assert_eq!(backoff_after("timeout", true), None);
        assert_eq!(backoff_after("read-error", false), None);
        assert_eq!(backoff_after("read-error", true), None);
        assert_eq!(backoff_after("manual-disconnect", false), None);
        assert_eq!(backoff_after("stop-signal", false), None);
    }
}

#[cfg(test)]
mod read_loop_tests {
    use super::*;
    use crate::gamepad::ns_protocol::{build_subcmd, NEUTRAL_RUMBLE};
    use crate::gamepad::lightbar::{Effect, LightMode, LightbarConfig, Rgb};
    use crate::gamepad::rumble::{ds_report, ns_rumble_report, DS_BUZZ_MOTOR, NS_BUZZ};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    enum Step {
        Frame(Vec<u8>),
        /// A read that times out after `ms` (returns 0 bytes).
        Idle(u64),
        Error,
    }

    /// Scripted device for `read_loop`. When the script runs out every
    /// read is a timeout of the requested length. Each write is recorded
    /// with its time and the number of reads before it (one read per loop
    /// iteration); `fail_write(n)` makes the Nth write (1-based) fail.
    /// `inject` sends commands on the loop's own channel when the Nth
    /// read (1-based) happens; `stop_at` raises the loop's stop flag (app
    /// shutdown) when the Nth read happens.
    struct ScriptIo {
        steps: Mutex<VecDeque<Step>>,
        writes: Mutex<Vec<Vec<u8>>>,
        stamps: Mutex<Vec<Instant>>,
        at_read: Mutex<Vec<usize>>,
        fail_write: fn(usize) -> bool,
        reads: Mutex<usize>,
        inject: Option<(usize, Vec<SourceCmd>)>,
        cmd_tx: Mutex<Option<Sender<SourceCmd>>>,
        stop_at: Option<usize>,
        stop: Mutex<Option<Arc<AtomicBool>>>,
    }

    impl ScriptIo {
        fn new(steps: Vec<Step>) -> Self {
            ScriptIo {
                steps: Mutex::new(steps.into()),
                writes: Mutex::new(Vec::new()),
                stamps: Mutex::new(Vec::new()),
                at_read: Mutex::new(Vec::new()),
                fail_write: |_| false,
                reads: Mutex::new(0),
                inject: None,
                cmd_tx: Mutex::new(None),
                stop_at: None,
                stop: Mutex::new(None),
            }
        }
    }

    impl HidIo for ScriptIo {
        fn write(&self, data: &[u8]) -> Result<usize> {
            let n_write = {
                let mut w = self.writes.lock().unwrap();
                w.push(data.to_vec());
                w.len()
            };
            self.stamps.lock().unwrap().push(Instant::now());
            self.at_read.lock().unwrap().push(*self.reads.lock().unwrap());
            if (self.fail_write)(n_write) {
                return Err(anyhow!("write failed"));
            }
            Ok(data.len())
        }
        fn read_timeout(&self, buf: &mut [u8], ms: i32) -> Result<usize> {
            let n_read = {
                let mut r = self.reads.lock().unwrap();
                *r += 1;
                *r
            };
            if let Some((at, cmds)) = &self.inject {
                if *at == n_read {
                    if let Some(tx) = self.cmd_tx.lock().unwrap().as_ref() {
                        cmds.iter().for_each(|&c| tx.send(c).unwrap());
                    }
                }
            }
            if self.stop_at == Some(n_read) {
                if let Some(stop) = self.stop.lock().unwrap().as_ref() {
                    stop.store(true, Ordering::SeqCst);
                }
            }
            let step = self.steps.lock().unwrap().pop_front();
            match step {
                Some(Step::Frame(f)) => {
                    let n = f.len().min(buf.len());
                    buf[..n].copy_from_slice(&f[..n]);
                    Ok(n)
                }
                Some(Step::Idle(t)) => {
                    thread::sleep(Duration::from_millis(t));
                    Ok(0)
                }
                Some(Step::Error) => Err(anyhow!("device gone")),
                None => {
                    thread::sleep(Duration::from_millis(ms as u64));
                    Ok(0)
                }
            }
        }
    }

    fn ds_frame() -> Vec<u8> {
        let mut b = vec![0u8; REPORT_LEN_BT];
        b[0] = 0x31;
        b[2..6].copy_from_slice(&[128; 4]);
        b[9] = 0x08;
        b[34] = 0x80;
        b[38] = 0x80;
        b
    }

    fn ns_frame() -> Vec<u8> {
        let mut b = vec![0u8; 49];
        b[0] = 0x30;
        b[6..9].copy_from_slice(&[0x00, 0x08, 0x80]);
        b[9..12].copy_from_slice(&[0x00, 0x08, 0x80]);
        b
    }

    fn ns_3f() -> Vec<u8> {
        let mut b = vec![0u8; 12];
        b[0] = 0x3F;
        b
    }

    fn run(io: &ScriptIo, session: &mut Session) -> (&'static str, Duration, Vec<GamepadEvent>) {
        run_with_cmds(io, session, vec![])
    }

    fn run_with_cmds(
        io: &ScriptIo,
        session: &mut Session,
        cmds: Vec<SourceCmd>,
    ) -> (&'static str, Duration, Vec<GamepadEvent>) {
        let (tx, rx) = unbounded();
        let (dtx, drx) = unbounded::<SourceCmd>();
        for c in cmds {
            dtx.send(c).unwrap();
        }
        *io.cmd_tx.lock().unwrap() = Some(dtx.clone());
        let stop = Arc::new(AtomicBool::new(false));
        *io.stop.lock().unwrap() = Some(stop.clone());
        let mut last_state = None;
        let mut prev_buttons = [false; BUTTON_SLOTS];
        let mut touchpad = TouchpadState::default();
        let params = CursorParams::default();
        let t0 = Instant::now();
        let outcome = read_loop(
            io, session, &tx, &mut last_state, &mut prev_buttons, &mut touchpad,
            &params, &stop, &drx,
        );
        let elapsed = t0.elapsed();
        (outcome, elapsed, rx.try_iter().collect())
    }

    #[test]
    fn rumble_never_ends_the_session() {
        let io = ScriptIo::new(vec![Step::Frame(ds_frame()), Step::Frame(ds_frame()), Step::Error]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error");
    }

    #[test]
    fn disconnect_cmd_still_ends_the_session() {
        let io = ScriptIo::new(vec![Step::Frame(ds_frame()), Step::Error]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Disconnect]);
        assert_eq!(outcome, "manual-disconnect");
    }

    #[test]
    fn disconnect_after_rumble_still_ends_the_session() {
        let io = ScriptIo::new(vec![Step::Frame(ds_frame()), Step::Error]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) =
            run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble, SourceCmd::Disconnect]);
        assert_eq!(outcome, "manual-disconnect");
    }

    #[test]
    fn ds_deadline_is_800ms() {
        // No report at all: gone after ~800 ms, not before.
        let io = ScriptIo::new(vec![]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, elapsed, _) = run(&io, &mut s);
        assert_eq!(outcome, "timeout");
        assert!(elapsed >= Duration::from_millis(800), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(1600), "{elapsed:?}");

        // A 500 ms gap between decoded reports (BT interference) is survived.
        let io = ScriptIo::new(vec![
            Step::Frame(ds_frame()), Step::Idle(500), Step::Frame(ds_frame()), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::DualSense);
        assert_eq!(run(&io, &mut s).0, "read-error", "500 ms gap must not disconnect");

        // A 900 ms gap is not (DualSense is tighter than the Switch Pro).
        let io = ScriptIo::new(vec![
            Step::Frame(ds_frame()), Step::Idle(900), Step::Frame(ds_frame()), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::DualSense);
        assert_eq!(run(&io, &mut s).0, "timeout", "900 ms gap must disconnect");
    }

    #[test]
    fn ns_deadline_is_1000ms() {
        let io = ScriptIo::new(vec![]);
        let mut s = Session::new(ControllerKind::NsPro);
        let (outcome, elapsed, _) = run(&io, &mut s);
        assert_eq!(outcome, "timeout");
        assert!(elapsed >= Duration::from_millis(1000), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(2000), "{elapsed:?}");

        // The 900 ms gap that drops a DualSense is survived by a Switch Pro.
        let io = ScriptIo::new(vec![
            Step::Frame(ns_frame()), Step::Idle(900), Step::Frame(ns_frame()), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::NsPro);
        assert_eq!(run(&io, &mut s).0, "read-error", "900 ms gap must not disconnect");
    }

    #[test]
    fn decoded_frame_resets_deadline_undecoded_does_not() {
        // Decoded 0x31 frames 600 ms apart keep a DualSense alive past 800 ms.
        let io = ScriptIo::new(vec![
            Step::Frame(ds_frame()), Step::Idle(600),
            Step::Frame(ds_frame()), Step::Idle(600),
            Step::Frame(ds_frame()), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, events) = run(&io, &mut s);
        assert_eq!(outcome, "read-error", "decoded frames must reset the deadline");
        assert_eq!(events.first(), Some(&GamepadEvent::Connected(ControllerKind::DualSense)));

        // A non-0x31 report is traffic but not a decoded frame: no reset.
        let mut junk = ds_frame();
        junk[0] = 0x01;
        let io = ScriptIo::new(vec![
            Step::Idle(600), Step::Frame(junk), Step::Idle(300), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, events) = run(&io, &mut s);
        assert_eq!(outcome, "timeout");
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn ns_0x3f_reinit_rate_limited_once_per_second() {
        let mut steps = Vec::new();
        for _ in 0..5 {
            steps.push(Step::Frame(ns_3f()));
            steps.push(Step::Idle(4));
        }
        steps.push(Step::Frame(ns_frame()));
        steps.push(Step::Idle(600));
        steps.push(Step::Frame(ns_frame()));
        steps.push(Step::Idle(500));
        steps.push(Step::Frame(ns_3f())); // > 1 s after the first re-init
        steps.push(Step::Frame(ns_frame()));
        steps.push(Step::Error);
        let io = ScriptIo::new(steps);
        let mut s = Session::new(ControllerKind::NsPro);
        s.ns_counter = 4;
        let (outcome, _, events) = run(&io, &mut s);
        assert_eq!(outcome, "read-error");
        assert_eq!(events.first(), Some(&GamepadEvent::Connected(ControllerKind::NsPro)));
        let writes = io.writes.lock().unwrap().clone();
        assert_eq!(
            writes,
            vec![build_subcmd(4, 0x03, &[0x30]).to_vec(), build_subcmd(5, 0x03, &[0x30]).to_vec()],
            "one 0x03 0x30 per second of 0x3F, on the session counter"
        );
        assert_eq!(s.ns_counter, 6);
    }

    #[test]
    fn ds_ignores_0x3f() {
        let io = ScriptIo::new(vec![
            Step::Frame(ds_frame()), Step::Frame(ns_3f()), Step::Frame(ds_frame()), Step::Error,
        ]);
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run(&io, &mut s);
        assert_eq!(outcome, "read-error");
        assert!(io.writes.lock().unwrap().is_empty());
    }

    /// `n` decoded frames, each followed by a 2 ms idle read, then a read error.
    fn frames(frame: fn() -> Vec<u8>, n: usize) -> Vec<Step> {
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(Step::Frame(frame()));
            v.push(Step::Idle(2));
        }
        v.push(Step::Error);
        v
    }

    fn writes_and_stamps(io: &ScriptIo) -> (Vec<Vec<u8>>, Vec<Instant>) {
        (io.writes.lock().unwrap().clone(), io.stamps.lock().unwrap().clone())
    }

    #[test]
    fn ds_rumble_writes_buzz_then_stop_after_200ms() {
        let io = ScriptIo::new(frames(ds_frame, 200));
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error", "the rumble must not end the session");
        let (writes, stamps) = writes_and_stamps(&io);
        assert_eq!(
            writes,
            vec![
                ds_report(DS_BUZZ_MOTOR, true).to_vec(),
                ds_report(0, true).to_vec(),
                ds_report(0, true).to_vec(),
            ]
        );
        // The plan starts just before the first write is stamped.
        let buzz = stamps[1] - stamps[0];
        assert!(buzz >= Duration::from_millis(199), "buzz lasted {buzz:?}");
    }

    #[test]
    fn ds_rumble_uses_old_flags_when_firmware_is_old() {
        let io = ScriptIo::new(frames(ds_frame, 200));
        let mut s = Session::new(ControllerKind::DualSense);
        s.ds_rumble_v2 = false;
        run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        let (writes, _) = writes_and_stamps(&io);
        assert!(!writes.is_empty(), "no rumble written");
        assert_eq!(writes[0][3], 0x03);
        assert_eq!(writes[0], ds_report(DS_BUZZ_MOTOR, false).to_vec());
        assert_eq!(writes.last().unwrap(), &ds_report(0, false).to_vec());
    }

    #[test]
    fn ds_disconnect_mid_buzz_sends_stop_first() {
        // Disconnect arrives on the 4th read: well inside the 200 ms buzz.
        let mut steps = vec![Step::Frame(ds_frame())];
        steps.extend((0..100).map(|_| Step::Idle(5)));
        let mut io = ScriptIo::new(steps);
        io.inject = Some((4, vec![SourceCmd::Disconnect]));
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "manual-disconnect");
        let (writes, _) = writes_and_stamps(&io);
        assert_eq!(
            writes,
            vec![ds_report(DS_BUZZ_MOTOR, true).to_vec(), ds_report(0, true).to_vec()],
            "buzz, then exactly one stop before the session ends"
        );
    }

    #[test]
    fn restart_then_disconnect_before_the_new_buzz_still_stops() {
        // Read 4: a second Rumble restarts the plan and a Disconnect ends
        // the session in the same iteration, before the new buzz goes out.
        // The first buzz is still running, so a stop must go out.
        let mut steps = vec![Step::Frame(ds_frame())];
        steps.extend((0..100).map(|_| Step::Idle(5)));
        let mut io = ScriptIo::new(steps);
        io.inject = Some((4, vec![SourceCmd::Rumble, SourceCmd::Disconnect]));
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "manual-disconnect");
        let (writes, _) = writes_and_stamps(&io);
        assert_eq!(
            writes,
            vec![ds_report(DS_BUZZ_MOTOR, true).to_vec(), ds_report(0, true).to_vec()],
            "the restarted plan owes the first buzz's stop"
        );
    }

    #[test]
    fn rumble_write_errors_still_try_the_stops_session_continues() {
        let mut io = ScriptIo::new(frames(ds_frame, 200));
        io.fail_write = |_| true;
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error", "read errors decide liveness, not write errors");
        let (writes, stamps) = writes_and_stamps(&io);
        assert_eq!(
            writes,
            vec![
                ds_report(DS_BUZZ_MOTOR, true).to_vec(),
                ds_report(0, true).to_vec(),
                ds_report(0, true).to_vec(),
            ],
            "a failed buzz skips nothing but the buzz; each stop is tried once"
        );
        let to_stop = stamps[1] - stamps[0];
        assert!(to_stop >= Duration::from_millis(199), "stop came after {to_stop:?}");
    }

    #[test]
    fn ds_failed_stop_write_retries_the_second_stop() {
        let mut io = ScriptIo::new(frames(ds_frame, 200));
        io.fail_write = |n| n == 2;
        let mut s = Session::new(ControllerKind::DualSense);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error");
        let (writes, stamps) = writes_and_stamps(&io);
        assert_eq!(
            writes,
            vec![
                ds_report(DS_BUZZ_MOTOR, true).to_vec(),
                ds_report(0, true).to_vec(),
                ds_report(0, true).to_vec(),
            ],
            "the 3rd write is the second stop"
        );
        let to_second = stamps[2] - stamps[0];
        assert!(to_second >= Duration::from_millis(229), "second stop after {to_second:?}");
    }

    #[test]
    fn ns_rumble_frames_are_30ms_apart_and_share_the_counter() {
        let io = ScriptIo::new(frames(ns_frame, 200));
        let mut s = Session::new(ControllerKind::NsPro);
        s.ns_counter = 14;
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error");
        let (writes, stamps) = writes_and_stamps(&io);
        assert!(writes.iter().all(|w| w[0] == 0x10), "only rumble reports here");
        let mut want = Vec::new();
        for (i, c) in [14u8, 15, 0, 1, 2, 3, 4].into_iter().enumerate() {
            let motors = if i < 4 { NS_BUZZ } else { NEUTRAL_RUMBLE };
            want.push(ns_rumble_report(c, motors).to_vec());
        }
        assert_eq!(writes, want, "4 buzz + 3 neutral on the session counter");
        assert_eq!(s.ns_counter, 5);
        for w in stamps.windows(2) {
            let gap = w[1] - w[0];
            assert!(gap >= Duration::from_millis(30), "gap {gap:?} under 30 ms");
        }
    }

    #[test]
    fn ns_reinit_and_rumble_keep_30ms_apart() {
        // A 0x3F every fifth report while the buzz runs.
        let mut steps = Vec::new();
        for i in 0..200 {
            steps.push(Step::Frame(if i % 5 == 0 { ns_3f() } else { ns_frame() }));
            steps.push(Step::Idle(2));
        }
        steps.push(Step::Error);
        let io = ScriptIo::new(steps);
        let mut s = Session::new(ControllerKind::NsPro);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error");
        let (writes, stamps) = writes_and_stamps(&io);
        let reinit = writes.iter().filter(|w| w[0] == 0x01 && w[10] == 0x03).count();
        let rumble = writes.iter().filter(|w| w[0] == 0x10).count();
        assert!(reinit >= 1, "the 0x3F re-init must still go out: {reinit}");
        assert_eq!(rumble, 7, "every rumble step still goes out");
        for w in stamps.windows(2) {
            let gap = w[1] - w[0];
            assert!(gap >= Duration::from_millis(30), "gap {gap:?} under 30 ms");
        }
    }

    /// A DualSense session with the lightbar attached the way `worker_real`
    /// attaches it.
    fn ds_session_with(cfg: LightbarConfig, kbm: bool) -> Session {
        let live = LightbarLive::default();
        live.set_config(cfg);
        live.set_kbm(kbm);
        let mut s = Session::new(ControllerKind::DualSense);
        s.attach_lightbar(&live);
        s
    }

    fn is_setup(w: &[u8]) -> bool {
        w[0] == 0x31 && w[41] == 0x02 && w[44] == 0x02
    }

    fn is_color(w: &[u8]) -> bool {
        w[0] == 0x31 && w[4] == 0x04
    }

    const BLUE: Rgb = Rgb([0, 0, 255]);

    /// kbm = the default rainbow at 80 % (never pure blue); other = fixed
    /// blue at 100 %, so a pure-blue write can only be `other`'s frame.
    fn blue_other_cfg() -> LightbarConfig {
        let mut cfg = LightbarConfig::default();
        cfg.other.effect = Effect::Fixed;
        cfg.other.color = BLUE;
        cfg.other.brightness = 100;
        cfg
    }

    #[test]
    fn lightbar_writes_while_streaming() {
        let io = ScriptIo::new(frames(ds_frame, 150));
        let mut s = ds_session_with(LightbarConfig::default(), true);
        let (outcome, elapsed, _) = run(&io, &mut s);
        assert_eq!(outcome, "read-error");
        assert!(elapsed >= Duration::from_millis(250), "{elapsed:?}");
        let (writes, _) = writes_and_stamps(&io);
        assert!(!writes.is_empty() && is_setup(&writes[0]), "first write is the setup report");
        let colors = writes[1..].iter().filter(|w| is_color(w)).count();
        assert!(colors >= 4, "rainbow animates: {colors} colour reports");
        assert!(writes.iter().all(|w| is_setup(w) || is_color(w)), "only lightbar reports");
    }

    #[test]
    fn native_mode_writes_nothing() {
        // Stopped by the app (the exit-frame path) so no write path is skipped.
        let mut io = ScriptIo::new(frames(ds_frame, 150));
        io.stop_at = Some(40);
        let mut cfg = LightbarConfig::default();
        cfg.mode = LightMode::Native;
        let mut s = ds_session_with(cfg, true);
        let (outcome, _, _) = run(&io, &mut s);
        assert_eq!(outcome, "stop-signal");
        assert!(io.writes.lock().unwrap().is_empty(), "native mode writes not a byte");
    }

    #[test]
    fn lightbar_write_failure_never_ends_session() {
        let mut io = ScriptIo::new(frames(ds_frame, 150));
        io.fail_write = |_| true;
        let mut s = ds_session_with(LightbarConfig::default(), true);
        let (outcome, elapsed, events) = run(&io, &mut s);
        assert_eq!(outcome, "read-error", "only the scripted read error ends it");
        assert!(elapsed >= Duration::from_millis(250), "ended early: {elapsed:?}");
        assert_eq!(events.first(), Some(&GamepadEvent::Connected(ControllerKind::DualSense)));
        let (writes, _) = writes_and_stamps(&io);
        assert!(writes.len() > 1, "a failed write is retried: {}", writes.len());
    }

    #[test]
    fn rumble_step_wins_the_iteration() {
        let io = ScriptIo::new(frames(ds_frame, 150));
        let mut s = ds_session_with(LightbarConfig::default(), true);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "read-error");
        let (writes, stamps) = writes_and_stamps(&io);
        assert_eq!(writes[0][5], DS_BUZZ_MOTOR, "the buzz goes out before the setup");
        // Rumble reports set r[3] (motor flags); lightbar reports never do.
        let rumble: Vec<(Vec<u8>, Instant)> = writes
            .iter()
            .zip(&stamps)
            .filter(|(w, _)| w[3] != 0)
            .map(|(w, t)| (w.clone(), *t))
            .collect();
        assert_eq!(
            rumble.iter().map(|(w, _)| w.clone()).collect::<Vec<_>>(),
            vec![
                ds_report(DS_BUZZ_MOTOR, true).to_vec(),
                ds_report(0, true).to_vec(),
                ds_report(0, true).to_vec(),
            ],
            "every rumble step still goes out"
        );
        let buzz = rumble[1].1 - rumble[0].1;
        assert!(buzz >= Duration::from_millis(199), "buzz lasted {buzz:?}");
        assert!(writes.iter().any(|w| is_setup(w)), "the lightbar still runs");
        // One write per loop iteration: the rumble's iteration has no lightbar write.
        let at_read = io.at_read.lock().unwrap().clone();
        for (i, w) in at_read.windows(2).enumerate() {
            assert_ne!(w[0], w[1], "writes {i} and {} share an iteration", i + 1);
        }
    }

    #[test]
    fn stop_signal_writes_exit_frame() {
        // App stop on the 20th read, mid-buzz.
        let mut io = ScriptIo::new(frames(ds_frame, 150));
        io.stop_at = Some(20);
        let mut s = ds_session_with(blue_other_cfg(), true);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "stop-signal");
        let (writes, _) = writes_and_stamps(&io);
        let blue = ds_lightbar_report(BLUE).to_vec();
        assert_eq!(writes.last(), Some(&blue), "the exit frame is the last write");
        assert_eq!(writes.iter().filter(|w| **w == blue).count(), 1, "written once, at exit");
        assert_eq!(writes[writes.len() - 2], ds_report(0, true).to_vec(), "the rumble stop first");
    }

    #[test]
    fn manual_disconnect_writes_no_exit_frame() {
        let mut io = ScriptIo::new(frames(ds_frame, 150));
        io.inject = Some((20, vec![SourceCmd::Disconnect]));
        let mut s = ds_session_with(blue_other_cfg(), true);
        let (outcome, _, _) = run(&io, &mut s);
        assert_eq!(outcome, "manual-disconnect");
        let (writes, _) = writes_and_stamps(&io);
        let blue = ds_lightbar_report(BLUE).to_vec();
        assert!(!writes.is_empty(), "the lightbar ran before the disconnect");
        assert_ne!(writes.last(), Some(&blue));
        assert!(writes.iter().all(|w| *w != blue), "no exit frame on manual disconnect");
    }

    #[test]
    fn ns_session_has_no_lightbar() {
        let live = LightbarLive::default();
        live.set_kbm(true);
        let mut s = Session::new(ControllerKind::NsPro);
        s.attach_lightbar(&live);
        let mut io = ScriptIo::new(frames(ns_frame, 150));
        io.stop_at = Some(60);
        let (outcome, _, _) = run_with_cmds(&io, &mut s, vec![SourceCmd::Rumble]);
        assert_eq!(outcome, "stop-signal");
        let (writes, _) = writes_and_stamps(&io);
        assert!(!writes.is_empty(), "the rumble still goes out");
        assert!(writes.iter().all(|w| w[0] != 0x31), "no 0x31 report on a Switch Pro");
    }

    #[test]
    fn rumble_while_searching_is_drained_without_writes() {
        // worker_real's Searching branch: no device, so commands are only consumed.
        let (tx, rx) = unbounded::<SourceCmd>();
        tx.send(SourceCmd::Rumble).unwrap();
        tx.send(SourceCmd::Disconnect).unwrap();
        drain_commands(&rx);
        assert!(rx.is_empty(), "Rumble and Disconnect must both be consumed");
    }
}
