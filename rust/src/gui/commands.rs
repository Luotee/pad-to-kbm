//! Tauri IPC commands exposed to the frontend.
//!
//! Every function in this module is registered via `generate_handler![]` in
//! `runtime.rs`. JS invokes them with `window.__TAURI__.core.invoke(name, args)`.
//!
//! Iron rule #9: commands are the gate between JS and Rust. JS may NOT
//! synthesise keys, may NOT decide mappings. Read-only commands (like
//! `get_config`) are trivially safe. Write commands go through `safety` +
//! engine.
//!
//! Pattern: each command has a `*_impl` sibling that is Tauri-free and can be
//! called directly from integration tests (which cannot construct
//! `tauri::State`). The `#[tauri::command]` wrappers are gated on the `gui`
//! feature; the `*_impl` functions are always compiled.

use crate::config::{Binding, ButtonEntry, Config, MacroDef, ADDED_IDS};
use crate::config_io::{write_atomic, ConfigDoc};
use crate::engine::Handle;
use crate::gamepad::lightbar::LightbarConfig;
use crate::gamepad::ControllerKind;
use crate::pad_bindings::{
    default_pad_bindings, PadBindings, Page, Side, TriggerMode, LABELS, QUICK_IDS,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(feature = "gui")]
use std::path::PathBuf;

// ─── Tauri command wrappers (gui feature only) ───────────────────────────────

#[cfg(feature = "gui")]
use tauri::State;
#[cfg(feature = "gui")]
use tauri::Emitter as _;

/// Return the current live config.
///
/// The config is held in a `RwLock` inside the engine's `Handle`. We clone
/// it here so the lock is not held across the async serialisation boundary.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn get_config(engine: State<'_, Handle>) -> Result<Config, String> {
    Ok(engine.config_read().clone())
}

