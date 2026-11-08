//! Protocol-neutral pad snapshot shared by every HID decoder
//! (DualSense 0x31, Switch Pro, ...). Diffing two of these yields the
//! `GamepadEvent` stream.

use super::battery::Battery;
use super::events::GamepadEvent;
use crossbeam_channel::Sender;

/// Number of button slots in a pad snapshot (ids `0..=32`).
pub const BUTTON_SLOTS: usize = 33;

/// Decoded snapshot of a single input frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PadState {
    pub stick_lx: f32,
    pub stick_ly: f32,
    pub stick_rx: f32,
    pub stick_ry: f32,
    pub trigger_l2: f32,
    pub trigger_r2: f32,
    /// Bit-vector of buttons indexed `0..BUTTON_SLOTS`. Same ids as v1.x
    /// `gamepad::button_index`, so existing config files migrate as-is.
    pub buttons: [bool; BUTTON_SLOTS],
    /// Touchpad finger 0: true when a finger is currently in contact.
    pub finger0_active: bool,
    /// Touchpad x in 0..=1919.
    pub finger0_x: u16,
    /// Touchpad y in 0..=1079.
    pub finger0_y: u16,
    /// Whole-touchpad physical click switch.
    pub touchpad_btn: bool,
    /// Battery, when the report carries one (DualSense, Switch Pro).
    pub battery: Option<Battery>,
}

// Hand-written: std only derives `Default` for arrays up to 32 elements and
// `BUTTON_SLOTS` is 33.
impl Default for PadState {
    fn default() -> Self {
        PadState {
            stick_lx: 0.0,
            stick_ly: 0.0,
            stick_rx: 0.0,
            stick_ry: 0.0,
            trigger_l2: 0.0,
            trigger_r2: 0.0,
            buttons: [false; BUTTON_SLOTS],
            finger0_active: false,
            finger0_x: 0,
            finger0_y: 0,
            touchpad_btn: false,
            battery: None,
        }
    }
}

/// Emit the `GamepadEvent` stream for the change from `prev` to `cur`
/// (everything on the first frame, when `prev` is `None`). Shared by every
/// transport worker (HID, BLE) so they produce identical events.
pub(crate) fn emit_pad_diff(
    prev: Option<&PadState>,
    cur: &PadState,
    prev_buttons: &mut [bool; BUTTON_SLOTS],
    tx: &Sender<GamepadEvent>,
) {
    if prev.map_or(true, |p| p.stick_lx != cur.stick_lx) {
        let _ = tx.send(GamepadEvent::Stick { axis: 0, value: cur.stick_lx });
    }
    if prev.map_or(true, |p| p.stick_ly != cur.stick_ly) {
        let _ = tx.send(GamepadEvent::Stick { axis: 1, value: cur.stick_ly });
    }
    if prev.map_or(true, |p| p.stick_rx != cur.stick_rx) {
        let _ = tx.send(GamepadEvent::Stick { axis: 2, value: cur.stick_rx });
    }
    if prev.map_or(true, |p| p.stick_ry != cur.stick_ry) {
        let _ = tx.send(GamepadEvent::Stick { axis: 3, value: cur.stick_ry });
    }
    if prev.map_or(true, |p| p.trigger_l2 != cur.trigger_l2) {
        let _ = tx.send(GamepadEvent::Trigger { axis: 4, value: cur.trigger_l2 });
    }
    if prev.map_or(true, |p| p.trigger_r2 != cur.trigger_r2) {
        let _ = tx.send(GamepadEvent::Trigger { axis: 5, value: cur.trigger_r2 });
    }
    for (i, &down) in cur.buttons.iter().enumerate() {
        if down != prev_buttons[i] {
            let id = i as u32;
            let _ = tx.send(if down {
                GamepadEvent::ButtonDown(id)
            } else {
                GamepadEvent::ButtonUp(id)
            });
        }
    }
    prev_buttons.copy_from_slice(&cur.buttons);
    // First frame: only a reading. Later: any change, a reading that
    // disappears (Some -> None) included.
    if prev.map_or(cur.battery.is_some(), |p| p.battery != cur.battery) {
        let _ = tx.send(GamepadEvent::Battery(cur.battery));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn emit_pad_diff_first_call_emits_all_then_nothing() {
        let (tx, rx) = unbounded();
        let mut cur = PadState::default();
        cur.buttons[3] = true;
        cur.buttons[7] = true;
        let mut prev_buttons = [false; BUTTON_SLOTS];

        emit_pad_diff(None, &cur, &mut prev_buttons, &tx);
        let evs: Vec<GamepadEvent> = rx.try_iter().collect();
        assert_eq!(evs.iter().filter(|e| matches!(e, GamepadEvent::Stick { .. })).count(), 4);
        assert_eq!(evs.iter().filter(|e| matches!(e, GamepadEvent::Trigger { .. })).count(), 2);
        assert!(evs.contains(&GamepadEvent::ButtonDown(3)));
        assert!(evs.contains(&GamepadEvent::ButtonDown(7)));
        assert_eq!(evs.len(), 8);

        emit_pad_diff(Some(&cur), &cur, &mut prev_buttons, &tx);
        assert_eq!(rx.try_iter().count(), 0, "identical frame emits nothing");
    }

    #[test]
    fn emit_pad_diff_sends_battery_on_first_frame_and_on_change_only() {
        use crate::gamepad::Battery;
        let (tx, rx) = unbounded();
        let mut prev_buttons = [false; BUTTON_SLOTS];
        let mut cur = PadState::default();
        cur.battery = Some(Battery { pct: 80, charging: false });

        emit_pad_diff(None, &cur, &mut prev_buttons, &tx);
        let batt = |evs: Vec<GamepadEvent>| -> Vec<GamepadEvent> {
            evs.into_iter().filter(|e| matches!(e, GamepadEvent::Battery(_))).collect()
        };
        assert_eq!(batt(rx.try_iter().collect()), vec![GamepadEvent::Battery(cur.battery)]);

        emit_pad_diff(Some(&cur), &cur, &mut prev_buttons, &tx);
        assert_eq!(rx.try_iter().count(), 0, "same reading: nothing");

        let mut next = cur.clone();
        next.battery = Some(Battery { pct: 80, charging: true });
        emit_pad_diff(Some(&cur), &next, &mut prev_buttons, &tx);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![GamepadEvent::Battery(next.battery)]);

        // The reading disappears (DualSense error status mid-session): say so.
        let gone = PadState::default();
        emit_pad_diff(Some(&next), &gone, &mut prev_buttons, &tx);
        assert_eq!(batt(rx.try_iter().collect()), vec![GamepadEvent::Battery(None)]);
        emit_pad_diff(Some(&gone), &gone, &mut prev_buttons, &tx);
        assert_eq!(rx.try_iter().count(), 0, "still none: nothing");

        // A pad without a reading sends none.
        let none = PadState::default();
        emit_pad_diff(None, &none, &mut [false; BUTTON_SLOTS], &tx);
        assert!(batt(rx.try_iter().collect()).is_empty());
    }
}
