use crate::config::{Binding, Config, MouseButton};
use crate::gamepad::{ControllerKind, GamepadEvent};
use crate::pad_bindings::{PadBindings, Side, TriggerMode, QUICK_IDS};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyAction {
    /// Key value as written in config (canonical), and what sent it.
    Press(String, Source),
    Release(String, Source),
    MacroStart { name: String, source_id: u32 },
    MacroStop { source_id: u32 },
    MousePress(MouseButton),
    MouseRelease(MouseButton),
    MouseMove { dx: i32, dy: i32 },
    /// Pure diagnostic — engine forwards as a Tauri event so the GUI
    /// can show a debug dot at the captured touchpad position.
    TouchpadClick { raw_x: u16, raw_y: u16, quadrant: u32 },
    /// Continuous touchpad hover preview. Emitted per HID frame on
    /// quadrant change, forwarded to Tauri as a diagnostic event for GUI
    /// hover rendering. `quadrant = 255` is the sentinel for "finger lifted".
    TouchpadHover { raw_x: u16, raw_y: u16, quadrant: u32 },
}

/// What sent a key: a pad button (its id) or a running macro (its name).
/// Shown in 最近的事件 and written to the event log (v3.2.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Button(u32),
    Macro(String),
}

impl Source {
    /// `btn:0` / `macro:連發` (event log column).
    pub fn tag(&self) -> String {
        match self {
            Source::Button(id) => format!("btn:{id}"),
            Source::Macro(name) => format!("macro:{name}"),
        }
    }
}

pub struct Mapper {
    cfg: Config,
    /// The pad whose table is used: the session owner, or `DualSense` when
    /// no controller is connected.
    pad: ControllerKind,
    /// Last reported value per stick axis (0..=3) and trigger axis (4..=5).
    last_axis: [f32; 6],
    /// Active virtual-button state for indices 15..=24.
    virt_active: [bool; 25],
    /// Accumulates `(id, pressed)` for any virtual-button state flip that
    /// happens inside `transition_virtual` — sticks (15..=22) and analog
    /// triggers (23..=24). The engine drains this every tick via
    /// `take_visual_transitions` and forwards each entry as an
    /// `EngineEvent::ButtonDown` / `ButtonUp` so the GUI can flash the
    /// same press-ring it shows for real, gilrs-delivered button events.
    pending_visual: Vec<(u32, bool)>,
    /// The binding each held id was pressed with. Its release uses (and
    /// removes) this entry, so a release always matches its press even if
    /// the page, the trigger mode or the config changed in between
    /// (iron rule 2). An id not in here releases nothing.
    held: HashMap<u32, Binding>,
    /// Digital trigger state (`ButtonDown(23|24)`), per side. A side is
    /// held while its digital bit OR its analog virtual is active, so a
    /// pad that reports both (DualSense, Switch Pro) counts once.
    trigger_digital: [bool; 2],
    /// Combined trigger state, per side.
    trigger_active: [bool; 2],
    /// Held page-mode triggers, in press order; the last one picks the page.
    page_stack: Vec<Side>,
}

const TRIGGER_L: u32 = 23;
const TRIGGER_R: u32 = 24;

fn trigger_side(id: u32) -> Option<Side> {
    match id {
        TRIGGER_L => Some(Side::L),
        TRIGGER_R => Some(Side::R),
        _ => None,
    }
}

fn side_index(side: Side) -> usize {
    match side {
        Side::L => 0,
        Side::R => 1,
    }
}

fn side_id(side: Side) -> u32 {
    match side {
        Side::L => TRIGGER_L,
        Side::R => TRIGGER_R,
    }
}