/// Tauri command: replace one button binding, validate, atomically write to
/// disk, and hot-rebind the live engine — all or nothing.
///
/// On validation failure the disk is untouched and the live config unchanged.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_binding(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
    page: Page,
    id: u32,
    entry: ButtonEntry,
) -> Result<(), String> {
    set_binding_impl(&*engine, &config_path, pad, page, id, entry).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Tauri command: swap two slots of one pad (base or page) in one write, so
/// a drag-swap cannot tear. JS passes `pageA`, `idA`, `pageB`, `idB`.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn swap_bindings(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
    page_a: Page,
    id_a: u32,
    page_b: Page,
    id_b: u32,
) -> Result<(), String> {
    swap_bindings_impl(&*engine, &config_path, pad, (page_a, id_a), (page_b, id_b))
        .map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Tauri command: set one trigger's mode (`page` / `key`) on one pad.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_trigger_mode(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
    side: Side,
    mode: TriggerMode,
) -> Result<(), String> {
    set_trigger_mode_impl(&*engine, &config_path, pad, side, mode).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Tauri command: 全部清除 — unbind every slot of one pad.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn clear_pad(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
) -> Result<(), String> {
    clear_pad_impl(&*engine, &config_path, pad).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Tauri command: 恢復預設 — one pad back to the default table.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn reset_pad(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
) -> Result<(), String> {
    reset_pad_impl(&*engine, &config_path, pad).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Tauri command: write one pad's whole table (復原 / 重做).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_pad_tables(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
    tables: PadBindings,
) -> Result<(), String> {
    set_pad_tables_impl(&*engine, &config_path, pad, tables).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

// ─── Pure *_impl helpers (always compiled, no Tauri dependency) ──────────────

/// Pure implementation of `get_config` callable without `tauri::State`.
///
/// Exposed for integration tests; the Tauri wrapper above delegates here.
#[allow(dead_code)]
pub fn get_config_impl(engine: &Handle) -> Config {
    engine.config_read().clone()
}

/// Load the on-disk config, patch it, validate, write it atomically, then
/// replace the live engine config with the result (which bumps the
/// generation the mapper watches). Any failure leaves disk and engine
/// unchanged.
fn mutate_config(
    engine: &Handle,
    config_path: &Path,
    patch: impl FnOnce(&mut ConfigDoc) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut doc = ConfigDoc::load(config_path)?;
    patch(&mut doc)?;
    doc.validate()?;
    write_atomic(config_path, &doc)?;
    *engine.config_write() = doc.typed().clone();
    Ok(())
}

/// A quick page holds only `QUICK_IDS`; the base table any id `0..=32`.
fn check_slot(page: Page, id: u32) -> anyhow::Result<()> {
    if !crate::config::VALID_BUTTON_IDS.contains(&id) {
        anyhow::bail!("unknown button id {id}; allowed range is 0..=32");
    }
    if page != Page::Base && !QUICK_IDS.contains(&id) {
        anyhow::bail!("button id {id} is not allowed on a quick page (only 0..=3 and 11..=14)");
    }
    Ok(())
}

/// Pure implementation of `set_binding` callable without `tauri::State`.
///
/// Sets one slot (`page`, `id`) of one pad. On a quick page an `Unbound`
/// entry removes the slot. Disk and engine change together or not at all.
pub fn set_binding_impl(
    engine: &Handle,
    config_path: &Path,
    pad: ControllerKind,
    page: Page,
    id: u32,
    entry: ButtonEntry,
) -> anyhow::Result<()> {
    check_slot(page, id)?;
    mutate_config(engine, config_path, |doc| {
        doc.replace_binding(pad, page, id, entry);
        Ok(())
    })
}

/// Pure implementation of `swap_bindings`: the two slots exchange their
/// bindings (each slot keeps its own label) in one load and one write.
/// An empty page slot counts as `Unbound`.
pub fn swap_bindings_impl(
    engine: &Handle,
    config_path: &Path,
    pad: ControllerKind,
    a: (Page, u32),
    b: (Page, u32),
) -> anyhow::Result<()> {
    check_slot(a.0, a.1)?;
    check_slot(b.0, b.1)?;
    mutate_config(engine, config_path, |doc| {
        let slot = |doc: &ConfigDoc, (page, id): (Page, u32)| -> ButtonEntry {
            doc.typed()
                .pad(pad)
                .table(page)
                .get(&id.to_string())
                .cloned()
                .unwrap_or_else(|| ButtonEntry {
                    label: LABELS[id as usize].to_string(),
                    binding: Binding::Unbound,
                })
        };
        let ea = slot(doc, a);
        let eb = slot(doc, b);
        doc.replace_binding(pad, a.0, a.1, ButtonEntry { label: ea.label, binding: eb.binding });
        doc.replace_binding(pad, b.0, b.1, ButtonEntry { label: eb.label, binding: ea.binding });
        Ok(())
    })
}

/// Pure implementation of `set_trigger_mode`.
pub fn set_trigger_mode_impl(
    engine: &Handle,
    config_path: &Path,
    pad: ControllerKind,
    side: Side,
    mode: TriggerMode,
) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        doc.set_trigger_mode(pad, side, mode);
        Ok(())
    })
}

/// Pure implementation of `clear_pad`: every base slot unbound (labels
/// kept), both pages empty, trigger modes kept.
pub fn clear_pad_impl(engine: &Handle, config_path: &Path, pad: ControllerKind) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        let cleared = doc.typed().pad(pad).cleared();
        doc.replace_pad(pad, cleared);
        Ok(())
    })
}

/// Pure implementation of `reset_pad`: the default table (spec 4.8).
pub fn reset_pad_impl(engine: &Handle, config_path: &Path, pad: ControllerKind) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        doc.replace_pad(pad, default_pad_bindings(pad));
        Ok(())
    })
}

/// Pure implementation of `set_pad_tables`: one pad's whole table, checked
/// like a load (ids added since v2.0 are filled in, then validated).
pub fn set_pad_tables_impl(
    engine: &Handle,
    config_path: &Path,
    pad: ControllerKind,
    mut tables: PadBindings,
) -> anyhow::Result<()> {
    for (id, label) in ADDED_IDS {
        tables.buttons.entry(id.to_string()).or_insert_with(|| ButtonEntry {
            label: (*label).to_string(),
            binding: Binding::Unbound,
        });
    }
    mutate_config(engine, config_path, |doc| {
        doc.replace_pad(pad, tables);
        Ok(())
    })
}

