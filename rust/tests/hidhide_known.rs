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
use dualsense_mapper::gui::file_watcher::{reload, Reload};
use dualsense_mapper::hider::remember_hidden_pads;

const DS_USB: &str = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
const DS_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000";
const NSP_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\9&1&0&0000";

fn kp(kind: &str, id: &str) -> KnownPad {
    KnownPad { kind: kind.into(), id: id.into(), addr: None }
}

/// A Bluetooth pad with its address (Task 10).
fn kpa(kind: &str, id: &str, addr: &str) -> KnownPad {
    KnownPad { kind: kind.into(), id: id.into(), addr: Some(addr.into()) }
}

const ADDR: &str = "7C66EF8223D2";

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
fn the_bluetooth_address_round_trips_and_an_entry_without_one_still_reads() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![kpa("dualsense", DS_BT, ADDR), kp("dualsense", DS_USB)];
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["hidhide_known"][0], serde_json::json!({ "kind": "dualsense", "id": DS_BT, "addr": ADDR }));
    assert_eq!(v["hidhide_known"][1], serde_json::json!({ "kind": "dualsense", "id": DS_USB }), "no address: not written");
    assert_eq!(Config::load_from_str(&json).unwrap().hidhide_known, cfg.hidhide_known);
    // A v3.6.0-dev entry ({kind, id}) reads with no address.
    let k: KnownPad = serde_json::from_value(serde_json::json!({ "kind": "dualsense", "id": DS_BT })).unwrap();
    assert_eq!(k, kp("dualsense", DS_BT));
}

#[test]
fn validate_rejects_a_bad_address() {
    let base = test_helpers::load_example();
    for bad in ["7c66ef8223d2", "7C:66:EF:82:23:D2", "7C66EF8223", ""] {
        let mut cfg = base.clone();
        cfg.hidhide_known = vec![kpa("dualsense", DS_BT, bad)];
        let err = cfg.validate().expect_err(bad);
        assert!(format!("{err:#}").contains("addr"), "{bad}: {err:#}");
    }
    let mut ok = base.clone();
    ok.hidhide_known = vec![kpa("dualsense", DS_BT, ADDR)];
    ok.validate().expect("12 upper-case hex digits");
}

#[test]
fn a_lower_case_address_loads_upper_case_and_a_bad_one_is_still_rejected() {
    let mut v = serde_json::to_value(test_helpers::load_example()).unwrap();
    v["hidhide_known"] = serde_json::json!([{ "kind": "dualsense", "id": DS_BT, "addr": "7c66ef8223d2" }]);
    let cfg = Config::load_from_str(&v.to_string()).expect("lower case is the same address");
    assert_eq!(cfg.hidhide_known[0].addr.as_deref(), Some(ADDR));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("padtokbm.json");
    std::fs::write(&path, v.to_string()).unwrap();
    assert_eq!(ConfigDoc::load(&path).unwrap().typed().hidhide_known[0].addr.as_deref(), Some(ADDR), "the IPC's load too");
    for bad in ["7c66ef8223", "7c66ef8223zz", "7C:66:EF:82:23:D2"] {
        v["hidhide_known"][0]["addr"] = serde_json::json!(bad);
        let err = Config::load_from_str(&v.to_string()).expect_err(bad);
        assert!(format!("{err:#}").contains("addr"), "{bad}: {err:#}");
    }
}

#[test]
fn remember_fills_a_missing_address_and_keeps_a_known_one() {
    let list = vec![kp("dualsense", DS_BT), kpa("ns_pro", NSP_BT, "AABBCCDDEEFF")];
    // Learned again with its address (the start's look-up): filled in place.
    let next = remember_known(&list, &[kpa("dualsense", &DS_BT.to_lowercase(), ADDR), kpa("ns_pro", NSP_BT, "112233445566")]);
    assert_eq!(next, vec![kpa("dualsense", DS_BT, ADDR), kpa("ns_pro", NSP_BT, "AABBCCDDEEFF")]);
    // Found without one: nothing changes.
    assert_eq!(remember_known(&next, &[kp("dualsense", DS_BT)]), next);
    // A new pad keeps the address it was learned with.
    assert_eq!(remember_known(&[], &[kpa("dualsense", DS_BT, ADDR)]), vec![kpa("dualsense", DS_BT, ADDR)]);
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
fn an_old_entry_gets_its_address_written_once() {
    let mut cfg = test_helpers::load_example();
    cfg.hidhide_known = vec![kp("dualsense", DS_BT), kp("dualsense", DS_USB)];
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    let found = [kpa("dualsense", DS_BT, ADDR)];
    assert!(remember_hidden_pads(&h, file.path(), &found).unwrap(), "address added: written");
    let want = vec![kpa("dualsense", DS_BT, ADDR), kp("dualsense", DS_USB)];
    assert_eq!(h.config_read().hidhide_known, want);
    assert_eq!(ConfigDoc::load(file.path()).unwrap().typed().hidhide_known, want);
    assert!(!remember_hidden_pads(&h, file.path(), &found).unwrap(), "known already: written once");
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

#[test]
fn watcher_reload_applies_external_edits_only() {
    let cfg = test_helpers::load_example();
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg.clone()).unwrap();
    let h = engine.handle();
    assert_eq!(reload(&h, file.path()), Reload::Same);
    let mut edited = cfg.clone();
    edited.deadzone = 0.33;
    std::fs::write(file.path(), serde_json::to_string_pretty(&edited).unwrap()).unwrap();
    assert_eq!(reload(&h, file.path()), Reload::Applied);
    assert!((h.config_read().deadzone - 0.33).abs() < 1e-6);
    std::fs::write(file.path(), "{ not json").unwrap();
    assert!(matches!(reload(&h, file.path()), Reload::Invalid(_)));
    engine.shutdown();
}

#[test]
fn a_watcher_reload_never_drops_a_remembered_pad_from_the_live_config() {
    // The watcher loads, compares and replaces the live config; a remember
    // landing between its load and its replace would be lost from the live
    // config without the shared file lock.
    let cfg = test_helpers::load_example();
    let file = test_helpers::tmp_config_with(&cfg);
    let engine = Engine::spawn_with_fake_gamepad(cfg).unwrap();
    let h = engine.handle();
    let path = file.path().to_path_buf();
    let done = std::sync::atomic::AtomicBool::new(false);
    let lost = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = reload(&h, &path);
            }
        });
        s.spawn(|| {
            for i in 0..16 {
                let id = format!(r"HID\VID_054C&PID_0CE6\7&{i}&0&0000");
                remember_hidden_pads(&h, &path, &[kp("dualsense", &id)]).unwrap();
                for _ in 0..20 {
                    if !h.config_read().hidhide_known.iter().any(|k| k.id == id) {
                        lost.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    std::thread::yield_now();
                }
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    });
    assert_eq!(lost.load(std::sync::atomic::Ordering::SeqCst), 0, "a reload replaced the live config with a stale file");
    engine.shutdown();
}