impl Mapper {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            pad: ControllerKind::DualSense,
            last_axis: [0.0; 6],
            virt_active: [false; 25],
            pending_visual: Vec::new(),
            held: HashMap::new(),
            trigger_digital: [false; 2],
            trigger_active: [false; 2],
            page_stack: Vec::new(),
        }
    }

    /// Use the table of `kind` for presses from now on. `None` (no
    /// controller) uses the `dualsense` table.
    pub fn set_pad(&mut self, kind: Option<ControllerKind>) {
        self.pad = kind.unwrap_or(ControllerKind::DualSense);
    }

    /// Swap in a new config, keeping the held bindings, the virtual-axis
    /// state and the held triggers: a key held across the change is still
    /// released with the binding it was pressed with.
    pub fn replace_config(&mut self, cfg: Config) {
        self.cfg = cfg;
    }

    /// Forget every held press, trigger and stick state, keeping the config
    /// and the pad. The engine calls this right after it released
    /// everything itself (pause, mapping ON→OFF): the releases of those
    /// presses may never arrive (no events are polled while paused), and a
    /// stale entry would swallow the next press.
    pub fn reset_input_state(&mut self) {
        self.last_axis = [0.0; 6];
        self.virt_active = [false; 25];
        self.pending_visual.clear();
        self.held.clear();
        self.trigger_digital = [false; 2];
        self.trigger_active = [false; 2];
        self.page_stack.clear();
    }

    /// Drain the buffered virtual-button transitions accumulated since the
    /// last call. Each entry is `(id, pressed)`; pressed=true means the
    /// virtual press just went active, false means it just released.
    pub fn take_visual_transitions(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.pending_visual)
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn handle(&mut self, ev: GamepadEvent) -> Vec<KeyAction> {
        match ev {
            GamepadEvent::Connected(_) | GamepadEvent::Disconnected => Vec::new(),
            GamepadEvent::ButtonDown(id) => match trigger_side(id) {
                Some(side) => {
                    self.trigger_digital[side_index(side)] = true;
                    self.trigger_edge(side)
                }
                None => self.press(id),
            },
            GamepadEvent::ButtonUp(id) => match trigger_side(id) {
                Some(side) => {
                    self.trigger_digital[side_index(side)] = false;
                    self.trigger_edge(side)
                }
                None => self.release(id),
            },
            GamepadEvent::Stick { axis, value } => {
                self.last_axis[axis as usize] = value;
                self.update_axis_virtuals(axis)
            }
            GamepadEvent::Trigger { axis, value } => {
                self.last_axis[axis as usize] = value;
                self.update_trigger_virtuals(axis)
            }
            GamepadEvent::MouseDelta { dx, dy } => vec![KeyAction::MouseMove { dx, dy }],
            GamepadEvent::TouchpadClick { raw_x, raw_y, quadrant } => {
                vec![KeyAction::TouchpadClick { raw_x, raw_y, quadrant }]
            }
            GamepadEvent::TouchpadHover { raw_x, raw_y, quadrant } => {
                vec![KeyAction::TouchpadHover { raw_x, raw_y, quadrant }]
            }
        }
    }

    fn pad_bindings(&self) -> &PadBindings {
        self.cfg.pad(self.pad)
    }

    /// The binding a press of `id` uses right now: the active quick page
    /// for a quick id while a page-mode trigger is held (a missing id there
    /// is `Unbound`, never the base binding), else the base table.
    fn binding_for_press(&self, id: u32) -> Binding {
        let pad = self.pad_bindings();
        let table = match self.page_stack.last() {
            Some(side) if QUICK_IDS.contains(&id) => pad.table(side.page()),
            _ => &pad.buttons,
        };
        table
            .get(&id.to_string())
            .map(|e| e.binding.clone())
            .unwrap_or(Binding::Unbound)
    }

    fn press(&mut self, id: u32) -> Vec<KeyAction> {
        if self.held.contains_key(&id) {
            tracing::debug!(id, "press: already held, ignored");
            return Vec::new();
        }
        let binding = self.binding_for_press(id);
        let out = press_actions(id, &binding);
        self.held.insert(id, binding);
        out
    }

    fn release(&mut self, id: u32) -> Vec<KeyAction> {
        match self.held.remove(&id) {
            Some(binding) => release_actions(id, &binding),
            None => Vec::new(),
        }
    }

    /// Re-evaluate a trigger side after its digital or analog source
    /// changed, and act on the combined edge.
    fn trigger_edge(&mut self, side: Side) -> Vec<KeyAction> {
        let i = side_index(side);
        let now = self.trigger_digital[i] || self.virt_active[side_id(side) as usize];
        if now == self.trigger_active[i] {
            return Vec::new();
        }
        self.trigger_active[i] = now;
        if now {
            match self.pad_bindings().trigger_mode.get(side) {
                TriggerMode::Page => {
                    self.page_stack.retain(|s| *s != side);
                    self.page_stack.push(side);
                    Vec::new()
                }
                TriggerMode::Key => self.press(side_id(side)),
            }
        } else {
            // Undo whatever the press did, whatever the mode is now.
            self.page_stack.retain(|s| *s != side);
            self.release(side_id(side))
        }
    }

    fn update_axis_virtuals(&mut self, axis: u32) -> Vec<KeyAction> {
        // gilrs convention: stick X negative = left, positive = right;
        //                   stick Y negative = down, positive = up.
        // (Note: this differs from pygame, which inverts Y. The legacy Python
        //  used pygame's flipped convention; the Rust port matches gilrs directly.)
        // axis 0 (lx): neg → 17 (left),  pos → 18 (right)
        // axis 1 (ly): neg → 16 (down),  pos → 15 (up)
        // axis 2 (rx): neg → 21 (left),  pos → 22 (right)
        // axis 3 (ry): neg → 20 (down),  pos → 19 (up)
        let (neg_id, pos_id) = match axis {
            0 => (17, 18),
            1 => (16, 15),
            2 => (21, 22),
            3 => (20, 19),
            _ => return Vec::new(),
        };
        let v = self.last_axis[axis as usize];
        let dz = self.cfg.deadzone;
        let want_neg = v < -dz;
        let want_pos = v > dz;
        let mut out = Vec::new();
        out.extend(self.transition_virtual(neg_id, want_neg));
        out.extend(self.transition_virtual(pos_id, want_pos));
        out
    }

    fn update_trigger_virtuals(&mut self, axis: u32) -> Vec<KeyAction> {
        // axis 4 (l2) → 23, axis 5 (r2) → 24. Already normalized to [0, 1].
        let id = match axis { 4 => 23, 5 => 24, _ => return Vec::new() };
        let v = self.last_axis[axis as usize];
        let want = v >= self.cfg.trigger_threshold;
        self.transition_virtual(id, want)
    }

    fn transition_virtual(&mut self, id: u32, want_active: bool) -> Vec<KeyAction> {
        let was = self.virt_active[id as usize];
        if want_active == was { return Vec::new(); }
        self.virt_active[id as usize] = want_active;
        // Record the transition so the engine can re-emit it as a visual
        // ButtonDown/Up event (the GUI press-ring otherwise never lights up
        // for stick directions or analog-trigger virtual ids, because those
        // never come back as a real gilrs ButtonPressed event).
        self.pending_visual.push((id, want_active));
        if let Some(side) = trigger_side(id) {
            return self.trigger_edge(side);
        }
        if want_active { self.press(id) } else { self.release(id) }
    }
}