/// Every pad and page with every binding to macro `name` rewritten by
/// `f` (`None` = unbind: base slots become `Unbound`, page slots are
/// removed).
fn rewrite_macro_refs(doc: &mut ConfigDoc, name: &str, f: impl Fn() -> Option<Binding>) {
    for kind in ControllerKind::ALL {
        let mut pad = doc.typed().pad(kind).clone();
        let mut changed = false;
        for page in [Page::Base, Page::QuickL, Page::QuickR] {
            let table = pad.table_mut(page);
            let hits: Vec<String> = table
                .iter()
                .filter(|(_, e)| matches!(e.binding, Binding::Macro(ref n) if n == name))
                .map(|(id, _)| id.clone())
                .collect();
            for id in hits {
                changed = true;
                match (f(), page) {
                    (Some(b), _) => table.get_mut(&id).expect("listed above").binding = b,
                    (None, Page::Base) => table.get_mut(&id).expect("listed above").binding = Binding::Unbound,
                    (None, _) => {
                        table.remove(&id);
                    }
                }
            }
        }
        if changed {
            doc.replace_pad(kind, pad);
        }
    }
}

// ─── set_macro ───────────────────────────────────────────────────────────────

/// Tauri command: insert or replace a named macro, validate, atomically write
/// to disk, and hot-rebind the live engine.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_macro(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    name: String,
    def: MacroDef,
) -> Result<(), String> {
    set_macro_impl(&*engine, &config_path, name, def).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `set_macro` callable without `tauri::State`.
pub fn set_macro_impl(
    engine: &Handle,
    config_path: &Path,
    name: String,
    def: MacroDef,
) -> anyhow::Result<()> {
    let mut doc = ConfigDoc::load(config_path)?;
    let mut macros = doc.typed().macros.clone();
    macros.insert(name, def);
    doc.replace_macros(macros);
    doc.validate()?;
    write_atomic(config_path, &doc)?;
    engine.config_write().macros = doc.typed().macros.clone();
    Ok(())
}

// ─── delete_macro ────────────────────────────────────────────────────────────

/// Tauri command: delete a named macro and unbind it on every pad and page.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn delete_macro(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    name: String,
) -> Result<(), String> {
    delete_macro_impl(&*engine, &config_path, &name).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `delete_macro` callable without `tauri::State`.
///
/// Unbinds the macro everywhere first — every pad, every page (spec 4.6:
/// deleting a macro also unbinds it) — then removes it, in one write.
pub fn delete_macro_impl(
    engine: &Handle,
    config_path: &Path,
    name: &str,
) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        rewrite_macro_refs(doc, name, || None);
        let mut macros = doc.typed().macros.clone();
        macros.remove(name);
        doc.replace_macros(macros);
        Ok(())
    })
}

// ─── rename_macro ────────────────────────────────────────────────────────────

/// Tauri command: rename a macro and update all button bindings that reference
/// the old name.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn rename_macro(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    old: String,
    new: String,
) -> Result<(), String> {
    rename_macro_impl(&*engine, &config_path, &old, &new).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `rename_macro` callable without `tauri::State`.
///
/// Renames the macro in the macros map and rewrites every binding, on every
/// pad and page, that referenced the old name.
pub fn rename_macro_impl(
    engine: &Handle,
    config_path: &Path,
    old: &str,
    new: &str,
) -> anyhow::Result<()> {
    if old == new {
        return Ok(());
    }
    mutate_config(engine, config_path, |doc| {
        if !doc.typed().macros.contains_key(old) {
            anyhow::bail!("macro '{old}' does not exist");
        }
        if doc.typed().macros.contains_key(new) {
            anyhow::bail!("macro '{new}' already exists");
        }
        let mut macros = doc.typed().macros.clone();
        let def = macros.remove(old).expect("just checked");
        macros.insert(new.to_string(), def);
        doc.replace_macros(macros);
        // Every binding, on every pad and page, follows the new name.
        rewrite_macro_refs(doc, old, || Some(Binding::Macro(new.to_string())));
        Ok(())
    })
}

// ─── Settings ────────────────────────────────────────────────────────────────

/// Shape of the Settings tab form on the frontend. Deserialised from a
/// `#[tauri::command]` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub deadzone: f32,
    pub trigger_threshold: f32,
    pub min_press_ms: [u32; 2],
    pub tick_jitter_ms: [u32; 2],
    pub log_events: bool,
    #[serde(default = "default_touchpad_cursor_enabled")]
    pub touchpad_cursor_enabled: bool,
    #[serde(default = "default_touchpad_cursor_sensitivity")]
    pub touchpad_cursor_sensitivity: f32,
    #[serde(default = "default_touchpad_midpoint_x")]
    pub touchpad_midpoint_x: u16,
    #[serde(default = "default_touchpad_midpoint_y")]
    pub touchpad_midpoint_y: u16,
    #[serde(default = "default_touchpad_accel_slow_threshold")]
    pub touchpad_accel_slow_threshold: u32,
    #[serde(default = "default_touchpad_accel_fast_threshold")]
    pub touchpad_accel_fast_threshold: u32,
    #[serde(default = "default_touchpad_accel_gain_slow")]
    pub touchpad_accel_gain_slow: f32,
    #[serde(default = "default_touchpad_accel_gain_fast")]
    pub touchpad_accel_gain_fast: f32,
    #[serde(default = "default_touchpad_deadzone_radius")]
    pub touchpad_deadzone_radius: u32,
    #[serde(default = "default_touchpad_click_freeze_enabled")]
    pub touchpad_click_freeze_enabled: bool,
}

