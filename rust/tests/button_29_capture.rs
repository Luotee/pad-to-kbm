//! Button id 29 (Capture, Switch) — added in v2.5.0. Missing "29" in a v2.4
//! config is auto-filled as Unbound before validation (iron rule 1); the
//! validator itself still demands the full 0..=29 range.

use dualsense_mapper::config::{Binding, Config};
use dualsense_mapper::config_io::ConfigDoc;
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gui::commands::set_output_mode_impl;
use std::path::PathBuf;

const SLUGS: [&str; 3] = ["dualsense", "ns_pro", "ns2_pro"];

const LABEL: &str = "Capture（Switch 擷取鍵）";

fn v24_example_value() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for slug in SLUGS {
        let removed = v["pads"][slug]["buttons"].as_object_mut().unwrap().remove("29");
        assert!(removed.is_some(), "example must ship button 29 on {slug}");
    }
    v
}

#[test]
fn v24_config_without_29_autofills_capture() {
    let v = v24_example_value();
    let s = serde_json::to_string(&v).unwrap();
    let cfg = Config::load_from_str(&s).expect("v2.4 config must load via auto-fill");
    let e = &cfg.pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons["29"];
    assert!(matches!(e.binding, Binding::Unbound));
    assert_eq!(e.label, LABEL);

    let doc = ConfigDoc::load_from_value(v).expect("doc load");
    let e = &doc.typed().pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons["29"];
    assert!(matches!(e.binding, Binding::Unbound));
    assert_eq!(e.label, LABEL);
}

#[test]
fn validate_requires_29() {
    let v = v24_example_value();
    let cfg: Config = serde_json::from_value(v).unwrap();
    assert!(!cfg.pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons.contains_key("29"));
    let err = cfg.validate().unwrap_err();
    assert!(format!("{err:#}").contains("29"), "got: {err:#}");
}

#[test]
fn v24_file_round_trips_29_and_keeps_help() {
    let v = v24_example_value();
    let help_before = v["_help"].clone();
    assert!(!help_before.is_null());
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let cfg = Config::load_from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    set_output_mode_impl(&h, tmp.path(), "dualsense", "pad").unwrap();

    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    for slug in SLUGS {
        assert_eq!(disk["pads"][slug]["buttons"]["29"]["label"], serde_json::json!(LABEL));
        assert_eq!(disk["pads"][slug]["buttons"]["29"]["type"], serde_json::json!("unbound"));
    }
    assert_eq!(disk["_help"], help_before);
    engine.shutdown();
}