fn press_actions(id: u32, binding: &Binding) -> Vec<KeyAction> {
    match binding {
        Binding::Key(k) => {
            tracing::info!(id, key = %k, "press");
            vec![KeyAction::Press(k.clone(), Source::Button(id))]
        }
        Binding::Macro(n) => {
            tracing::info!(id, macro_name = %n, "macro start");
            vec![KeyAction::MacroStart { name: n.clone(), source_id: id }]
        }
        Binding::Mouse(b) => {
            tracing::info!(id, ?b, "mouse press");
            vec![KeyAction::MousePress(*b)]
        }
        Binding::Unbound => {
            tracing::debug!(id, "press: unbound");
            Vec::new()
        }
    }
}

fn release_actions(id: u32, binding: &Binding) -> Vec<KeyAction> {
    match binding {
        Binding::Key(k) => {
            tracing::info!(id, key = %k, "release");
            vec![KeyAction::Release(k.clone(), Source::Button(id))]
        }
        Binding::Macro(_) => {
            tracing::info!(id, "macro stop");
            vec![KeyAction::MacroStop { source_id: id }]
        }
        Binding::Mouse(b) => {
            tracing::info!(id, ?b, "mouse release");
            vec![KeyAction::MouseRelease(*b)]
        }
        Binding::Unbound => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ButtonEntry, MacroDef, MacroStep, StepAction};
    use crate::gamepad::ControllerKind;
    use crate::pad_bindings::{Page, TriggerMode};
    use std::collections::BTreeMap;

    fn cfg_with_overrides(mut overrides: Vec<(u32, Binding)>) -> Config {
        let mut buttons = BTreeMap::new();
        for id in crate::config::VALID_BUTTON_IDS {
            buttons.insert(id.to_string(), ButtonEntry {
                label: format!("b{id}"),
                binding: Binding::Unbound,
            });
        }
        for (id, b) in overrides.drain(..) {
            buttons.insert(id.to_string(), ButtonEntry { label: format!("b{id}"), binding: b });
        }
        let mut macros = BTreeMap::new();
        macros.insert("m_left_right".into(), MacroDef {
            repeat: true,
            steps: vec![MacroStep {
                key: "Left".into(), action: StepAction::Down, delay_ms: [10, 20],
            }],
        });
        let pads = crate::gamepad::ControllerKind::ALL
            .into_iter()
            .map(|k| (k.slug().to_string(), crate::pad_bindings::PadBindings {
                buttons: buttons.clone(),
                quick_l: BTreeMap::new(),
                quick_r: BTreeMap::new(),
                trigger_mode: Default::default(),
            }))
            .collect();
        Config {
            version: 2,
            deadzone: 0.4,
            trigger_threshold: 0.5,
            min_press_ms: [8, 25],
            tick_jitter_ms: [0, 3],
            log_events: false,
            pads, macros,
            touchpad_cursor_enabled: true,
            touchpad_cursor_sensitivity: 1.5,
            touchpad_midpoint_x: 960,
            touchpad_midpoint_y: 540,
            touchpad_accel_slow_threshold: 5,
            touchpad_accel_fast_threshold: 20,
            touchpad_accel_gain_slow: 0.5,
            touchpad_accel_gain_fast: 1.5,
            touchpad_deadzone_radius: 2,
            touchpad_click_freeze_enabled: true,
            mapping_enabled: Default::default(),
            rumble_on_connect: true,
            stick_cal: Default::default(),
            last_pad: None,
        }
    }

    #[test]
    fn press_carries_button_source() {
        let cfg = cfg_with_overrides(vec![(3, Binding::Key("j".into()))]);
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(GamepadEvent::ButtonDown(3)), vec![KeyAction::Press("j".into(), Source::Button(3))]);
        assert_eq!(m.handle(GamepadEvent::ButtonUp(3)), vec![KeyAction::Release("j".into(), Source::Button(3))]);
    }

    #[test]
    fn physical_key_press_release() {
        let cfg = cfg_with_overrides(vec![(0, Binding::Key("x".into()))]);
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(GamepadEvent::ButtonDown(0)),
                   vec![KeyAction::Press("x".into(), Source::Button(0))]);
        assert_eq!(m.handle(GamepadEvent::ButtonUp(0)),
                   vec![KeyAction::Release("x".into(), Source::Button(0))]);
    }

    #[test]
    fn unbound_button_yields_nothing() {
        let cfg = cfg_with_overrides(vec![]);
        let mut m = Mapper::new(cfg);
        assert!(m.handle(GamepadEvent::ButtonDown(3)).is_empty());
    }

    #[test]
    fn stick_crosses_deadzone_then_returns() {
        let cfg = cfg_with_overrides(vec![(17, Binding::Key("Left".into()))]);
        let mut m = Mapper::new(cfg);

        // below deadzone — nothing
        assert!(m.handle(GamepadEvent::Stick { axis: 0, value: -0.2 }).is_empty());

        // cross deadzone negative — press 17
        assert_eq!(
            m.handle(GamepadEvent::Stick { axis: 0, value: -0.9 }),
            vec![KeyAction::Press("Left".into(), Source::Button(17))]
        );

        // still active — nothing further
        assert!(m.handle(GamepadEvent::Stick { axis: 0, value: -0.7 }).is_empty());

        // return inside deadzone — release 17
        assert_eq!(
            m.handle(GamepadEvent::Stick { axis: 0, value: 0.0 }),
            vec![KeyAction::Release("Left".into(), Source::Button(17))]
        );
    }

    #[test]
    fn trigger_threshold_drives_virtual_23() {
        let mut cfg = cfg_with_overrides(vec![
            (23, Binding::Macro("m_left_right".into())),
        ]);
        cfg.pad_mut(ControllerKind::DualSense).trigger_mode.l = TriggerMode::Key;
        let mut m = Mapper::new(cfg);
        // normalized value < threshold — nothing
        assert!(m.handle(GamepadEvent::Trigger { axis: 4, value: 0.1 }).is_empty());
        // cross threshold — macro start
        assert_eq!(
            m.handle(GamepadEvent::Trigger { axis: 4, value: 0.9 }),
            vec![KeyAction::MacroStart { name: "m_left_right".into(), source_id: 23 }]
        );
        // release below threshold — macro stop
        assert_eq!(
            m.handle(GamepadEvent::Trigger { axis: 4, value: 0.0 }),
            vec![KeyAction::MacroStop { source_id: 23 }]
        );
    }

    #[test]
    fn mouse_binding_press_release() {
        let cfg = cfg_with_overrides(vec![(25, Binding::Mouse(MouseButton::Left))]);
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(GamepadEvent::ButtonDown(25)),
                   vec![KeyAction::MousePress(MouseButton::Left)]);
        assert_eq!(m.handle(GamepadEvent::ButtonUp(25)),
                   vec![KeyAction::MouseRelease(MouseButton::Left)]);
    }

    #[test]
    fn mouse_delta_passes_through_as_move() {
        let cfg = cfg_with_overrides(vec![]);
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(GamepadEvent::MouseDelta { dx: 10, dy: -5 }),
                   vec![KeyAction::MouseMove { dx: 10, dy: -5 }]);
    }

    #[test]
    fn shared_key_two_sources_emit_independently() {
        // D-pad Up (id 11) and L-stick Up (id 15) both bound to "Up".
        // gilrs: ly positive = up, so axis 1 value +0.9 activates id 15.
        let cfg = cfg_with_overrides(vec![
            (11, Binding::Key("Up".into())),
            (15, Binding::Key("Up".into())),
        ]);
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(GamepadEvent::ButtonDown(11)),
                   vec![KeyAction::Press("Up".into(), Source::Button(11))]);
        assert_eq!(m.handle(GamepadEvent::Stick { axis: 1, value: 0.9 }),
                   vec![KeyAction::Press("Up".into(), Source::Button(15))]);
        // Releasing one MUST still produce a Release action — refcount logic in
        // safety.rs decides whether to actually let the key go up.
        assert_eq!(m.handle(GamepadEvent::ButtonUp(11)),
                   vec![KeyAction::Release("Up".into(), Source::Button(11))]);
    }

    // ─── Per-pad tables, trigger modes, quick pages ─────────────────────────

    fn k(s: &str) -> Binding { Binding::Key(s.into()) }
    fn press(id: u32, s: &str) -> Vec<KeyAction> { vec![KeyAction::Press(s.into(), Source::Button(id))] }
    fn release(id: u32, s: &str) -> Vec<KeyAction> { vec![KeyAction::Release(s.into(), Source::Button(id))] }
    fn down(id: u32) -> GamepadEvent { GamepadEvent::ButtonDown(id) }
    fn up(id: u32) -> GamepadEvent { GamepadEvent::ButtonUp(id) }
    fn trig(axis: u32, value: f32) -> GamepadEvent { GamepadEvent::Trigger { axis, value } }

    fn set(cfg: &mut Config, kind: ControllerKind, page: Page, id: u32, b: Binding) {
        cfg.pad_mut(kind).table_mut(page).insert(id.to_string(), ButtonEntry { label: String::new(), binding: b });
    }

    /// Every pad: base 11 = Up, quick_l 11 = F1, quick_r 11 = F2, both
    /// triggers in page mode, trigger_threshold 0.5.
    fn quick_cfg() -> Config {
        let mut cfg = cfg_with_overrides(vec![(11, k("Up"))]);
        for kind in ControllerKind::ALL {
            set(&mut cfg, kind, Page::QuickL, 11, k("F1"));
            set(&mut cfg, kind, Page::QuickR, 11, k("F2"));
        }
        cfg
    }

    fn mapper() -> Mapper {
        Mapper::new(quick_cfg())
    }

    #[test]
    fn page_mode_trigger_sends_nothing_and_selects_quick_l() {
        let mut m = mapper();
        m.set_pad(Some(ControllerKind::DualSense));
        assert!(m.handle(trig(4, 1.0)).is_empty());
        assert_eq!(m.handle(down(11)), press(11, "F1"));
        assert_eq!(m.handle(up(11)), release(11, "F1"));
        assert!(m.handle(trig(4, 0.0)).is_empty());
        assert_eq!(m.handle(down(11)), press(11, "Up"), "page gone with the trigger");
    }

    #[test]
    fn unbound_quick_slot_sends_nothing() {
        let mut m = mapper();
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::DualSense, Page::Base, 12, k("Down"));
        set(&mut cfg, ControllerKind::DualSense, Page::QuickL, 13, Binding::Unbound);
        m.replace_config(cfg);
        m.handle(trig(4, 1.0));
        assert!(m.handle(down(12)).is_empty(), "absent page id: no fall-through to base");
        assert!(m.handle(up(12)).is_empty());
        assert!(m.handle(down(13)).is_empty(), "Unbound page id");
        assert!(m.handle(up(13)).is_empty());
    }

    #[test]
    fn non_quick_ids_keep_base_while_page_held() {
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::DualSense, Page::Base, 9, k("q"));
        let mut m = Mapper::new(cfg);
        m.handle(trig(4, 1.0));
        assert_eq!(m.handle(down(9)), press(9, "q"));
        assert_eq!(m.handle(up(9)), release(9, "q"));
    }

    #[test]
    fn release_after_trigger_up_uses_page_binding() {
        let mut m = mapper();
        m.handle(trig(4, 1.0));
        assert_eq!(m.handle(down(11)), press(11, "F1"));
        assert!(m.handle(trig(4, 0.0)).is_empty());
        assert_eq!(m.handle(up(11)), release(11, "F1"));
    }

    #[test]
    fn press_before_trigger_keeps_base_binding() {
        let mut m = mapper();
        assert_eq!(m.handle(down(11)), press(11, "Up"));
        assert!(m.handle(trig(4, 1.0)).is_empty());
        assert_eq!(m.handle(up(11)), release(11, "Up"));
    }

    #[test]
    fn page_falls_back_to_still_held_trigger() {
        let mut m = mapper();
        m.handle(trig(4, 1.0));
        m.handle(trig(5, 1.0));
        m.handle(trig(5, 0.0));
        assert_eq!(m.handle(down(11)), press(11, "F1"));
    }

    #[test]
    fn last_pressed_trigger_wins() {
        let mut m = mapper();
        m.handle(trig(4, 1.0));
        m.handle(trig(5, 1.0));
        assert_eq!(m.handle(down(11)), press(11, "F2"));
        assert_eq!(m.handle(up(11)), release(11, "F2"));
        // Re-pressing L while R is still held makes L the last one.
        m.handle(trig(4, 0.0));
        m.handle(trig(4, 1.0));
        assert_eq!(m.handle(down(11)), press(11, "F1"));
    }

    #[test]
    fn key_mode_trigger_is_an_ordinary_button() {
        let mut cfg = quick_cfg();
        cfg.pad_mut(ControllerKind::DualSense).trigger_mode.l = TriggerMode::Key;
        set(&mut cfg, ControllerKind::DualSense, Page::Base, 23, k("Q"));
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(trig(4, 1.0)), press(23, "Q"));
        assert_eq!(m.handle(down(11)), press(11, "Up"), "quick_l ignored in key mode");
        assert_eq!(m.handle(up(11)), release(11, "Up"));
        assert_eq!(m.handle(trig(4, 0.0)), release(23, "Q"));
    }

    #[test]
    fn rebind_while_held_releases_original_key() {
        let mut m = mapper();
        assert_eq!(m.handle(down(11)), press(11, "Up"));
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::DualSense, Page::Base, 11, k("W"));
        m.replace_config(cfg);
        assert_eq!(m.handle(up(11)), release(11, "Up"));
        assert_eq!(m.handle(down(11)), press(11, "W"));
    }

    #[test]
    fn trigger_mode_change_while_page_held_releases_cleanly() {
        let mut m = mapper();
        m.handle(trig(4, 1.0));
        let mut cfg = quick_cfg();
        cfg.pad_mut(ControllerKind::DualSense).trigger_mode.l = TriggerMode::Key;
        set(&mut cfg, ControllerKind::DualSense, Page::Base, 23, k("Q"));
        m.replace_config(cfg);
        assert!(m.handle(trig(4, 0.0)).is_empty(), "a page press never sent a key, so its release sends none");
        assert_eq!(m.handle(down(11)), press(11, "Up"));
    }

    #[test]
    fn set_pad_switches_tables() {
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::NsPro, Page::Base, 11, k("W"));
        let mut m = Mapper::new(cfg);
        assert_eq!(m.handle(down(11)), press(11, "Up"), "no pad: dualsense table");
        m.handle(up(11));
        m.set_pad(Some(ControllerKind::NsPro));
        assert_eq!(m.handle(down(11)), press(11, "W"));
        m.handle(up(11));
        m.set_pad(None);
        assert_eq!(m.handle(down(11)), press(11, "Up"));
    }

    #[test]
    fn ns_pro_digital_trigger_counts_once() {
        let mut m = mapper();
        m.set_pad(Some(ControllerKind::NsPro));
        assert!(m.handle(down(23)).is_empty());
        assert!(m.handle(trig(4, 1.0)).is_empty());
        assert!(m.handle(up(23)).is_empty());
        assert_eq!(m.handle(down(11)), press(11, "F1"), "still held: axis active");
        assert_eq!(m.handle(up(11)), release(11, "F1"));
        assert!(m.handle(trig(4, 0.0)).is_empty());
        assert_eq!(m.handle(down(11)), press(11, "Up"), "both sources released");
    }

    #[test]
    fn digital_and_analog_trigger_press_a_key_mode_binding_once() {
        let mut cfg = quick_cfg();
        cfg.pad_mut(ControllerKind::NsPro).trigger_mode.r = TriggerMode::Key;
        set(&mut cfg, ControllerKind::NsPro, Page::Base, 24, k("Shift"));
        let mut m = Mapper::new(cfg);
        m.set_pad(Some(ControllerKind::NsPro));
        assert_eq!(m.handle(down(24)), press(24, "Shift"));
        assert!(m.handle(trig(5, 1.0)).is_empty());
        assert!(m.handle(trig(5, 0.0)).is_empty(), "digital still down");
        assert_eq!(m.handle(up(24)), release(24, "Shift"));
    }

    #[test]
    fn analog_trigger_still_reports_visual_transitions() {
        let mut m = mapper();
        m.handle(trig(4, 1.0));
        assert_eq!(m.take_visual_transitions(), vec![(23, true)]);
        m.handle(trig(4, 0.0));
        assert_eq!(m.take_visual_transitions(), vec![(23, false)]);
    }

    #[test]
    fn page_macro_stops_on_release_after_trigger_up() {
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::DualSense, Page::QuickR, 0, Binding::Macro("m_left_right".into()));
        let mut m = Mapper::new(cfg);
        m.handle(trig(5, 1.0));
        assert_eq!(m.handle(down(0)), vec![KeyAction::MacroStart { name: "m_left_right".into(), source_id: 0 }]);
        m.handle(trig(5, 0.0));
        assert_eq!(m.handle(up(0)), vec![KeyAction::MacroStop { source_id: 0 }]);
    }

    #[test]
    fn reset_input_state_forgets_held_and_keeps_pad() {
        let mut cfg = quick_cfg();
        set(&mut cfg, ControllerKind::NsPro, Page::Base, 11, k("W"));
        let mut m = Mapper::new(cfg);
        m.set_pad(Some(ControllerKind::NsPro));
        assert_eq!(m.handle(down(11)), press(11, "W"));
        m.handle(trig(5, 1.0));
        m.reset_input_state();
        assert!(m.handle(up(11)).is_empty(), "everything was already released by the engine");
        assert_eq!(m.handle(down(11)), press(11, "W"), "same pad, no page, a new press goes through");
        assert!(m.handle(trig(5, 1.0)).is_empty(), "trigger state forgotten: a fresh page press");
        assert_eq!(m.take_visual_transitions().last(), Some(&(24, true)));
    }
}