fn default_touchpad_cursor_enabled() -> bool { true }
fn default_touchpad_cursor_sensitivity() -> f32 { 1.5 }
fn default_touchpad_midpoint_x() -> u16 { 960 }
fn default_touchpad_midpoint_y() -> u16 { 540 }
fn default_touchpad_accel_slow_threshold() -> u32 { 5 }
fn default_touchpad_accel_fast_threshold() -> u32 { 20 }
fn default_touchpad_accel_gain_slow() -> f32 { 0.5 }
fn default_touchpad_accel_gain_fast() -> f32 { 1.5 }
fn default_touchpad_deadzone_radius() -> u32 { 2 }
fn default_touchpad_click_freeze_enabled() -> bool { true }

impl Settings {
    /// Spec §9 — the factory defaults that ship with the example config.
    pub fn defaults() -> Self {
        Self {
            deadzone: 0.4,
            trigger_threshold: 0.5,
            min_press_ms: [8, 25],
            tick_jitter_ms: [0, 3],
            log_events: true,
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
        }
    }
}

/// Tauri command: apply settings from the frontend Settings tab, validate,
/// atomically write to disk, and hot-reload the live engine.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_settings(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    settings: Settings,
) -> Result<(), String> {
    set_settings_impl(&*engine, &config_path, settings).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `set_settings` callable without `tauri::State`.
pub fn set_settings_impl(
    engine: &Handle,
    config_path: &Path,
    s: Settings,
) -> anyhow::Result<()> {
    let mut doc = ConfigDoc::load(config_path)?;
    doc.set_deadzone(s.deadzone);
    doc.set_trigger_threshold(s.trigger_threshold);
    doc.set_min_press_ms_unchecked(s.min_press_ms);
    doc.set_tick_jitter_ms_unchecked(s.tick_jitter_ms);
    doc.set_log_events(s.log_events);
    doc.set_touchpad_cursor_enabled(s.touchpad_cursor_enabled);
    doc.set_touchpad_cursor_sensitivity(s.touchpad_cursor_sensitivity);
    doc.set_touchpad_midpoint_x(s.touchpad_midpoint_x);
    doc.set_touchpad_midpoint_y(s.touchpad_midpoint_y);
    doc.set_touchpad_accel_slow_threshold(s.touchpad_accel_slow_threshold);
    doc.set_touchpad_accel_fast_threshold(s.touchpad_accel_fast_threshold);
    doc.set_touchpad_accel_gain_slow(s.touchpad_accel_gain_slow);
    doc.set_touchpad_accel_gain_fast(s.touchpad_accel_gain_fast);
    doc.set_touchpad_deadzone_radius(s.touchpad_deadzone_radius);
    doc.set_touchpad_click_freeze_enabled(s.touchpad_click_freeze_enabled);
    doc.validate()?;
    write_atomic(config_path, &doc)?;
    let mut live = engine.config_write();
    live.deadzone = s.deadzone;
    live.trigger_threshold = s.trigger_threshold;
    live.min_press_ms = s.min_press_ms;
    live.tick_jitter_ms = s.tick_jitter_ms;
    live.log_events = s.log_events;
    live.touchpad_cursor_enabled = s.touchpad_cursor_enabled;
    live.touchpad_cursor_sensitivity = s.touchpad_cursor_sensitivity;
    live.touchpad_midpoint_x = s.touchpad_midpoint_x;
    live.touchpad_midpoint_y = s.touchpad_midpoint_y;
    live.touchpad_accel_slow_threshold = s.touchpad_accel_slow_threshold;
    live.touchpad_accel_fast_threshold = s.touchpad_accel_fast_threshold;
    live.touchpad_accel_gain_slow = s.touchpad_accel_gain_slow;
    live.touchpad_accel_gain_fast = s.touchpad_accel_gain_fast;
    live.touchpad_deadzone_radius = s.touchpad_deadzone_radius;
    live.touchpad_click_freeze_enabled = s.touchpad_click_freeze_enabled;
    drop(live);
    // Update the HID worker's atomics so cursor / quadrant changes take
    // effect on the next decoded frame without an engine restart.
    let params = engine.cursor_params();
    params.set_enabled(s.touchpad_cursor_enabled);
    params.set_sensitivity(s.touchpad_cursor_sensitivity);
    params.set_midpoint_x(s.touchpad_midpoint_x);
    params.set_midpoint_y(s.touchpad_midpoint_y);
    params.set_accel_slow_threshold(s.touchpad_accel_slow_threshold);
    params.set_accel_fast_threshold(s.touchpad_accel_fast_threshold);
    params.set_accel_gain_slow(s.touchpad_accel_gain_slow);
    params.set_accel_gain_fast(s.touchpad_accel_gain_fast);
    params.set_deadzone_radius(s.touchpad_deadzone_radius);
    params.set_click_freeze_enabled(s.touchpad_click_freeze_enabled);
    Ok(())
}

/// Tauri command: reset all settings to factory defaults (Spec §9).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn reset_settings(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
) -> Result<(), String> {
    reset_settings_impl(&*engine, &config_path).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `reset_settings` callable without `tauri::State`.
pub fn reset_settings_impl(
    engine: &Handle,
    config_path: &Path,
) -> anyhow::Result<()> {
    set_settings_impl(engine, config_path, Settings::defaults())
}

// ─── set_mapping_enabled ──────────────────────────────────────────────────────

/// Tauri command: turn mapping ON/OFF for one controller kind (persisted).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_mapping_enabled(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    kind: String,
    enabled: bool,
) -> Result<(), String> {
    set_mapping_enabled_impl(&*engine, &config_path, &kind, enabled)
        .map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `set_mapping_enabled` callable without `tauri::State`.
///
/// Unknown `kind` slugs are rejected before touching disk.
pub fn set_mapping_enabled_impl(
    engine: &Handle,
    config_path: &Path,
    kind: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let k = crate::gamepad::ControllerKind::from_slug(kind)
        .ok_or_else(|| anyhow::anyhow!("unknown controller kind '{kind}'"))?;
    let mut doc = ConfigDoc::load(config_path)?;
    doc.set_mapping_enabled(k, enabled);
    doc.validate()?;
    write_atomic(config_path, &doc)?;
    engine
        .config_write()
        .mapping_enabled
        .insert(k.slug().to_string(), enabled);
    Ok(())
}

// ─── set_pad_off ─────────────────────────────────────────────────────────────

/// Tauri command: the header's 關 for one pad (`kind` slug). Runtime only:
/// not written to the config, so no `config-changed`; the engine re-sends
/// `controller-status` with `off_pads`. Returns the pads now off (slugs).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_pad_off(engine: State<'_, Handle>, kind: String, off: bool) -> Result<Vec<String>, String> {
    set_pad_off_impl(&engine, &kind, off).map_err(|e| format!("{e:#}"))
}

