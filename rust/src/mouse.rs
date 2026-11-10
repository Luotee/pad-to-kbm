//! OS mouse synthesis sink. Mirrors `KeyboardSink`: an Enigo handle
//! gated by `dry_run`, with the safety refcount in `KeyState` deciding
//! whether to actually emit a Press / Release transition.
//!
//! Iron rule #3: `Drop` and `release_all_held` synthesise Release for
//! every held mouse button so shutdown / panic cannot leak a stuck
//! MOUSEDOWN. `min_press_ms` is deliberately not honoured here — the
//! anti-cheat profile of a stuck button is far worse than a fast
//! release.

use crate::config::MouseButton;
use crate::safety::{Edge, SharedKeyState};
use anyhow::Result;
use enigo::{Axis, Button as EnigoButton, Direction, Enigo, Mouse, Settings};

pub struct MouseSink {
    enigo: Option<Enigo>,
    state: SharedKeyState,
    dry_run: bool,
    /// Dry run only: the total movement asked for, for tests to read.
    moved: Option<std::sync::Arc<std::sync::Mutex<(i64, i64)>>>,
}

impl MouseSink {
    pub fn new(state: SharedKeyState, dry_run: bool) -> Result<Self> {
        let enigo = if dry_run { None } else { Some(Enigo::new(&Settings::default())?) };
        Ok(Self { enigo, state, dry_run, moved: None })
    }

    /// Dry run: add every `move_rel` to `total` (engine tests).
    pub fn with_move_total(mut self, total: std::sync::Arc<std::sync::Mutex<(i64, i64)>>) -> Self {
        self.moved = Some(total);
        self
    }

    pub fn click_press(&mut self, b: MouseButton) -> Result<()> {
        // Wheel events are one-shot scrolls — they have no Press/Release pair.
        if matches!(b, MouseButton::WheelUp | MouseButton::WheelDown) {
            let delta = if matches!(b, MouseButton::WheelUp) { -1 } else { 1 };
            self.emit_scroll(delta)?;
            return Ok(());
        }
        let edge = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.press_mouse(b)
        };
        if edge == Edge::Press {
            self.emit_button(b, Direction::Press)?;
        }
        Ok(())
    }

    pub fn click_release(&mut self, b: MouseButton) -> Result<()> {
        // Wheel "release" is a no-op — the scroll happened on press.
        if matches!(b, MouseButton::WheelUp | MouseButton::WheelDown) {
            return Ok(());
        }
        let edge = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.release_mouse(b)
        };
        if edge == Edge::Release {
            self.emit_button(b, Direction::Release)?;
        }
        Ok(())
    }

    pub fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        if self.dry_run {
            tracing::debug!(dx, dy, "[dry-run] mouse move_rel");
            if let Some(t) = &self.moved {
                let mut t = t.lock().unwrap_or_else(|p| p.into_inner());
                t.0 += i64::from(dx);
                t.1 += i64::from(dy);
            }
            return Ok(());
        }
        let enigo = self.enigo.as_mut()
            .expect("non-dry-run MouseSink must hold an Enigo (constructed in new())");
        enigo.move_mouse(dx, dy, enigo::Coordinate::Rel)
            .map_err(|e| anyhow::anyhow!("mouse move ({dx},{dy}): {e}"))?;
        Ok(())
    }

    /// Release every currently held mouse button. Called by Drop and on
    /// pause / shutdown / panic. `min_press_ms` is intentionally not
    /// honoured — refer to the iron rule in CLAUDE.md.
    pub fn release_all_held(&mut self) {
        let held: Vec<MouseButton> = {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.drain_held_mouse()
        };
        for b in held {
            tracing::info!(?b, "releasing held mouse button on shutdown");
            let _ = self.emit_button(b, Direction::Release);
        }
    }

    fn emit_button(&mut self, b: MouseButton, dir: Direction) -> Result<()> {
        tracing::info!(?b, ?dir, "mouse synth");
        if self.dry_run {
            println!("[dry-run] {dir:?} mouse {b:?}");
            return Ok(());
        }
        let eb = enigo_button(b)
            .expect("wheel-up/down handled in click_press; should not reach emit_button");
        let enigo = self.enigo.as_mut()
            .expect("non-dry-run MouseSink must hold an Enigo (constructed in new())");
        enigo.button(eb, dir)
            .map_err(|e| anyhow::anyhow!("enigo mouse {b:?} {dir:?}: {e}"))?;
        Ok(())
    }

    fn emit_scroll(&mut self, delta: i32) -> Result<()> {
        tracing::info!(delta, "wheel scroll");
        if self.dry_run {
            println!("[dry-run] wheel scroll delta={delta}");
            return Ok(());
        }
        let enigo = self.enigo.as_mut()
            .expect("non-dry-run MouseSink must hold an Enigo (constructed in new())");
        enigo.scroll(delta, Axis::Vertical)
            .map_err(|e| anyhow::anyhow!("wheel scroll {delta}: {e}"))?;
        Ok(())
    }
}

impl Drop for MouseSink {
    fn drop(&mut self) {
        self.release_all_held();
    }
}

fn enigo_button(b: MouseButton) -> Option<EnigoButton> {
    Some(match b {
        MouseButton::Left => EnigoButton::Left,
        MouseButton::Middle => EnigoButton::Middle,
        MouseButton::Right => EnigoButton::Right,
        MouseButton::WheelUp | MouseButton::WheelDown => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety;

    #[test]
    fn dry_run_press_release_balances_state() {
        let state = safety::shared();
        let mut sink = MouseSink::new(state.clone(), true).unwrap();
        sink.click_press(MouseButton::Left).unwrap();
        assert!(state.lock().unwrap().is_mouse_held(MouseButton::Left));
        sink.click_release(MouseButton::Left).unwrap();
        assert!(!state.lock().unwrap().is_mouse_held(MouseButton::Left));
    }

    #[test]
    fn double_press_only_first_emits_and_release_at_zero() {
        let state = safety::shared();
        let mut sink = MouseSink::new(state.clone(), true).unwrap();
        sink.click_press(MouseButton::Right).unwrap();
        sink.click_press(MouseButton::Right).unwrap();
        // Still held after one release — refcount is 1.
        sink.click_release(MouseButton::Right).unwrap();
        assert!(state.lock().unwrap().is_mouse_held(MouseButton::Right));
        sink.click_release(MouseButton::Right).unwrap();
        assert!(!state.lock().unwrap().is_mouse_held(MouseButton::Right));
    }

    #[test]
    fn release_all_held_drains_refcount() {
        let state = safety::shared();
        let mut sink = MouseSink::new(state.clone(), true).unwrap();
        sink.click_press(MouseButton::Left).unwrap();
        sink.click_press(MouseButton::Middle).unwrap();
        sink.release_all_held();
        let s = state.lock().unwrap();
        assert_eq!(s.len_held_mouse(), 0);
    }

    #[test]
    fn wheel_press_is_one_shot_no_refcount_change() {
        let state = safety::shared();
        let mut sink = MouseSink::new(state.clone(), true).unwrap();
        sink.click_press(MouseButton::WheelUp).unwrap();
        assert_eq!(state.lock().unwrap().len_held_mouse(), 0);
        // Release is a no-op.
        sink.click_release(MouseButton::WheelUp).unwrap();
        assert_eq!(state.lock().unwrap().len_held_mouse(), 0);
    }

    #[test]
    fn move_rel_zero_zero_is_noop() {
        let state = safety::shared();
        let mut sink = MouseSink::new(state.clone(), true).unwrap();
        sink.move_rel(0, 0).unwrap(); // no panic, no error
    }
}
