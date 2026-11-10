//! v3.9.0 游標: the `cursor` binding moves the mouse from a stick or a D-pad.
//! It covers a whole group of four direction slots (left stick 15..=18,
//! right stick 19..=22, D-pad 11..=14, and the D-pad of each quick page),
//! never one slot. The group rules live in Rust (iron rule 9): the config
//! validator and the binding IPC commands.

use dualsense_mapper::config::{Binding, ButtonEntry, Config};
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gamepad::ControllerKind;
use dualsense_mapper::gui::commands::{set_binding_impl, set_settings_impl, swap_bindings_impl, Settings};
use dualsense_mapper::pad_bindings::Page;

const DS: ControllerKind = ControllerKind::DualSense;

fn example() -> Config {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    Config::load_from_path(&path).unwrap()
}

fn tmp(cfg: &Config) -> tempfile::NamedTempFile {
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), serde_json::to_string_pretty(cfg).unwrap()).unwrap();
    f
}

fn set(cfg: &mut Config, page: Page, id: u32, b: Binding) {
    cfg.pad_mut(DS).table_mut(page).insert(id.to_string(), ButtonEntry { label: String::new(), binding: b });
}

fn bind(cfg: &Config, page: Page, id: u32) -> Binding {
    cfg.pad(DS).table(page).get(&id.to_string()).map(|e| e.binding.clone()).unwrap_or(Binding::Unbound)
}

fn k(s: &str) -> Binding {
    Binding::Key(s.into())
}

fn entry(b: Binding) -> ButtonEntry {
    ButtonEntry { label: "x".into(), binding: b }
}

// ── Config ────────────────────────────────────────────────────────────────