/// Pure implementation of `set_pad_off`. Unknown slugs are rejected.
pub fn set_pad_off_impl(engine: &Handle, kind: &str, off: bool) -> anyhow::Result<Vec<String>> {
    let k = ControllerKind::from_slug(kind).ok_or_else(|| anyhow::anyhow!("unknown controller kind '{kind}'"))?;
    engine.set_pad_off(k, off);
    Ok(engine.off_pads().into_iter().map(|k| k.slug().to_string()).collect())
}

// ─── ViGEmBus (Switch 2 Pro Xbox output) ─────────────────────────────────────

/// Tauri command: `"installed" | "missing" | "unsupported"`.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub async fn get_vigem_status() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| crate::vigem_setup::status().to_string())
        .await
        .map_err(|e| e.to_string())
}

/// Tauri command: download the pinned official ViGEmBus installer, verify
/// it and run it (iron rule 7); returns the status after the installer
/// exits. When installed, the engine gets one more virtual-pad plug.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub async fn install_vigem(engine: State<'_, Handle>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(crate::vigem_setup::install)
        .await
        .map_err(|e| e.to_string())??;
    let status = tauri::async_runtime::spawn_blocking(|| crate::vigem_setup::status())
        .await
        .map_err(|e| e.to_string())?;
    if status == "installed" {
        engine.retry_virtual_pad();
    }
    Ok(status.to_string())
}

