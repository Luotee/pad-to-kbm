//! Button ids 30..=32 (GL / GR / C, Switch 2 Pro) — added in v2.6.0.
//! Missing entries in a v2.5 config are auto-filled as Unbound before
//! validation (iron rule 1); the validator itself still demands the full
//! 0..=32 range.

use dualsense_mapper::config::{Binding, Config};
use dualsense_mapper::config_io::ConfigDoc;
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gui::commands::set_output_mode_impl;
use std::path::PathBuf;

const SLUGS: [&str; 3] = ["dualsense", "ns_pro", "ns2_pro"];

const NEW_IDS: [(&str, &str); 3] = [
    ("30", "GL（Switch 2 背鍵左）"),
    ("31", "GR（Switch 2 背鍵右）"),
    ("32", "C（Switch 2 C 鍵）"),
];

fn v25_example_value() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (id, _) in NEW_IDS {
        for slug in SLUGS {
            let removed = v["pads"][slug]["buttons"].as_object_mut().unwrap().remove(id);
            assert!(removed.is_some(), "example must ship button {id} on {slug}");
        }
    }
    v
}

#[test]
fn v25_config_without_30_32_autofills() {
    let v = v25_example_value();
    let s = serde_json::to_string(&v).unwrap();
    let cfg = Config::load_from_str(&s).expect("v2.5 config must load via auto-fill");
    let doc = ConfigDoc::load_from_value(v).expect("doc load");
    for (id, label) in NEW_IDS {
        let e = &cfg.pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons[id];
        assert!(matches!(e.binding, Binding::Unbound));
        assert_eq!(e.label, label);
        let e = &doc.typed().pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons[id];
        assert!(matches!(e.binding, Binding::Unbound));
        assert_eq!(e.label, label);
    }
}

#[test]
fn validate_requires_32() {
    let mut v = v25_example_value();
    // Keep 30 and 31 so only 32 is missing.
    for (id, label) in &NEW_IDS[..2] {
        for slug in SLUGS {
            v["pads"][slug]["buttons"][*id] = serde_json::json!({"label": label, "type": "unbound"});
        }
    }
    let cfg: Config = serde_json::from_value(v).unwrap();
    assert!(!cfg.pad(dualsense_mapper::gamepad::ControllerKind::DualSense).buttons.contains_key("32"));
    let err = cfg.validate().unwrap_err();
    assert!(format!("{err:#}").contains("32"), "got: {err:#}");
}

#[test]
fn v25_file_round_trips_30_32_and_keeps_help() {
    let v = v25_example_value();
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
    for (id, label) in NEW_IDS {
        for slug in SLUGS {
            assert_eq!(disk["pads"][slug]["buttons"][id]["label"], serde_json::json!(label));
            assert_eq!(disk["pads"][slug]["buttons"][id]["type"], serde_json::json!("unbound"));
        }
    }
    assert_eq!(disk["_help"], help_before);
    engine.shutdown();
}
