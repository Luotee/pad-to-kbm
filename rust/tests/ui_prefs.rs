//! v3.4.0: one file beside the exe. The board's last pad lives in
//! padtokbm.json (`last_pad`); an old ui-prefs file (padtokbm.ui.json, or the
//! pre-v3.1.0 dualsense-mapper.ui.json and its .bak) hands its last pad over
//! once at startup and is deleted.
//! v3.4.1: that start also turns `log_events` off, which every config written
//! before v3.4.0 had on by default.

use dualsense_mapper::config::Config;
use dualsense_mapper::engine::Engine;
use dualsense_mapper::file_names::{CONFIG_FILE, LEGACY_UI_PREFS_FILE, UI_PREFS_FILE};
use dualsense_mapper::gui::commands::{get_ui_prefs_impl, set_ui_prefs_impl};
use dualsense_mapper::gui::ui_prefs::{absorb, Absorb, UiPrefs};
use std::fs;

const EXAMPLE: &str = include_str!("../config.example.json");
const LEGACY_UI_BAK: &str = "dualsense-mapper.ui.json.bak";

/// A config as v3.3.0 and earlier wrote it on the first start: logging on.
fn old_example() -> String {
    let mut raw: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    raw["log_events"] = true.into();
    serde_json::to_string_pretty(&raw).unwrap()
}

