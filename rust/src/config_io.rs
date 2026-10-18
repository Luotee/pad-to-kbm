//! Configuration on-disk reader/writer that preserves unknown JSON fields.
//!
//! Typed Config gives us validation; serde_json::Value gives us byte-for-byte
//! round-trip of `_help`, `_keyboard_keys`, and any future "_*" doc fields.
//! We carry both.

use crate::config::{migrate_v1, Binding, ButtonEntry, Config, MacroDef, ADDED_IDS};
use crate::gamepad::ControllerKind;
use crate::pad_bindings::{PadBindings, Page, Side, TriggerMode};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct ConfigDoc {
    raw: Value,
    typed: Config,
}

impl ConfigDoc {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut raw: Value = serde_json::from_str(&bytes)
            .with_context(|| format!("parsing {} as JSON", path.display()))?;
        migrate_v1(&mut raw);
        let mut typed: Config = serde_json::from_value(raw.clone())
            .with_context(|| format!("typed-parsing {}", path.display()))?;
        migrate_touchpad_ids(&mut typed, &mut raw);
        typed.validate()?;
        Ok(Self { raw, typed })
    }

    /// Build a `ConfigDoc` from an already-typed value tree. Used by tests
    /// that seed a known config without touching the filesystem first.
    pub fn load_from_value(mut raw: Value) -> Result<Self> {
        migrate_v1(&mut raw);
        let mut typed: Config =
            serde_json::from_value(raw.clone()).context("typed-parsing in-memory config")?;
        migrate_touchpad_ids(&mut typed, &mut raw);
        typed.validate()?;
        Ok(Self { raw, typed })
    }

    pub fn typed(&self) -> &Config {
        &self.typed
    }

    pub fn typed_mut(&mut self) -> &mut Config {
        &mut self.typed
    }

    pub fn set_deadzone(&mut self, v: f32) {
        self.typed.deadzone = v;
        self.raw["deadzone"] = Value::from(v);
    }

    pub fn set_trigger_threshold(&mut self, v: f32) {
        self.typed.trigger_threshold = v;
        self.raw["trigger_threshold"] = Value::from(v);
    }

    pub fn set_log_events(&mut self, v: bool) {
        self.typed.log_events = v;
        self.raw["log_events"] = Value::from(v);
    }

    pub fn set_touchpad_cursor_enabled(&mut self, v: bool) {
        self.typed.touchpad_cursor_enabled = v;
        self.raw["touchpad_cursor_enabled"] = Value::from(v);
    }

    pub fn set_touchpad_cursor_sensitivity(&mut self, v: f32) {
        self.typed.touchpad_cursor_sensitivity = v;
        self.raw["touchpad_cursor_sensitivity"] = Value::from(v);
    }

    pub fn set_touchpad_midpoint_x(&mut self, v: u16) {
        self.typed.touchpad_midpoint_x = v;
        self.raw["touchpad_midpoint_x"] = Value::from(v);
    }

    pub fn set_touchpad_midpoint_y(&mut self, v: u16) {
        self.typed.touchpad_midpoint_y = v;
        self.raw["touchpad_midpoint_y"] = Value::from(v);
    }

    pub fn set_touchpad_accel_slow_threshold(&mut self, v: u32) {
        self.typed.touchpad_accel_slow_threshold = v;
        self.raw["touchpad_accel_slow_threshold"] = Value::from(v);
    }

    pub fn set_touchpad_accel_fast_threshold(&mut self, v: u32) {
        self.typed.touchpad_accel_fast_threshold = v;
        self.raw["touchpad_accel_fast_threshold"] = Value::from(v);
    }

    pub fn set_touchpad_accel_gain_slow(&mut self, v: f32) {
        self.typed.touchpad_accel_gain_slow = v;
        self.raw["touchpad_accel_gain_slow"] = Value::from(v);
    }

    pub fn set_touchpad_accel_gain_fast(&mut self, v: f32) {
        self.typed.touchpad_accel_gain_fast = v;
        self.raw["touchpad_accel_gain_fast"] = Value::from(v);
    }

    pub fn set_touchpad_deadzone_radius(&mut self, v: u32) {
        self.typed.touchpad_deadzone_radius = v;
        self.raw["touchpad_deadzone_radius"] = Value::from(v);
    }

    pub fn set_stick_cal(&mut self, kind: ControllerKind, cal: crate::gamepad::stick_cal::PadStickCal) {
        self.typed.stick_cal.insert(kind.slug().to_string(), cal);
        if !self.raw["stick_cal"].is_object() {
            self.raw["stick_cal"] = serde_json::json!({});
        }
        self.raw["stick_cal"][kind.slug()] = serde_json::to_value(cal).unwrap();
    }

    /// Back to the factory calibration; drops the `stick_cal` key when empty.
    pub fn remove_stick_cal(&mut self, kind: ControllerKind) {
        self.typed.stick_cal.remove(kind.slug());
        if let Some(obj) = self.raw.as_object_mut() {
            let empty = match obj.get_mut("stick_cal").and_then(Value::as_object_mut) {
                Some(t) => {
                    t.remove(kind.slug());
                    t.is_empty()
                }
                None => false,
            };
            if empty {
                obj.remove("stick_cal");
            }
        }
    }

    pub fn set_rumble_on_connect(&mut self, v: bool) {
        self.typed.rumble_on_connect = v;
        self.raw["rumble_on_connect"] = Value::from(v);
    }

    pub fn set_last_pad(&mut self, pad: &str) {
        self.typed.last_pad = Some(pad.to_string());
        self.raw["last_pad"] = Value::from(pad);
    }

    pub fn set_touchpad_click_freeze_enabled(&mut self, v: bool) {
        self.typed.touchpad_click_freeze_enabled = v;
        self.raw["touchpad_click_freeze_enabled"] = Value::from(v);
    }

    pub fn set_mapping_enabled(&mut self, kind: crate::gamepad::ControllerKind, enabled: bool) {
        self.typed.mapping_enabled.insert(kind.slug().to_string(), enabled);
        if !self.raw["mapping_enabled"].is_object() {
            self.raw["mapping_enabled"] = serde_json::json!({});
        }
        self.raw["mapping_enabled"][kind.slug()] = Value::Bool(enabled);
    }

    /// Set min_press_ms without running validation.
    /// Callers that need the invariant enforced must call `validate()` afterwards.
    pub fn set_min_press_ms_unchecked(&mut self, v: [u32; 2]) {
        self.typed.min_press_ms = v;
        self.raw["min_press_ms"] = serde_json::json!([v[0], v[1]]);
    }

    /// Set tick_jitter_ms without running validation.
    /// Callers that need the invariant enforced must call `validate()` afterwards.
    pub fn set_tick_jitter_ms_unchecked(&mut self, v: [u32; 2]) {
        self.typed.tick_jitter_ms = v;
        self.raw["tick_jitter_ms"] = serde_json::json!([v[0], v[1]]);
    }

    /// Set one binding in one table of one pad. On a quick page an
    /// `Unbound` entry removes the id instead of storing it (a missing page
    /// id already sends nothing).
    pub fn replace_binding(&mut self, kind: ControllerKind, page: Page, id: u32, entry: ButtonEntry) {
        let k = id.to_string();
        let slug = kind.slug();
        let field = page_field(page);
        if page != Page::Base && entry.binding == Binding::Unbound {
            if let Some(obj) = self.raw["pads"][slug][field].as_object_mut() {
                obj.remove(&k);
            }
            self.typed.pad_mut(kind).table_mut(page).remove(&k);
            return;
        }
        self.raw["pads"][slug][field][&k] = serde_json::to_value(&entry).unwrap();
        self.typed.pad_mut(kind).table_mut(page).insert(k, entry);
    }

    pub fn set_trigger_mode(&mut self, kind: ControllerKind, side: Side, mode: TriggerMode) {
        self.typed.pad_mut(kind).trigger_mode.set(side, mode);
        let modes = self.typed.pad(kind).trigger_mode;
        self.raw["pads"][kind.slug()]["trigger_mode"] = serde_json::to_value(modes).unwrap();
    }

    /// Replace one pad's whole table.
    pub fn replace_pad(&mut self, kind: ControllerKind, pad: PadBindings) {
        self.raw["pads"][kind.slug()] = serde_json::to_value(&pad).unwrap();
        self.typed.pads.insert(kind.slug().to_string(), pad);
    }

    pub fn replace_macros(&mut self, macros: BTreeMap<String, MacroDef>) {
        self.raw["macros"] = serde_json::to_value(&macros).unwrap();
        self.typed.macros = macros;
    }

    pub fn validate(&self) -> Result<()> {
        self.typed.validate()
    }

    pub fn pretty(&self) -> String {
        serde_json::to_string_pretty(&self.raw).expect("Value is always serialisable")
    }
}

