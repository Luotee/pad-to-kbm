use dualsense_mapper::config_io::{write_atomic, ConfigDoc};
use serde_json::json;

// Full 25-button FIXTURE (buttons 0..=24).  Buttons not relevant to the
// test intent are set to "unbound"; the macro reference on button 23 is
// satisfied by the "macro_A" entry in "macros".
const FIXTURE: &str = r#"{
  "_help": ["this is doc"],
  "_keyboard_keys": { "letters": ["a", "b"] },
  "version": 1,
  "deadzone": 0.4,
  "trigger_threshold": 0.5,
  "min_press_ms": [8, 25],
  "tick_jitter_ms": [0, 3],
  "log_events": true,
  "buttons": {
    "0":  { "label": "Cross",      "type": "key",     "value": "x" },
    "1":  { "label": "Circle",     "type": "unbound" },
    "2":  { "label": "Square",     "type": "unbound" },
    "3":  { "label": "Triangle",   "type": "unbound" },
    "4":  { "label": "Share",      "type": "unbound" },
    "5":  { "label": "PS",         "type": "unbound" },
    "6":  { "label": "Options",    "type": "unbound" },
    "7":  { "label": "L3",         "type": "unbound" },
    "8":  { "label": "R3",         "type": "unbound" },
    "9":  { "label": "L1",         "type": "unbound" },
    "10": { "label": "R1",         "type": "unbound" },
    "11": { "label": "DPad Up",    "type": "unbound" },
    "12": { "label": "DPad Down",  "type": "unbound" },
    "13": { "label": "DPad Left",  "type": "unbound" },
    "14": { "label": "DPad Right", "type": "unbound" },
    "15": { "label": "LS Up",      "type": "unbound" },
    "16": { "label": "LS Down",    "type": "unbound" },
    "17": { "label": "LS Left",    "type": "unbound" },
    "18": { "label": "LS Right",   "type": "unbound" },
    "19": { "label": "RS Up",      "type": "unbound" },
    "20": { "label": "RS Down",    "type": "unbound" },
    "21": { "label": "RS Left",    "type": "unbound" },
    "22": { "label": "RS Right",   "type": "unbound" },
    "23": { "label": "L2",         "type": "macro",   "value": "macro_A" },
    "24": { "label": "R2",         "type": "unbound" }
  },
  "macros": {
    "macro_A": {
      "loop": false,
      "steps": [
        { "key": "Left", "action": "down", "delay_ms": [50, 80] },
        { "key": "Left", "action": "up",   "delay_ms": [15, 25] }
      ]
    }
  }
}"#;

#[test]
fn round_trip_preserves_help_fields() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), FIXTURE).unwrap();

    let doc = ConfigDoc::load(tmp.path()).expect("load");
    write_atomic(tmp.path(), &doc).expect("write");

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    assert_eq!(on_disk["_help"], json!(["this is doc"]));
    assert_eq!(on_disk["_keyboard_keys"]["letters"], json!(["a", "b"]));
}

#[test]
fn updating_deadzone_preserves_help_and_changes_value() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), FIXTURE).unwrap();

    let mut doc = ConfigDoc::load(tmp.path()).unwrap();
    doc.set_deadzone(0.6);
    write_atomic(tmp.path(), &doc).unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    assert_eq!(on_disk["_help"], json!(["this is doc"]));
    // 0.6 cannot be represented exactly as f32; allow epsilon tolerance.
    let dz = on_disk["deadzone"].as_f64().unwrap();
    assert!((dz - 0.6_f64).abs() < 1e-4, "deadzone should be ~0.6, got {dz}");
}

#[test]
fn invalid_payload_fails_validation_and_does_not_touch_disk() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), FIXTURE).unwrap();

    let mut doc = ConfigDoc::load(tmp.path()).unwrap();
    doc.set_min_press_ms_unchecked([25, 8]); // min >= max — invalid
    let err = doc.validate().expect_err("should reject");
    assert!(format!("{err}").contains("min_press_ms"));

    let on_disk = std::fs::read_to_string(tmp.path()).unwrap();
    assert_eq!(on_disk.trim(), FIXTURE.trim(), "disk untouched on validation failure");
}

#[test]
fn set_mapping_enabled_preserves_help() {
    use dualsense_mapper::gamepad::ControllerKind;
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let orig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&example).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    std::fs::copy(&example, &path).unwrap();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.set_mapping_enabled(ControllerKind::DualSense, false);
    write_atomic(&path, &doc).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["mapping_enabled"]["dualsense"], false);
    assert_eq!(raw["_help"], orig["_help"]);
}

#[test]
fn set_lightbar_writes_raw_and_typed_and_keeps_help() {
    use dualsense_mapper::gamepad::lightbar::{Effect, LightbarConfig};
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let orig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&example).unwrap()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    std::fs::copy(&example, &path).unwrap();
    let mut doc = ConfigDoc::load(&path).unwrap();
    let mut lb = LightbarConfig::default();
    lb.kbm.effect = Effect::Breathe;
    lb.other.brightness = 30;
    doc.set_lightbar(lb);
    assert_eq!(doc.typed().lightbar, lb);
    write_atomic(&path, &doc).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["lightbar"]["kbm"]["effect"], "breathe");
    assert_eq!(raw["lightbar"]["other"]["brightness"], 30);
    assert_eq!(raw["_help"], orig["_help"]);
}

// ─── Per-pad / per-page writes (config version 2) ────────────────────────────

use dualsense_mapper::config::{Binding, ButtonEntry, Config};
use dualsense_mapper::gamepad::ControllerKind;
use dualsense_mapper::pad_bindings::{Page, Side, TriggerMode};

