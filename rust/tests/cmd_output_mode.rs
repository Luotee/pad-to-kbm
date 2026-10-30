mod test_helpers;

use dualsense_mapper::config::OutputMode;
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gamepad::ControllerKind;
use dualsense_mapper::gui::commands::set_output_mode_impl;

#[test]
fn persists_and_updates_live_engine() {
    let cfg = test_helpers::load_example();
    let cfg_path = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let h = engine.handle();

    set_output_mode_impl(&h, cfg_path.path(), "dualsense", "xbox").unwrap();

    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cfg_path.path()).unwrap()).unwrap();
    assert_eq!(disk["output_mode"]["dualsense"], serde_json::json!("xbox"));
    assert_eq!(h.config_read().output_mode_for(ControllerKind::DualSense), OutputMode::Xbox);
    engine.shutdown();
}

#[test]
fn drops_the_legacy_entry_on_disk_and_in_the_engine() {
    let mut cfg = test_helpers::load_example();
    cfg.mapping_enabled.insert("dualsense".into(), false);
    cfg.mapping_enabled.insert("ns2_pro".into(), false);
    let cfg_path = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let h = engine.handle();

    set_output_mode_impl(&h, cfg_path.path(), "dualsense", "kbm").unwrap();

    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cfg_path.path()).unwrap()).unwrap();
    assert_eq!(disk["mapping_enabled"], serde_json::json!({"ns2_pro": false}));
    let live = h.config_read();
    assert!(!live.mapping_enabled.contains_key("dualsense"));
    assert_eq!(live.output_mode_for(ControllerKind::DualSense), OutputMode::Kbm);
    assert_eq!(live.output_mode_for(ControllerKind::Ns2Pro), OutputMode::Xbox);
    drop(live);
    engine.shutdown();
}

#[test]
fn rejects_unknown_kind_or_mode_and_leaves_disk_untouched() {
    let cfg = test_helpers::load_example();
    let cfg_path = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let h = engine.handle();

    let before = std::fs::read(cfg_path.path()).unwrap();
    let err = set_output_mode_impl(&h, cfg_path.path(), "ps5", "xbox").unwrap_err();
    assert!(format!("{err:#}").contains("ps5"), "error should name the kind; got: {err:#}");
    let err = set_output_mode_impl(&h, cfg_path.path(), "dualsense", "joystick").unwrap_err();
    assert!(format!("{err:#}").contains("joystick"), "error should name the mode; got: {err:#}");
    assert_eq!(before, std::fs::read(cfg_path.path()).unwrap());
    assert_eq!(h.config_read().output_mode_for(ControllerKind::DualSense), OutputMode::Kbm);
    engine.shutdown();
}

#[test]
fn old_config_without_field_gains_it() {
    let cfg = test_helpers::load_example();
    let mut v = serde_json::to_value(&cfg).unwrap();
    v.as_object_mut().unwrap().remove("mapping_enabled");
    v.as_object_mut().unwrap().remove("output_mode");
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let h = engine.handle();

    set_output_mode_impl(&h, f.path(), "dualsense", "pad").unwrap();

    let disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(f.path()).unwrap()).unwrap();
    let map = disk["output_mode"].as_object().expect("field gained");
    assert_eq!(map.len(), 1, "only dualsense present; got {map:?}");
    assert_eq!(map["dualsense"], serde_json::json!("pad"));
    assert!(disk.get("mapping_enabled").is_none());
    engine.shutdown();
}
