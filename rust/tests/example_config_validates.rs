use dualsense_mapper::config::Config;
use dualsense_mapper::gamepad::ControllerKind;
use dualsense_mapper::pad_bindings::default_pad_bindings;
use std::path::PathBuf;

#[test]
fn shipped_example_config_validates() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::load_from_path(&path).expect("load+validate");
    assert_eq!(cfg.version, 2);
    for k in ControllerKind::ALL {
        assert_eq!(cfg.pad(k).buttons.len(), 33, "{k:?}");
    }
}

#[test]
fn example_file_is_version_2_with_default_pads() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::load_from_path(&path).expect("load+validate");
    assert_eq!(cfg.version, 2);
    assert!(cfg.macros.is_empty(), "spec 4.8: no macros by default");
    // Read the file's own pads, not the filled-in ones: the example must
    // carry all three tables itself.
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for k in ControllerKind::ALL {
        assert!(raw["pads"][k.slug()].is_object(), "example lists pad {}", k.slug());
        let want = default_pad_bindings(k);
        let got = cfg.pad(k);
        assert_eq!(got.trigger_mode, want.trigger_mode, "{k:?}");
        assert_eq!(got.quick_l, want.quick_l, "{k:?}");
        assert_eq!(got.quick_r, want.quick_r, "{k:?}");
        for (id, e) in &want.buttons {
            assert_eq!(got.buttons[id].binding, e.binding, "{k:?} id {id}");
        }
    }
}

#[test]
fn shipped_example_config_has_rumble_on_connect() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let text = std::fs::read_to_string(&path).expect("read example config");
    let cfg = Config::load_from_path(&path).expect("load+validate");
    assert!(cfg.rumble_on_connect);
    assert!(text.contains("\"rumble_on_connect\""), "example config must carry the key");
}

#[test]
fn shipped_example_config_has_default_lightbar() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::load_from_path(&path).expect("load+validate");
    assert_eq!(cfg.lightbar, dualsense_mapper::gamepad::lightbar::LightbarConfig::default());
}

#[test]
fn shipped_maple_profile_validates() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/maple_artale.json");
    Config::load_from_path(&path).expect("maple profile load+validate");
}

#[test]
fn example_help_is_accurate_about_applying_edits() {
    // The GUI's file watcher applies a saved edit; the --cli mode has no
    // watcher, so the help must not promise auto-apply without saying so.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let help: Vec<&str> = raw["_help"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let line = help
        .iter()
        .find(|l| l.contains("automatically"))
        .expect("a help line says how edits are applied");
    assert!(line.contains("--cli") && line.to_lowercase().contains("restart"), "got: {line}");
}

/// README FAQ: the event log is something the user turns on. The first-run
/// config (a copy of this file) must not write padtokbm-events.log (v3.4.0).
#[test]
fn first_run_config_does_not_log_events() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::load_from_path(&path).expect("load+validate");
    assert!(!cfg.log_events);
}

/// v3.7.0: the Nano's table carries the Nano's own button names (the same
/// labels `default_pad_bindings` writes), and the first-run help says how
/// its buttons map.
#[test]
fn example_nano_table_and_help_use_the_nano_names() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let want = default_pad_bindings(ControllerKind::XboxNano);
    let table = &raw["pads"]["xbox_nano"]["buttons"];
    for (id, e) in &want.buttons {
        assert_eq!(table[id]["label"].as_str(), Some(e.label.as_str()), "xbox_nano id {id}");
    }
    for (id, name) in [("0", "A"), ("1", "B"), ("2", "X"), ("3", "Y"), ("9", "LB"), ("10", "RB")] {
        assert_eq!(want.buttons[id].label, name, "id {id}");
    }
    for (id, name) in [("4", "View"), ("5", "Xbox"), ("6", "Menu"), ("7", "L3"), ("8", "R3"), ("23", "LT"), ("24", "RT"), ("29", "Share")] {
        assert!(want.buttons[id].label.contains(name), "id {id}: {}", want.buttons[id].label);
    }
    // The other pads keep the shared labels.
    assert_eq!(default_pad_bindings(ControllerKind::NsPro).buttons["0"].label, "Cross（叉叉）");

    let help: Vec<&str> = raw["_help"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let line = help.iter().find(|l| l.starts_with("Xbox Nano")).expect("a help line for the Nano");
    for part in ["A=0", "View=4", "Xbox=5", "Menu=6", "LT=23", "RT=24", "Share=29"] {
        assert!(line.contains(part), "{part} missing: {line}");
    }
}