fn example_copy() -> (tempfile::TempDir, std::path::PathBuf) {
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    std::fs::copy(&example, &path).unwrap();
    (dir, path)
}

fn key(k: &str) -> ButtonEntry {
    ButtonEntry { label: "slot".into(), binding: Binding::Key(k.into()) }
}

#[test]
fn loading_v1_and_writing_saves_version_2_pads() {
    let maple = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/maple_artale.json");
    let orig: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&maple).unwrap()).unwrap();
    assert_eq!(orig["version"], 1, "fixture is a v1 file");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    std::fs::copy(&maple, &path).unwrap();

    let doc = ConfigDoc::load(&path).unwrap();
    write_atomic(&path, &doc).unwrap();

    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], 2);
    assert!(v.get("buttons").is_none(), "old top-level buttons dropped");
    assert_eq!(v["pads"]["ns2_pro"]["trigger_mode"]["l"], "key");
    assert_eq!(v["pads"]["dualsense"]["buttons"]["0"], orig["buttons"]["0"]);
    assert_eq!(v["pads"]["ns_pro"]["buttons"]["32"]["type"], "unbound", "added ids filled in raw");
    assert_eq!(v["_help"], orig["_help"]);
    // And the saved file loads as a plain v2 file.
    Config::load_from_path(&path).unwrap();
}

#[test]
fn replace_binding_on_quick_page_round_trips() {
    let (_d, path) = example_copy();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.replace_binding(ControllerKind::DualSense, Page::QuickL, 11, key("F1"));
    write_atomic(&path, &doc).unwrap();
    let cfg = Config::load_from_path(&path).unwrap();
    assert_eq!(cfg.pad(ControllerKind::DualSense).quick_l["11"].binding, Binding::Key("F1".into()));
    assert!(!cfg.pad(ControllerKind::NsPro).quick_l.contains_key("11"));
    assert_eq!(doc.typed().pad(ControllerKind::DualSense).quick_l["11"].binding, Binding::Key("F1".into()));
}

#[test]
fn replace_binding_on_base_touches_one_pad() {
    let (_d, path) = example_copy();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.replace_binding(ControllerKind::Ns2Pro, Page::Base, 0, key("z"));
    write_atomic(&path, &doc).unwrap();
    let cfg = Config::load_from_path(&path).unwrap();
    assert_eq!(cfg.pad(ControllerKind::Ns2Pro).buttons["0"].binding, Binding::Key("z".into()));
    assert_eq!(cfg.pad(ControllerKind::DualSense).buttons["0"].binding, Binding::Unbound);
}

#[test]
fn unbound_on_quick_page_removes_the_key() {
    let (_d, path) = example_copy();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.replace_binding(ControllerKind::DualSense, Page::QuickL, 11, key("F1"));
    write_atomic(&path, &doc).unwrap();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.replace_binding(
        ControllerKind::DualSense,
        Page::QuickL,
        11,
        ButtonEntry { label: "slot".into(), binding: Binding::Unbound },
    );
    assert!(!doc.typed().pad(ControllerKind::DualSense).quick_l.contains_key("11"));
    write_atomic(&path, &doc).unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v["pads"]["dualsense"]["quick_l"].get("11").is_none());
    let cfg = Config::load_from_path(&path).unwrap();
    assert!(!cfg.pad(ControllerKind::DualSense).quick_l.contains_key("11"));
}

#[test]
fn set_trigger_mode_round_trips() {
    let (_d, path) = example_copy();
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.set_trigger_mode(ControllerKind::NsPro, Side::R, TriggerMode::Key);
    write_atomic(&path, &doc).unwrap();
    let cfg = Config::load_from_path(&path).unwrap();
    assert_eq!(cfg.pad(ControllerKind::NsPro).trigger_mode.r, TriggerMode::Key);
    assert_eq!(cfg.pad(ControllerKind::NsPro).trigger_mode.l, TriggerMode::Page);
    assert_eq!(cfg.pad(ControllerKind::DualSense).trigger_mode.r, TriggerMode::Page);
}

#[test]
fn replace_pad_round_trips() {
    let (_d, path) = example_copy();
    let mut doc = ConfigDoc::load(&path).unwrap();
    let mut pad = doc.typed().pad(ControllerKind::NsPro).cleared();
    pad.quick_r.insert("3".into(), key("F9"));
    doc.replace_pad(ControllerKind::NsPro, pad.clone());
    write_atomic(&path, &doc).unwrap();
    let cfg = Config::load_from_path(&path).unwrap();
    assert_eq!(cfg.pad(ControllerKind::NsPro), &pad);
}

#[test]
fn stick_cal_round_trips_and_removes() {
    use dualsense_mapper::gamepad::stick_cal::{AxisCal, PadStickCal, StickCal};
    let (_d, path) = example_copy();
    let a = AxisCal { center: 0.05, neg: 0.9, pos: 0.95 };
    let cal = PadStickCal { left: StickCal { x: a, y: AxisCal::IDENTITY }, right: StickCal { x: AxisCal::IDENTITY, y: a } };
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.set_stick_cal(ControllerKind::Ns2Pro, cal);
    write_atomic(&path, &doc).unwrap();
    assert_eq!(Config::load_from_path(&path).unwrap().stick_cal["ns2_pro"], cal);
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.remove_stick_cal(ControllerKind::Ns2Pro);
    write_atomic(&path, &doc).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(raw.get("stick_cal").is_none(), "empty table removed from the file");
}