/// Atomically write `doc` to `target`.
///
/// Calls `doc.validate()` first — returns an error without touching the
/// filesystem if validation fails.  Otherwise writes to a sibling `.tmp`
/// file and renames it into place so readers never see a partial write.
/// Migrate older configs by inserting missing ids added since v2.0
/// (`ADDED_IDS`: 25..=32) as `Unbound` into both the typed and the raw JSON view.
/// The raw patch keeps the next `write_atomic` round-trip honest.
fn migrate_touchpad_ids(typed: &mut Config, raw: &mut Value) {
    typed.fill_missing_pads();
    typed.fill_added_id_defaults();
    if !raw["pads"].is_object() {
        raw["pads"] = serde_json::json!({});
    }
    for k in ControllerKind::ALL {
        let slug = k.slug();
        if raw["pads"][slug].is_null() {
            raw["pads"][slug] = serde_json::to_value(typed.pad(k)).unwrap();
            continue;
        }
        for (id, label) in ADDED_IDS {
            let key = id.to_string();
            if raw["pads"][slug]["buttons"][&key].is_null() {
                raw["pads"][slug]["buttons"][&key] = serde_json::to_value(&ButtonEntry {
                    label: (*label).to_string(),
                    binding: Binding::Unbound,
                }).unwrap();
            }
        }
    }
    if raw["touchpad_cursor_enabled"].is_null() {
        raw["touchpad_cursor_enabled"] = Value::from(typed.touchpad_cursor_enabled);
    }
    if raw["touchpad_cursor_sensitivity"].is_null() {
        raw["touchpad_cursor_sensitivity"] = Value::from(typed.touchpad_cursor_sensitivity);
    }
    if raw["touchpad_midpoint_x"].is_null() {
        raw["touchpad_midpoint_x"] = Value::from(typed.touchpad_midpoint_x);
    }
    if raw["touchpad_midpoint_y"].is_null() {
        raw["touchpad_midpoint_y"] = Value::from(typed.touchpad_midpoint_y);
    }
}

/// The JSON field holding `page` inside a pad.
fn page_field(page: Page) -> &'static str {
    match page {
        Page::Base => "buttons",
        Page::QuickL => "quick_l",
        Page::QuickR => "quick_r",
    }
}

pub fn write_atomic(target: &Path, doc: &ConfigDoc) -> Result<()> {
    doc.validate().with_context(|| "validating before write")?;
    let mut tmp: PathBuf = target.to_path_buf();
    let fname = format!("{}.tmp", target.file_name().unwrap().to_string_lossy());
    tmp.set_file_name(fname);
    std::fs::write(&tmp, doc.pretty())
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, target)
        .with_context(|| format!("renaming {} → {}", tmp.display(), target.display()))?;
    Ok(())
}
