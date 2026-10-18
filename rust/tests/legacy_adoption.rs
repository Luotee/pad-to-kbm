//! v3.1.0 rename: the files beside the exe moved from `dualsense-mapper.*`
//! to `padtokbm.*`. The first start adopts the old files so no user loses
//! their bindings.

use dualsense_mapper::file_names::{
    adopt_all, adopt_legacy, default_config_path, Adoption, CONFIG_FILE, LEGACY_CONFIG_FILE,
    LEGACY_UI_PREFS_FILE, UI_PREFS_FILE,
};
use std::fs;

const OLD: &str = "{\"x\":1}";

#[test]
fn adopts_legacy_config() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    let got = adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE);
    assert!(matches!(got, Adoption::Adopted { .. }), "{got:?}");
    assert_eq!(fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(), OLD);
    assert_eq!(fs::read_to_string(dir.path().join("dualsense-mapper.json.bak")).unwrap(), OLD);
    assert!(!dir.path().join(LEGACY_CONFIG_FILE).exists());
}

#[test]
fn no_legacy_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE), Adoption::NoLegacy));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn both_present_touches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    fs::write(dir.path().join(CONFIG_FILE), "new").unwrap();
    assert!(matches!(adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE), Adoption::AlreadyNew { .. }));
    assert_eq!(fs::read_to_string(dir.path().join(LEGACY_CONFIG_FILE)).unwrap(), OLD);
    assert_eq!(fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(), "new");
    assert!(!dir.path().join("dualsense-mapper.json.bak").exists());
}

#[cfg(unix)]
#[test]
fn adopt_failure_keeps_legacy_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let (path, got) = default_config_path(dir.path());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(got, Adoption::Failed { .. }), "{got:?}");
    assert_eq!(path, dir.path().join(LEGACY_CONFIG_FILE));
    assert_eq!(fs::read_to_string(&path).unwrap(), OLD);
}

#[test]
fn default_path_after_adoption_is_new_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    let (path, got) = default_config_path(dir.path());
    assert!(matches!(got, Adoption::Adopted { .. }));
    assert_eq!(path, dir.path().join(CONFIG_FILE));
    let (path, got) = default_config_path(tempfile::tempdir().unwrap().path());
    assert!(matches!(got, Adoption::NoLegacy));
    assert!(path.ends_with(CONFIG_FILE));
}

#[test]
fn stale_bak_is_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    fs::write(dir.path().join("dualsense-mapper.json.bak"), "junk").unwrap();
    adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE);
    assert_eq!(fs::read_to_string(dir.path().join("dualsense-mapper.json.bak")).unwrap(), OLD);
}

/// v3.4.0: adoption moves the config only; the old ui-prefs file is left
/// for `gui::ui_prefs::absorb` (tests/ui_prefs.rs), which folds it in.
#[test]
fn adopt_all_moves_the_config_only() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    fs::write(dir.path().join(LEGACY_UI_PREFS_FILE), "{}").unwrap();
    let cfg = adopt_all(dir.path());
    assert_eq!(cfg, dir.path().join(CONFIG_FILE));
    assert_eq!(fs::read_to_string(dir.path().join(CONFIG_FILE)).unwrap(), OLD);
    assert!(!dir.path().join(LEGACY_CONFIG_FILE).exists());
    assert_eq!(fs::read_to_string(dir.path().join(LEGACY_UI_PREFS_FILE)).unwrap(), "{}");
    assert!(!dir.path().join(UI_PREFS_FILE).exists());
}

#[test]
fn already_new_reports_legacy_presence() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(CONFIG_FILE), "new").unwrap();
    assert_eq!(
        adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE),
        Adoption::AlreadyNew { legacy_present: false }
    );
    fs::write(dir.path().join(LEGACY_CONFIG_FILE), OLD).unwrap();
    assert_eq!(
        adopt_legacy(dir.path(), LEGACY_CONFIG_FILE, CONFIG_FILE),
        Adoption::AlreadyNew { legacy_present: true }
    );
}

#[test]
fn explicit_config_is_never_renamed() {
    // `--config <dir>/dualsense-mapper.json` is a path the user chose: only
    // default_config_path adopts; nothing else renames a config.
    let dir = tempfile::tempdir().unwrap();
    let chosen = dir.path().join(LEGACY_CONFIG_FILE);
    fs::write(&chosen, OLD).unwrap();
    assert_eq!(fs::read_to_string(&chosen).unwrap(), OLD);
    assert!(!dir.path().join(CONFIG_FILE).exists());
}