// ─── Stick calibration ───────────────────────────────────────────────────────

/// Tauri command: save one pad's stick calibration (validated by the reject
/// rules in `gamepad::stick_cal::check`).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn save_stick_cal(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
    cal: crate::gamepad::stick_cal::PadStickCal,
) -> Result<(), String> {
    save_stick_cal_impl(&*engine, &config_path, pad, cal).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `save_stick_cal`.
pub fn save_stick_cal_impl(
    engine: &Handle,
    config_path: &Path,
    pad: ControllerKind,
    cal: crate::gamepad::stick_cal::PadStickCal,
) -> anyhow::Result<()> {
    crate::gamepad::stick_cal::check(&cal).map_err(|e| anyhow::anyhow!(e))?;
    mutate_config(engine, config_path, |doc| {
        doc.set_stick_cal(pad, cal);
        Ok(())
    })
}

/// Tauri command: 恢復出廠校正 — back to the pad's factory calibration.
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn reset_stick_cal(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    pad: ControllerKind,
) -> Result<(), String> {
    reset_stick_cal_impl(&*engine, &config_path, pad).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `reset_stick_cal`.
pub fn reset_stick_cal_impl(engine: &Handle, config_path: &Path, pad: ControllerKind) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        doc.remove_stick_cal(pad);
        Ok(())
    })
}

/// Tauri command: the owner's latest raw and corrected stick positions
/// (polled by 設定 → 搖桿 while it is open).
#[cfg(feature = "gui")]
#[tauri::command]
pub fn get_stick_positions(engine: State<'_, Handle>) -> Option<crate::gamepad::stick_cal::StickPositions> {
    engine.stick_positions()
}

// ─── set_rumble_on_connect ───────────────────────────────────────────────────

/// Tauri command: the 連線震動 switch (`rumble_on_connect`, global).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_rumble_on_connect(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    enabled: bool,
) -> Result<(), String> {
    set_rumble_on_connect_impl(&*engine, &config_path, enabled).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `set_rumble_on_connect`.
pub fn set_rumble_on_connect_impl(engine: &Handle, config_path: &Path, enabled: bool) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        doc.set_rumble_on_connect(enabled);
        Ok(())
    })
}

// ─── set_lightbar ────────────────────────────────────────────────────────────

/// Tauri command: the whole 燈條 setting (`lightbar`, global).
#[cfg_attr(feature = "gui", tauri::command)]
#[cfg(feature = "gui")]
pub fn set_lightbar(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    lightbar: LightbarConfig,
) -> Result<(), String> {
    set_lightbar_impl(&*engine, &config_path, lightbar).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit("config-changed", serde_json::json!({"source": "ipc"}));
    Ok(())
}

/// Pure implementation of `set_lightbar`.
pub fn set_lightbar_impl(engine: &Handle, config_path: &Path, lightbar: LightbarConfig) -> anyhow::Result<()> {
    mutate_config(engine, config_path, |doc| {
        doc.set_lightbar(lightbar);
        Ok(())
    })
}

