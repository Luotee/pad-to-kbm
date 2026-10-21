mod test_helpers;

use dualsense_mapper::engine::Engine;
use dualsense_mapper::gamepad::lightbar::{Effect, LightMode, LightbarConfig, Rgb, Speed};
use dualsense_mapper::gui::commands::set_lightbar_impl;
use std::time::{Duration, Instant};

fn sample() -> LightbarConfig {
    let mut lb = LightbarConfig::default();
    lb.mode = LightMode::Custom;
    lb.kbm.effect = Effect::Breathe;
    lb.kbm.color = Rgb([0x2E, 0xA8, 0xFF]);
    lb.kbm.speed = Speed::Fast;
    lb.kbm.brightness = 50;
    lb.other.effect = Effect::Fixed;
    lb
}

#[test]
fn set_lightbar_writes_file_and_engine() {
    let cfg = test_helpers::load_example();
    let cfg_path = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let handle = engine.handle();
    let want = sample();

    set_lightbar_impl(&handle, cfg_path.path(), want).unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(cfg_path.path()).unwrap()).unwrap();
    let back: LightbarConfig = serde_json::from_value(on_disk["lightbar"].clone()).unwrap();
    assert_eq!(back, want);
    assert_eq!(handle.config_read().lightbar, want);

    // The edit must reach the engine-owned live state the HID worker reads.
    let deadline = Instant::now() + Duration::from_secs(1);
    while handle.lightbar_config() != want {
        assert!(Instant::now() < deadline, "lightbar edit never reached the live state");
        std::thread::sleep(Duration::from_millis(10));
    }
    engine.shutdown();
}

#[test]
fn set_lightbar_rejects_bad_brightness_and_leaves_file() {
    let cfg = test_helpers::load_example();
    let cfg_path = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let handle = engine.handle();
    let before = std::fs::read(cfg_path.path()).unwrap();

    let mut bad = LightbarConfig::default();
    bad.kbm.brightness = 85;
    let err = set_lightbar_impl(&handle, cfg_path.path(), bad).unwrap_err();
    assert!(format!("{err:#}").contains("brightness"), "got: {err:#}");

    assert_eq!(std::fs::read(cfg_path.path()).unwrap(), before);
    engine.shutdown();
}
