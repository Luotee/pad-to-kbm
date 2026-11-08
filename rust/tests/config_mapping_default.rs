//! v3.2.0: every pad defaults to keyboard/mouse output (鍵鼠). Existing
//! configs keep what they say: an explicit legacy `mapping_enabled`
//! `"ns2_pro": false` stays Xbox (v3.6.0: read through `output_mode_for`).

#[allow(dead_code)]
mod test_helpers;

use dualsense_mapper::config::{Config, OutputMode};
use dualsense_mapper::gamepad::ControllerKind;

#[test]
fn missing_key_means_mapping_on_for_every_pad() {
    let mut cfg = test_helpers::load_example();
    cfg.mapping_enabled.clear();
    for k in ControllerKind::ALL {
        assert!(cfg.mapping_enabled_for(k), "{k:?}");
    }
}

#[test]
fn explicit_false_still_wins() {
    let mut cfg = test_helpers::load_example();
    cfg.mapping_enabled.clear();
    cfg.mapping_enabled.insert("ns2_pro".into(), false);
    assert!(!cfg.mapping_enabled_for(ControllerKind::Ns2Pro));
    assert!(cfg.mapping_enabled_for(ControllerKind::DualSense));
}

#[test]
fn example_config_maps_every_pad() {
    // v3.6.0: the example says `output_mode` (kbm) for every pad and no
    // longer carries the legacy `mapping_enabled`.
    let text = include_str!("../config.example.json");
    let cfg = Config::load_from_str(text).expect("example parses");
    assert!(cfg.mapping_enabled.is_empty(), "{:?}", cfg.mapping_enabled);
    for k in ControllerKind::ALL {
        assert_eq!(cfg.output_mode.get(k.slug()), Some(&OutputMode::Kbm), "{k:?}");
    }
}