/// Tauri command: the config file's full path, shown in 設定 → 一般.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn get_config_path(config_path: State<'_, PathBuf>) -> String {
    config_path.display().to_string()
}

// ─── pause_mapper ─────────────────────────────────────────────────────────────

/// Tauri command: pause or unpause the mapper engine.
///
/// When paused the engine still receives gamepad events but suppresses all
/// key synthesis. Already-held keys are released immediately on pause
/// (Iron rule #2 — `set_paused` calls `release_all_held`).
#[cfg(feature = "gui")]
#[tauri::command]
pub fn pause_mapper(
    engine: tauri::State<'_, crate::engine::Handle>,
    paused: bool,
) -> Result<(), String> {
    engine.set_paused(paused);
    Ok(())
}

// ─── open_config_in_editor ───────────────────────────────────────────────────

/// Tauri command: open the config file in the platform default text editor.
///
/// Windows: Notepad. macOS: default text editor via `open -t`. Linux: xdg-open.
/// The command is fire-and-forget; errors are surfaced as IPC error strings.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn open_config_in_editor(
    config_path: tauri::State<'_, std::path::PathBuf>,
) -> Result<(), String> {
    open_path(&*config_path).map_err(|e| format!("{e:#}"))
}

/// Tauri command: open the folder that holds the config file in the
/// platform file manager (Windows Explorer, macOS Finder, Linux xdg-open).
#[cfg(feature = "gui")]
#[tauri::command]
pub fn open_config_folder(
    config_path: tauri::State<'_, std::path::PathBuf>,
) -> Result<(), String> {
    let dir = config_folder(&config_path);
    open_folder(&dir).map_err(|e| format!("{e:#}"))
}

/// The directory that holds `config_path` (`.` for a bare file name).
pub fn config_folder(config_path: &Path) -> std::path::PathBuf {
    match config_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

#[cfg(feature = "gui")]
fn open_folder(dir: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Explorer returns exit code 1 even on success; only a spawn
        // failure is an error.
        std::process::Command::new("explorer")
            .arg(dir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching explorer: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(dir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching open: {e}"))?;
    }
    #[cfg(all(target_os = "linux", feature = "gui"))]
    {
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching xdg-open: {e}"))?;
    }
    Ok(())
}

#[cfg(feature = "gui")]
fn open_path(path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("notepad")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching notepad: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-t")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching open: {e}"))?;
    }
    #[cfg(all(target_os = "linux", feature = "gui"))]
    {
        // GUI is Windows-only in v0.2.0 but the lib still compiles on Linux;
        // use xdg-open as a courtesy.
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching xdg-open: {e}"))?;
    }
    Ok(())
}

// ─── quit ─────────────────────────────────────────────────────────────────────

/// Tauri command: exit the process cleanly via Tauri's event loop.
///
/// `app.exit(0)` ends the event loop: `App::run_return` in `runtime::run`
/// returns, `engine.shutdown()` fires and `KeyboardSink::drop` releases held
/// keys (Iron rule #3), then the process exits. This IPC wrapper exists for
/// frontend use (e.g. Settings tab About → Quit button).
#[cfg(feature = "gui")]
#[tauri::command]
pub fn quit(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Tauri command: toggle the engine's capture-active flag.
///
/// Spec §11.2: while the frontend's key-capture surface is focused (bind
/// popup Key segment, macro step Key field), it must not pick up
/// self-synthesised keys. The frontend flips this flag around the capture
/// window; the engine pauses synth while it's set but still emits events.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn set_capture_active(
    engine: State<'_, Handle>,
    active: bool,
) -> Result<(), String> {
    engine.set_capture_active(active);
    Ok(())
}

/// Tauri command: turn a controller off. `pad` names it (`null`: the one
/// in use, the session owner); a pad that is not connected is a no-op. The
/// request goes through `SourceHub` to that pad's worker: a DualSense
/// gets the HID Turn Off (IOCTL), a Switch Pro the sleep subcommand, and a
/// Switch 2 Pro the NS2 Turn Off (06/02) followed by a BLE disconnect. The
/// workers stay alive and return to searching; pressing a button on the pad
/// reconnects it with no re-pairing. The frontend's "Turn Off" button in
/// the toolbar invokes this.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn disconnect_gamepad(
    engine: State<'_, Handle>,
    pad: Option<ControllerKind>,
) -> Result<(), String> {
    engine.disconnect(pad);
    Ok(())
}

