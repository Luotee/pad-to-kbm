//! Remembered HidHide pads (v3.6.0, `hidhide_known`): the config field, the
//! hider's write path (`hider::remember_hidden_pads`) and the settings'
//! forget commands. Both write paths share the config file lock, so a
//! remembered pad and a concurrent IPC write never overwrite each other.

mod test_helpers;

use dualsense_mapper::config::{forget_known, remember_known, Config, KnownPad, HIDHIDE_KNOWN_MAX};
use dualsense_mapper::config_io::ConfigDoc;
use dualsense_mapper::engine::Engine;
use dualsense_mapper::gui::commands::{
    forget_all_hidden_pads_impl, forget_hidden_pad_impl, set_settings_impl, Settings,
};
use dualsense_mapper::hider::remember_hidden_pads;

const DS_USB: &str = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
const DS_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000";
const NSP_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\9&1&0&0000";

fn kp(kind: &str, id: &str) -> KnownPad {
    KnownPad { kind: kind.into(), id: id.into() }
}

fn ids(list: &[KnownPad]) -> Vec<String> {
    list.iter().map(|k| k.id.clone()).collect()
}

// ── the field ──

#[test]
fn missing_field_reads_as_empty_and_empty_is_not_written() {
    let cfg = test_helpers::load_example();
    assert!(cfg.hidhide_known.is_empty(), "an older config has no list");
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(!json.contains("hidhide_known"), "empty: not written");
    let example = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.json")).unwrap();
    assert!(!example.contains("hidhide_known"));
}

#[test]
fn field_round_trips() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![kp("dualsense", DS_BT), kp("ns_pro", NSP_BT)];
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let back = Config::load_from_str(&json).unwrap();
    assert_eq!(back.hidhide_known, cfg.hidhide_known);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["hidhide_known"][0], serde_json::json!({ "kind": "dualsense", "id": DS_BT }));
}

#[test]
fn validate_rejects_a_bad_list() {
    let base = test_helpers::load_example();
    let check = |list: Vec<KnownPad>, what: &str| {
        let mut cfg = base.clone();
        cfg.hidhide_known = list;
        let err = cfg.validate().expect_err(what);
        assert!(format!("{err:#}").contains("hidhide_known"), "{what}: {err:#}");
    };
    check(vec![kp("joycon", DS_BT)], "unknown kind");
    check(vec![kp("dualsense", "")], "empty id");
    check(vec![kp("dualsense", DS_USB), kp("dualsense", &DS_USB.to_lowercase())], "duplicate id (case-insensitive)");
    let many = (0..=HIDHIDE_KNOWN_MAX).map(|i| kp("dualsense", &format!(r"HID\VID_054C&PID_0CE6\7&{i}&0&0000"))).collect();
    check(many, "more than the cap");
    let mut ok = base.clone();
    ok.hidhide_known = (0..HIDHIDE_KNOWN_MAX).map(|i| kp("dualsense", &format!(r"HID\VID_054C&PID_0CE6\7&{i}&0&0000"))).collect();
    ok.validate().expect("the cap itself is fine");
}

// ── list helpers ──

#[test]
fn remember_appends_new_ids_case_insensitively_and_keeps_the_newest_sixteen() {
    assert_eq!(HIDHIDE_KNOWN_MAX, 16);
    let list = vec![kp("dualsense", DS_USB)];
    assert_eq!(remember_known(&list, &[kp("dualsense", &DS_USB.to_lowercase())]), list, "same id, other case");
    assert_eq!(remember_known(&list, &[kp("ns_pro", NSP_BT)]), vec![kp("dualsense", DS_USB), kp("ns_pro", NSP_BT)]);
    let full: Vec<KnownPad> = (0..16).map(|i| kp("dualsense", &format!("id{i}"))).collect();
    let next = remember_known(&full, &[kp("ns_pro", NSP_BT)]);
    assert_eq!(next.len(), 16);
    assert_eq!(next[0].id, "id1", "the oldest goes");
    assert_eq!(next[15].id, NSP_BT);
}

#[test]
fn forget_removes_one_id_case_insensitively_or_all() {
    let list = vec![kp("dualsense", DS_USB), kp("ns_pro", NSP_BT)];
    assert_eq!(forget_known(&list, Some(&DS_USB.to_lowercase())), vec![kp("ns_pro", NSP_BT)]);
    assert_eq!(forget_known(&list, Some("nope")), list);
    assert!(forget_known(&list, None).is_empty());
}

// ── write paths ──

#[test]
fn remember_writes_the_file_and_the_live_config_once() {
    let cfg = test_helpers::load_example();
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    assert!(remember_hidden_pads(&h, file.path(), &[kp("dualsense", DS_BT)]).unwrap());
    assert_eq!(ids(&h.config_read().hidhide_known), vec![DS_BT.to_string()]);
    assert_eq!(ids(&ConfigDoc::load(file.path()).unwrap().typed().hidhide_known), vec![DS_BT.to_string()]);
    assert!(!remember_hidden_pads(&h, file.path(), &[kp("dualsense", &DS_BT.to_uppercase())]).unwrap(), "known: no write");
    engine.shutdown();
}

#[test]
fn forget_commands_remove_entries_and_report_a_change() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![kp("dualsense", DS_USB), kp("dualsense", DS_BT), kp("ns_pro", NSP_BT)];
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    assert!(forget_hidden_pad_impl(&h, file.path(), &DS_BT.to_lowercase()).unwrap());
    assert_eq!(ids(&h.config_read().hidhide_known), vec![DS_USB.to_string(), NSP_BT.to_string()]);
    assert_eq!(ids(&ConfigDoc::load(file.path()).unwrap().typed().hidhide_known), vec![DS_USB.to_string(), NSP_BT.to_string()]);
    assert!(!forget_hidden_pad_impl(&h, file.path(), DS_BT).unwrap(), "already gone: no change");
    assert!(forget_all_hidden_pads_impl(&h, file.path()).unwrap());
    assert!(h.config_read().hidhide_known.is_empty());
    let raw = std::fs::read_to_string(file.path()).unwrap();
    assert!(!raw.contains("hidhide_known"), "empty list: the key goes");
    assert!(!forget_all_hidden_pads_impl(&h, file.path()).unwrap());
    engine.shutdown();
}

#[test]
fn remembering_never_overwrites_a_concurrent_ipc_write() {
    // The hider thread writes while the IPC writes: both read-modify-write
    // the file, so without the shared lock one change would be lost.
    let cfg = test_helpers::load_example();
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    let path = file.path().to_path_buf();
    const N: usize = 16;
    std::thread::scope(|s| {
        let (h1, p1) = (h.clone(), path.clone());
        s.spawn(move || {
            for i in 0..N {
                remember_hidden_pads(&h1, &p1, &[kp("dualsense", &format!(r"HID\VID_054C&PID_0CE6\7&{i}&0&0000"))]).unwrap();
            }
        });
        let (h2, p2) = (h.clone(), path.clone());
        s.spawn(move || {
            for i in 0..N {
                let deadzone = 0.1 + (i as f32) * 0.01;
                set_settings_impl(&h2, &p2, Settings { deadzone, ..Settings::defaults() }).unwrap();
            }
        });
    });
    let disk = ConfigDoc::load(&path).unwrap();
    assert_eq!(disk.typed().hidhide_known.len(), N, "every remembered pad survived the IPC writes");
    assert!((disk.typed().deadzone - (0.1 + (N as f32 - 1.0) * 0.01)).abs() < 1e-4, "the last IPC write survived");
    assert_eq!(h.config_read().hidhide_known.len(), N, "live config too");
    engine.shutdown();
}
