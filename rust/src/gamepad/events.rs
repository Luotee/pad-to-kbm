//! Event surface produced by the gamepad source. Kept identical to the
//! v1.2.0 enum so engine.rs / mapper.rs / safety.rs / keyboard.rs
//! compile unchanged across the gilrs → hidapi migration.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GamepadEvent {
    Connected(crate::gamepad::ControllerKind),
    Disconnected,
    ButtonDown(u32),
    ButtonUp(u32),
    /// Sticks: lx=0, ly=1, rx=2, ry=3 — value in [-1.0, 1.0]
    Stick { axis: u32, value: f32 },
    /// Triggers: l2=4, r2=5 — value normalised to [0.0, 1.0]
    Trigger { axis: u32, value: f32 },
    /// Relative cursor motion from the DualSense touchpad. Bypasses
    /// the binding system — the mapper forwards it directly to the
    /// mouse sink. Values are already scaled by
    /// `touchpad_cursor_sensitivity`.
    MouseDelta { dx: i32, dy: i32 },
    /// Diagnostic — fires on every touchpad click rising edge,
    /// alongside the regular `ButtonDown(quadrant)`. Carries the raw
    /// finger coordinates so the GUI can render a debug dot at the
    /// captured position and the user can verify the chosen quadrant
    /// matches their intent.
    TouchpadClick { raw_x: u16, raw_y: u16, quadrant: u32 },
    /// Continuous touchpad hover preview. Emitted per HID frame on
    /// quadrant change (dedupe-on-change). `quadrant = 255` is the
    /// sentinel for "finger lifted, clear UI hover".
    TouchpadHover { raw_x: u16, raw_y: u16, quadrant: u32 },
    /// The pad's battery reading, sent when it changes (the first frame of
    /// a session included); `None` when a reading the pad had disappears
    /// (DualSense error status), so the board drops the stale value. The
    /// hub keeps it per connected pad and never
    /// passes it to the engine as an event; it never affects the
    /// connection state (iron rule 11).
    Battery(Option<crate::gamepad::Battery>),
}