/// Tauri command: return the current controller connection state.
///
/// The returned `ControllerStatus` carries the controller `kind` and any
/// `warnings` alongside the connected flag.
///
/// The engine emits `controller-status` events on connect/disconnect, but
/// Tauri events don't replay missed emissions — a pad already paired before
/// the frontend's `listen()` registers is invisible to the UI otherwise.
/// Frontend polls this once on init to seed the status indicator.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn get_controller_status(
    engine: State<'_, Handle>,
) -> Result<Option<crate::engine::ControllerStatus>, String> {
    Ok(engine.current_status())
}

// ─── UiPrefs ──────────────────────────────────────────────────────────────────

/// Tauri command: the board's UI state (the pad it showed last).
///
/// v3.4.0: read from the config (`last_pad`), not from a second file.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn get_ui_prefs(engine: State<'_, Handle>) -> Result<crate::gui::ui_prefs::UiPrefs, String> {
    Ok(get_ui_prefs_impl(&engine))
}

/// Pure implementation of `get_ui_prefs`.
pub fn get_ui_prefs_impl(engine: &Handle) -> crate::gui::ui_prefs::UiPrefs {
    crate::gui::ui_prefs::UiPrefs { last_pad: engine.config_read().last_pad.clone() }
}

/// Tauri command: remember the pad the board shows (`last_pad` in the
/// config, v3.4.0). Writes only when it changed, and then emits
/// `config-changed` (iron rule 10) marked `ui` so the activity log does not
/// report a settings change for a pad switch.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn set_ui_prefs(
    app: tauri::AppHandle,
    engine: State<'_, Handle>,
    config_path: State<'_, PathBuf>,
    prefs: crate::gui::ui_prefs::UiPrefs,
) -> Result<(), String> {
    if set_ui_prefs_impl(&engine, &config_path, prefs).map_err(|e| format!("{e:#}"))? {
        let _ = app.emit("config-changed", serde_json::json!({"source": "ipc", "ui": true}));
    }
    Ok(())
}

/// Pure implementation of `set_ui_prefs`: `true` when the config was written.
pub fn set_ui_prefs_impl(
    engine: &Handle,
    config_path: &Path,
    prefs: crate::gui::ui_prefs::UiPrefs,
) -> anyhow::Result<bool> {
    let Some(pad) = prefs.last_pad else { return Ok(false) };
    if engine.config_read().last_pad.as_deref() == Some(pad.as_str()) {
        return Ok(false);
    }
    mutate_config(engine, config_path, |doc| {
        doc.set_last_pad(&pad);
        Ok(())
    })?;
    Ok(true)
}

/// Tauri command: return the build-time `CARGO_PKG_VERSION`.
///
/// Iron rule #10 (v1.2.0): the Settings → About box must read the
/// Cargo version at runtime. Hardcoding a string in `settings.js` is
/// how v1.0.4–v1.1.4 shipped with stale "v0.2.0" displayed under the
/// version bumps.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn get_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Tauri command: open a URL in the OS default browser.
///
/// The webview itself does not navigate `<a target="_blank">` links —
/// CSP `default-src 'self'` blocks it and Tauri 2 does not ship a
/// shell-open default in `withGlobalTauri`. Frontend intercepts anchor
/// clicks and calls this command. Only http:/https: URLs accepted to
/// avoid `file:`/`javascript:` smuggling.
#[cfg(feature = "gui")]
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("refusing to open non-http(s) URL: {url}"));
    }
    open_external_url(&url).map_err(|e| format!("{e:#}"))
}

#[cfg(feature = "gui")]
fn open_external_url(url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching default browser: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching open: {e}"))?;
    }
    #[cfg(all(target_os = "linux", feature = "gui"))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow::anyhow!("launching xdg-open: {e}"))?;
    }
    Ok(())
}