#[test]
fn cursor_binding_round_trips_json() {
    let e: ButtonEntry = serde_json::from_str(r#"{ "label": "L-stick ↑", "type": "cursor" }"#).unwrap();
    assert_eq!(e.binding, Binding::Cursor);
    let back = serde_json::to_string(&e).unwrap();
    assert!(back.contains(r#""type":"cursor""#), "got: {back}");
}

#[test]
fn validate_accepts_whole_groups_on_base_and_quick_pages() {
    let mut cfg = example();
    for id in 19..=22 {
        set(&mut cfg, Page::Base, id, Binding::Cursor);
    }
    for id in 11..=14 {
        set(&mut cfg, Page::QuickL, id, Binding::Cursor);
    }
    cfg.validate().unwrap();
}

#[test]
fn validate_rejects_cursor_outside_a_group() {
    let mut cfg = example();
    set(&mut cfg, Page::Base, 3, Binding::Cursor);
    let err = format!("{:#}", cfg.validate().unwrap_err());
    assert!(err.contains("cursor") && err.contains('3'), "got: {err}");

    let mut cfg = example();
    set(&mut cfg, Page::QuickR, 0, Binding::Cursor);
    assert!(cfg.validate().is_err(), "a quick page's face button cannot hold the cursor");
}

#[test]
fn validate_rejects_a_partial_group() {
    let mut cfg = example();
    set(&mut cfg, Page::Base, 15, Binding::Cursor);
    let err = format!("{:#}", cfg.validate().unwrap_err());
    assert!(err.contains("cursor") && err.contains("15"), "got: {err}");
}

#[test]
fn cursor_speeds_default_to_one_and_are_range_checked() {
    let cfg = example();
    assert_eq!(cfg.cursor_stick_speed, 1.0);
    assert_eq!(cfg.cursor_dpad_speed, 1.0);
    let mut bad = cfg.clone();
    bad.cursor_stick_speed = 0.05;
    assert!(format!("{:#}", bad.validate().unwrap_err()).contains("cursor_stick_speed"));
    let mut bad = cfg.clone();
    bad.cursor_dpad_speed = 10.5;
    assert!(format!("{:#}", bad.validate().unwrap_err()).contains("cursor_dpad_speed"));
}

// ── IPC: set_binding ──────────────────────────────────────────────────────

#[test]
fn set_cursor_on_one_slot_covers_its_whole_group() {
    let cfg = example();
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_binding_impl(&h, f.path(), DS, Page::Base, 17, entry(Binding::Cursor)).unwrap();
    let live = h.config_read().clone();
    for id in 15..=18 {
        assert_eq!(bind(&live, Page::Base, id), Binding::Cursor, "id {id}");
    }
    assert_eq!(live.pad(DS).buttons["15"].label, "L-stick ↑", "each slot keeps its own label");
    assert_eq!(bind(&live, Page::Base, 11), k("Up"), "the D-pad is untouched");
    let disk = Config::load_from_path(f.path()).unwrap();
    assert_eq!(bind(&disk, Page::Base, 18), Binding::Cursor);
    engine.shutdown();
}

#[test]
fn set_cursor_on_a_quick_page_d_pad() {
    let cfg = example();
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_binding_impl(&h, f.path(), DS, Page::QuickR, 12, entry(Binding::Cursor)).unwrap();
    let live = h.config_read().clone();
    for id in 11..=14 {
        assert_eq!(bind(&live, Page::QuickR, id), Binding::Cursor, "quick_r {id}");
        assert_eq!(bind(&live, Page::QuickL, id), Binding::Unbound, "quick_l {id}");
    }
    engine.shutdown();
}

#[test]
fn set_cursor_outside_a_group_is_refused_and_not_written() {
    let cfg = example();
    let f = tmp(&cfg);
    let before = std::fs::read_to_string(f.path()).unwrap();
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    assert!(set_binding_impl(&h, f.path(), DS, Page::Base, 3, entry(Binding::Cursor)).is_err());
    assert!(set_binding_impl(&h, f.path(), DS, Page::QuickL, 0, entry(Binding::Cursor)).is_err());
    assert_eq!(std::fs::read_to_string(f.path()).unwrap(), before);
    engine.shutdown();
}

#[test]
fn a_key_dropped_on_a_cursor_slot_clears_the_other_three() {
    let mut cfg = example();
    for id in 15..=18 {
        set(&mut cfg, Page::Base, id, Binding::Cursor);
    }
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_binding_impl(&h, f.path(), DS, Page::Base, 15, entry(k("w"))).unwrap();
    let live = h.config_read().clone();
    assert_eq!(bind(&live, Page::Base, 15), k("w"));
    for id in 16..=18 {
        assert_eq!(bind(&live, Page::Base, id), Binding::Unbound, "id {id}");
    }
    engine.shutdown();
}

#[test]
fn clearing_one_cursor_slot_clears_all_four() {
    let mut cfg = example();
    for id in 11..=14 {
        set(&mut cfg, Page::QuickL, id, Binding::Cursor);
    }
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_binding_impl(&h, f.path(), DS, Page::QuickL, 14, entry(Binding::Unbound)).unwrap();
    let live = h.config_read().clone();
    assert!(live.pad(DS).quick_l.is_empty(), "every quick-page slot removed: {:?}", live.pad(DS).quick_l);
    engine.shutdown();
}

// ── IPC: swap_bindings ────────────────────────────────────────────────────

#[test]
fn dragging_a_cursor_onto_another_group_swaps_the_groups() {
    let mut cfg = example();
    for id in 15..=18 {
        set(&mut cfg, Page::Base, id, Binding::Cursor);
    }
    set(&mut cfg, Page::Base, 11, k("w"));
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    // From 搖桿左 (17) onto D-pad ↓ (12): direction for direction, not slot for slot.
    swap_bindings_impl(&h, f.path(), DS, (Page::Base, 17), (Page::Base, 12)).unwrap();
    let live = h.config_read().clone();
    for id in 11..=14 {
        assert_eq!(bind(&live, Page::Base, id), Binding::Cursor, "D-pad {id}");
    }
    assert_eq!(bind(&live, Page::Base, 15), k("w"), "stick ↑ gets the D-pad's ↑");
    assert_eq!(bind(&live, Page::Base, 16), k("Down"));
    assert_eq!(bind(&live, Page::Base, 17), k("Left"));
    assert_eq!(bind(&live, Page::Base, 18), k("Right"));
    engine.shutdown();
}

#[test]
fn swapping_a_cursor_group_with_a_quick_page_d_pad() {
    let mut cfg = example();
    for id in 11..=14 {
        set(&mut cfg, Page::QuickL, id, Binding::Cursor);
    }
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    // The right stick is unbound in the example: the quick page ends up empty.
    swap_bindings_impl(&h, f.path(), DS, (Page::QuickL, 11), (Page::Base, 20)).unwrap();
    let live = h.config_read().clone();
    for id in 19..=22 {
        assert_eq!(bind(&live, Page::Base, id), Binding::Cursor, "right stick {id}");
    }
    assert!(live.pad(DS).quick_l.is_empty(), "got: {:?}", live.pad(DS).quick_l);
    engine.shutdown();
}

#[test]
fn swapping_a_cursor_slot_with_a_slot_outside_any_group_is_refused() {
    let mut cfg = example();
    for id in 15..=18 {
        set(&mut cfg, Page::Base, id, Binding::Cursor);
    }
    set(&mut cfg, Page::Base, 3, k("q"));
    let f = tmp(&cfg);
    let before = std::fs::read_to_string(f.path()).unwrap();
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    assert!(swap_bindings_impl(&h, f.path(), DS, (Page::Base, 16), (Page::Base, 3)).is_err());
    assert!(swap_bindings_impl(&h, f.path(), DS, (Page::Base, 3), (Page::Base, 16)).is_err());
    assert_eq!(std::fs::read_to_string(f.path()).unwrap(), before);
    engine.shutdown();
}

#[test]
fn a_plain_swap_between_two_groups_still_swaps_one_slot() {
    let cfg = example();
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    swap_bindings_impl(&h, f.path(), DS, (Page::Base, 15), (Page::Base, 19)).unwrap();
    let live = h.config_read().clone();
    assert_eq!(bind(&live, Page::Base, 19), k("Up"));
    assert_eq!(bind(&live, Page::Base, 15), Binding::Unbound);
    assert_eq!(bind(&live, Page::Base, 16), k("Down"), "no cursor involved: the rest stays");
    engine.shutdown();
}

// ── Settings ──────────────────────────────────────────────────────────────

#[test]
fn cursor_speeds_persist_through_set_settings() {
    let cfg = example();
    let f = tmp(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_settings_impl(&h, f.path(), Settings { cursor_stick_speed: 2.5, cursor_dpad_speed: 0.5, ..Settings::defaults() }).unwrap();
    assert_eq!(h.config_read().cursor_stick_speed, 2.5);
    let disk = Config::load_from_path(f.path()).unwrap();
    assert_eq!((disk.cursor_stick_speed, disk.cursor_dpad_speed), (2.5, 0.5));
    let err = set_settings_impl(&h, f.path(), Settings { cursor_stick_speed: 20.0, ..Settings::defaults() }).unwrap_err();
    assert!(format!("{err:#}").contains("cursor_stick_speed"), "got: {err:#}");
    engine.shutdown();
}