fn dir_with_config(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join(CONFIG_FILE);
    fs::write(&cfg, text).unwrap();
    (dir, cfg)
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn last_pad_is_an_optional_config_field() {
    let cfg = Config::load_from_str(EXAMPLE).unwrap();
    assert_eq!(cfg.last_pad, None);
    assert!(!serde_json::to_string(&cfg).unwrap().contains("last_pad"), "not written while unset");
    let mut raw: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    raw["last_pad"] = "ns2_pro".into();
    let cfg = Config::load_from_str(&raw.to_string()).unwrap();
    assert_eq!(cfg.last_pad.as_deref(), Some("ns2_pro"));
}

#[test]
fn old_ui_file_hands_its_pad_to_the_config_and_goes() {
    let (dir, cfg) = dir_with_config(EXAMPLE);
    fs::write(dir.path().join(UI_PREFS_FILE), r#"{"drawer_open":false,"last_tab":null,"last_pad":"ns_pro"}"#).unwrap();
    assert_eq!(absorb(&cfg), Absorb::Done { last_pad: Some("ns_pro".into()), log_off: false });
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
    let after = Config::load_from_path(&cfg).unwrap();
    assert_eq!(after.last_pad.as_deref(), Some("ns_pro"));
    // Only last_pad changed; the user's _help / _keyboard_keys notes stay.
    let mut want = Config::load_from_str(EXAMPLE).unwrap();
    want.last_pad = Some("ns_pro".into());
    assert_eq!(after, want);
    let raw: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(raw.get("_help").is_some() && raw.get("_keyboard_keys").is_some());
}

#[test]
fn pre_rename_ui_files_go_too() {
    let (dir, cfg) = dir_with_config(EXAMPLE);
    fs::write(dir.path().join(LEGACY_UI_PREFS_FILE), r#"{"last_pad":"ns2_pro"}"#).unwrap();
    fs::write(dir.path().join(LEGACY_UI_BAK), r#"{"last_pad":"dualsense"}"#).unwrap();
    assert_eq!(absorb(&cfg), Absorb::Done { last_pad: Some("ns2_pro".into()), log_off: false });
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
    assert_eq!(Config::load_from_path(&cfg).unwrap().last_pad.as_deref(), Some("ns2_pro"));
}

#[test]
fn a_pad_already_in_the_config_wins() {
    let mut raw: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    raw["last_pad"] = "dualsense".into();
    let text = serde_json::to_string_pretty(&raw).unwrap();
    let (dir, cfg) = dir_with_config(&text);
    fs::write(dir.path().join(UI_PREFS_FILE), r#"{"last_pad":"ns_pro"}"#).unwrap();
    assert_eq!(absorb(&cfg), Absorb::Done { last_pad: None, log_off: false });
    assert_eq!(fs::read_to_string(&cfg).unwrap(), text, "config untouched");
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
}

#[test]
fn unreadable_ui_file_is_just_deleted() {
    let (dir, cfg) = dir_with_config(EXAMPLE);
    fs::write(dir.path().join(UI_PREFS_FILE), "not json").unwrap();
    assert_eq!(absorb(&cfg), Absorb::Done { last_pad: None, log_off: false });
    assert_eq!(fs::read_to_string(&cfg).unwrap(), EXAMPLE, "config untouched");
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
}

#[test]
fn a_broken_config_keeps_the_ui_file_for_the_next_start() {
    let (dir, cfg) = dir_with_config("{ broken");
    fs::write(dir.path().join(UI_PREFS_FILE), r#"{"last_pad":"ns_pro"}"#).unwrap();
    assert!(matches!(absorb(&cfg), Absorb::Kept { .. }));
    assert_eq!(fs::read_to_string(&cfg).unwrap(), "{ broken");
    assert!(dir.path().join(UI_PREFS_FILE).exists());
}

#[test]
fn nothing_to_do_without_old_files() {
    let (dir, cfg) = dir_with_config(EXAMPLE);
    assert_eq!(absorb(&cfg), Absorb::Nothing);
    assert_eq!(fs::read_to_string(&cfg).unwrap(), EXAMPLE);
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
}

#[test]
fn an_old_config_stops_logging_when_its_ui_file_is_absorbed() {
    let (dir, cfg) = dir_with_config(&old_example());
    fs::write(dir.path().join(UI_PREFS_FILE), r#"{"last_pad":"ns_pro"}"#).unwrap();
    assert_eq!(absorb(&cfg), Absorb::Done { last_pad: Some("ns_pro".into()), log_off: true });
    let mut want = Config::load_from_str(EXAMPLE).unwrap();
    want.last_pad = Some("ns_pro".into());
    assert_eq!(Config::load_from_path(&cfg).unwrap(), want, "log off, pad in, nothing else");
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()]);
}

#[test]
fn logging_goes_off_even_when_the_pad_is_not_moved() {
    for (ui, cfg_pad) in [(r#"{"last_pad":"ns_pro"}"#, Some("dualsense")), ("not json", None)] {
        let mut raw: serde_json::Value = serde_json::from_str(&old_example()).unwrap();
        if let Some(p) = cfg_pad {
            raw["last_pad"] = p.into();
        }
        let (dir, cfg) = dir_with_config(&raw.to_string());
        fs::write(dir.path().join(UI_PREFS_FILE), ui).unwrap();
        assert_eq!(absorb(&cfg), Absorb::Done { last_pad: None, log_off: true }, "{ui}");
        let after = Config::load_from_path(&cfg).unwrap();
        assert!(!after.log_events);
        assert_eq!(after.last_pad.as_deref(), cfg_pad, "the config's pad stays");
    }
}

#[test]
fn logging_chosen_after_the_move_is_kept() {
    // No old file: a v3.4.0+ config whose user turned logging on.
    let (_dir, cfg) = dir_with_config(&old_example());
    assert_eq!(absorb(&cfg), Absorb::Nothing);
    assert!(Config::load_from_path(&cfg).unwrap().log_events);
}

#[test]
fn set_ui_prefs_writes_last_pad_into_the_config_only() {
    let (dir, cfg_path) = dir_with_config(EXAMPLE);
    let engine = Engine::spawn_with_fake_gamepad(Config::load_from_str(EXAMPLE).unwrap()).unwrap();
    let h = engine.handle();
    assert_eq!(get_ui_prefs_impl(&h).last_pad, None);
    let changed = set_ui_prefs_impl(&h, &cfg_path, UiPrefs { last_pad: Some("ns2_pro".into()) }).unwrap();
    assert!(changed);
    assert_eq!(get_ui_prefs_impl(&h).last_pad.as_deref(), Some("ns2_pro"));
    assert_eq!(Config::load_from_path(&cfg_path).unwrap().last_pad.as_deref(), Some("ns2_pro"));
    assert_eq!(names(dir.path()), vec![CONFIG_FILE.to_string()], "no second file");
    // The same pad again writes nothing.
    let before = fs::read_to_string(&cfg_path).unwrap();
    assert!(!set_ui_prefs_impl(&h, &cfg_path, UiPrefs { last_pad: Some("ns2_pro".into()) }).unwrap());
    assert_eq!(fs::read_to_string(&cfg_path).unwrap(), before);
}
