use crate::config::{Binding, Config, MouseButton};
use crate::gamepad::{ControllerKind, GamepadEvent};
use crate::pad_bindings::{PadBindings, Side, TriggerMode, DPAD_GROUP, QUICK_IDS, STICK_GROUPS};
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
    /// 游標: the sub-pixel part of the movement not sent yet, and how long a
    /// D-pad cursor direction has been held (its speed ramp).
    cursor_rem: (f32, f32),
    dpad_hold_s: f32,
    rng: fastrand::Rng,
}

/// 游標 (v3.9.0) speeds in px/s at 1×: a stick pushed to the edge, and a
/// D-pad from the first press to the top speed after `DPAD_RAMP_S`.
const STICK_PX_S: f32 = 1200.0;
const DPAD_START_PX_S: f32 = 150.0;
const DPAD_TOP_PX_S: f32 = 900.0;
const DPAD_RAMP_S: f32 = 0.5;
/// A stalled tick moves at most this long a step, never a jump.
const CURSOR_MAX_DT_S: f32 = 0.05;
/// Each step's length varies by ± this fraction, so the movement never
/// keeps one fixed rhythm (anti-cheat self-discipline, like `tick_jitter_ms`).
const CURSOR_JITTER: f32 = 0.1;

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
            cursor_rem: (0.0, 0.0),
            dpad_hold_s: 0.0,
            rng: fastrand::Rng::new(),
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
        self.cursor_rem = (0.0, 0.0);
        self.dpad_hold_s = 0.0;
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
            GamepadEvent::Connected(_) | GamepadEvent::Disconnected | GamepadEvent::Battery(_) => {
                Vec::new()
            }
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

    /// One engine tick of 游標 movement (v3.9.0): `dt` seconds since the
    /// last call. `None` when nothing moves.
    ///
    /// A stick whose four directions hold the cursor (base table) moves it
    /// by how far it is pushed past the deadzone, squared, so a light push
    /// aims. A D-pad direction held with the cursor (base or quick page:
    /// the binding its press resolved to, iron rule 2) moves it from
    /// `DPAD_START_PX_S`, ramping to `DPAD_TOP_PX_S`. Every source adds up.
    pub fn cursor_step(&mut self, dt: f32) -> Option<KeyAction> {
        let dt = dt.clamp(0.0, CURSOR_MAX_DT_S);
        let (mut vx, mut vy) = (0.0f32, 0.0f32);
        let pad = self.cfg.pad(self.pad);
        let dz = self.cfg.deadzone;
        for (g, (ax, ay)) in STICK_GROUPS.iter().zip([(0, 1), (2, 3)]) {
            if pad.buttons.get(&g[0].to_string()).map_or(true, |e| e.binding != Binding::Cursor) {
                continue;
            }
            let (x, y) = (self.last_axis[ax], self.last_axis[ay]);
            let r = (x * x + y * y).sqrt();
            if r <= dz {
                continue;
            }
            let n = ((r - dz) / (1.0 - dz).max(1e-3)).min(1.0);
            let speed = STICK_PX_S * self.cfg.cursor_stick_speed * n * n;
            // gilrs +y is up; the screen's +y is down.
            vx += speed * x / r;
            vy -= speed * y / r;
        }
        let held = |id: u32| i32::from(self.held.get(&id) == Some(&Binding::Cursor));
        let [up, down, left, right] = DPAD_GROUP;
        let (dx, dy) = (held(right) - held(left), held(down) - held(up));
        if dx != 0 || dy != 0 {
            self.dpad_hold_s += dt;
            let t = (self.dpad_hold_s / DPAD_RAMP_S).min(1.0);
            let speed = (DPAD_START_PX_S + (DPAD_TOP_PX_S - DPAD_START_PX_S) * t) * self.cfg.cursor_dpad_speed;
            let len = ((dx * dx + dy * dy) as f32).sqrt();
            vx += speed * dx as f32 / len;
            vy += speed * dy as f32 / len;
        } else {
            self.dpad_hold_s = 0.0;
        }
        if vx == 0.0 && vy == 0.0 {
            self.cursor_rem = (0.0, 0.0);
            return None;
        }
        let j = 1.0 + CURSOR_JITTER * (2.0 * self.rng.f32() - 1.0);
        let fx = vx * dt * j + self.cursor_rem.0;
        let fy = vy * dt * j + self.cursor_rem.1;
        let (ix, iy) = (fx.trunc(), fy.trunc());
        self.cursor_rem = (fx - ix, fy - iy);
        if ix == 0.0 && iy == 0.0 {
            return None;
        }
        Some(KeyAction::MouseMove { dx: ix as i32, dy: iy as i32 })
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
        Binding::Cursor => Vec::new(),
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
        Binding::Cursor | Binding::Unbound => Vec::new(),
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
            cursor_stick_speed: 1.0,
            cursor_dpad_speed: 1.0,
            mapping_enabled: Default::default(),
            output_mode: Default::default(),
            rumble_on_connect: true,
            stick_cal: Default::default(),
            last_pad: None,
            hidhide_prompt_dismissed: false,
            lightbar: Default::default(),
            hidhide_known: Vec::new(),
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
    // ─── 游標 (v3.9.0): a stick or a D-pad moves the mouse ──────────────────

    fn stick(axis: u32, value: f32) -> GamepadEvent { GamepadEvent::Stick { axis, value } }

    /// `ids` hold the cursor in `page` of every pad.
    fn cursor_mapper(groups: &[(Page, [u32; 4])]) -> Mapper {
        let mut cfg = quick_cfg();
        for kind in ControllerKind::ALL {
            for (page, ids) in groups {
                for id in ids {
                    set(&mut cfg, kind, *page, *id, Binding::Cursor);
                }
            }
        }
        cfg.validate().unwrap();
        let mut m = Mapper::new(cfg);
        m.set_pad(Some(ControllerKind::DualSense));
        m
    }

    fn step(m: &mut Mapper, dt: f32) -> (i32, i32) {
        match m.cursor_step(dt) {
            Some(KeyAction::MouseMove { dx, dy }) => (dx, dy),
            None => (0, 0),
            other => panic!("cursor_step returned {other:?}"),
        }
    }

    /// `n` steps of `dt`, summed.
    fn run(m: &mut Mapper, n: usize, dt: f32) -> (i32, i32) {
        (0..n).map(|_| step(m, dt)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }

    const LS: (Page, [u32; 4]) = (Page::Base, [15, 16, 17, 18]);
    const RS: (Page, [u32; 4]) = (Page::Base, [19, 20, 21, 22]);
    const DP: (Page, [u32; 4]) = (Page::Base, [11, 12, 13, 14]);
    const QL: (Page, [u32; 4]) = (Page::QuickL, [11, 12, 13, 14]);

    #[test]
    fn cursor_slots_press_nothing() {
        let mut m = cursor_mapper(&[LS, DP]);
        assert!(m.handle(stick(0, 1.0)).is_empty(), "stick right on a cursor stick types nothing");
        assert!(m.handle(down(11)).is_empty());
        assert!(m.handle(up(11)).is_empty());
        assert_eq!(m.take_visual_transitions(), vec![(18, true)], "the board still lights the slot");
    }

    #[test]
    fn stick_full_right_moves_right_at_full_speed() {
        let mut m = cursor_mapper(&[RS]);
        m.handle(stick(2, 1.0));
        let (dx, dy) = step(&mut m, 0.05);
        assert!((54..=66).contains(&dx), "1200 px/s × 0.05 s ± 10 %, got {dx}");
        assert_eq!(dy, 0);
    }

    #[test]
    fn stick_up_moves_the_cursor_up_the_screen() {
        let mut m = cursor_mapper(&[LS]);
        m.handle(stick(1, 1.0));
        let (dx, dy) = step(&mut m, 0.05);
        assert_eq!(dx, 0);
        assert!(dy <= -54, "gilrs +y is up, the screen's +y is down; got {dy}");
    }

    #[test]
    fn stick_inside_the_deadzone_does_not_move() {
        let mut m = cursor_mapper(&[LS]);
        m.handle(stick(0, 0.35));
        assert_eq!(m.cursor_step(0.1), None);
    }

    #[test]
    fn a_half_push_is_much_slower_than_a_full_push() {
        let mut m = cursor_mapper(&[LS]);
        m.handle(stick(0, 0.7));   // halfway between the 0.4 deadzone and the edge
        let (half, _) = run(&mut m, 20, 0.05);
        assert!((270..=330).contains(&half), "(0.5)² × 1200 = 300 ± 10 %, got {half}");
    }

    #[test]
    fn a_stick_without_the_cursor_does_not_move_it() {
        let mut m = cursor_mapper(&[RS]);
        m.handle(stick(0, 1.0));
        assert_eq!(m.cursor_step(0.1), None, "the left stick still types its keys");
        m.handle(stick(2, 1.0));
        assert!(step(&mut m, 0.1).0 > 0, "the right stick moves it");
    }

    #[test]
    fn d_pad_starts_slow_and_speeds_up_while_held() {
        let mut m = cursor_mapper(&[DP]);
        m.handle(down(14));
        let (first, _) = step(&mut m, 0.02);
        for _ in 0..30 {
            step(&mut m, 0.02);
        }
        let (later, _) = step(&mut m, 0.02);
        assert!(first > 0 && first <= 6, "about 150 px/s at the start, got {first}");
        assert!((16..=20).contains(&later), "900 px/s × 0.02 s ± 10 % after half a second, got {later}");
        m.handle(up(14));
        assert_eq!(m.cursor_step(0.02), None);
        m.handle(down(13));
        let (again, _) = step(&mut m, 0.02);
        assert!((-6..0).contains(&again), "a new press starts slow again, got {again}");
    }

    #[test]
    fn d_pad_diagonal_is_not_faster_than_straight() {
        let mut m = cursor_mapper(&[DP]);
        m.handle(down(11));
        m.handle(down(14));
        let (dx, dy) = run(&mut m, 20, 0.05);
        let len = ((dx * dx + dy * dy) as f32).sqrt();
        assert!(dx > 0 && dy < 0, "up-right, got ({dx}, {dy})");
        assert!(len < 1.12 * 900.0, "normalised, got {len}");
    }

    #[test]
    fn quick_page_d_pad_moves_only_while_its_trigger_is_held() {
        let mut m = cursor_mapper(&[QL]);
        assert_eq!(m.handle(down(13)), Vec::new(), "base 13 is unbound in quick_cfg");
        assert_eq!(m.cursor_step(0.1), None, "no page: the base D-pad has no cursor");
        m.handle(up(13));
        m.handle(trig(4, 1.0));
        m.handle(down(13));
        let (dx, _) = step(&mut m, 0.1);
        assert!(dx < 0, "L2 held: the quick page's ← moves left, got {dx}");
        m.handle(trig(4, 0.0));
        let (dx2, _) = step(&mut m, 0.1);
        assert!(dx2 < 0, "the press keeps the binding it started with until released (iron rule 2)");
        m.handle(up(13));
        assert_eq!(m.cursor_step(0.1), None);
    }

    #[test]
    fn several_groups_add_up() {
        let mut m = cursor_mapper(&[RS, DP]);
        m.handle(stick(2, 1.0));
        let (alone, _) = run(&mut m, 20, 0.05);
        m.handle(down(14));
        let (both, _) = run(&mut m, 20, 0.05);
        assert!(both > alone + 100, "stick {alone} + D-pad gives more, got {both}");
    }

    #[test]
    fn cursor_speed_settings_scale_the_movement() {
        let mut cfg = cursor_mapper(&[RS]).config().clone();
        cfg.cursor_stick_speed = 2.0;
        let mut m = Mapper::new(cfg);
        m.set_pad(Some(ControllerKind::DualSense));
        m.handle(stick(2, 1.0));
        let (dx, _) = step(&mut m, 0.05);
        assert!((108..=132).contains(&dx), "2 × 60 ± 10 %, got {dx}");
    }

    #[test]
    fn each_step_is_jittered_and_sub_pixels_carry_over() {
        let mut m = cursor_mapper(&[RS]);
        m.handle(stick(2, 1.0));
        let steps: Vec<i32> = (0..200).map(|_| step(&mut m, 0.008).0).collect();
        let total: i32 = steps.iter().sum();
        assert!((1728..=2112).contains(&total), "200 × 9.6 px ± 10 %, got {total}");
        let mut distinct = steps.clone();
        distinct.sort();
        distinct.dedup();
        assert!(distinct.len() >= 2, "not one fixed step: {distinct:?}");
        assert!(steps.iter().all(|s| (8..=11).contains(s)), "each step 9.6 ± 10 %: {steps:?}");
    }

    #[test]
    fn a_long_stall_does_not_jump_the_cursor() {
        let mut m = cursor_mapper(&[RS]);
        m.handle(stick(2, 1.0));
        let (dx, _) = step(&mut m, 3.0);
        assert!((54..=66).contains(&dx), "dt clamped to 50 ms: 60 ± 10 %, got {dx}");
    }

    #[test]
    fn reset_input_state_stops_the_cursor() {
        let mut m = cursor_mapper(&[RS, DP]);
        m.handle(stick(2, 1.0));
        m.handle(down(14));
        assert!(step(&mut m, 0.1).0 > 0, "moving before the reset");
        m.reset_input_state();
        assert_eq!(m.cursor_step(0.1), None);
    }
}
