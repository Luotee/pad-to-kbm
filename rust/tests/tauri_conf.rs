//! The board's drag-and-drop (tile → slot, slot ↔ slot, drag-out clear,
//! macro step order) is HTML5 DnD. On Windows, Tauri's own drag-drop handler
//! (`dragDropEnabled`, default true) swallows it in WebView2, and web-smoke
//! cannot see that (it runs in plain Chromium). Pin the setting here.

#[test]
fn main_window_leaves_drag_and_drop_to_the_page() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let windows = conf["app"]["windows"].as_array().expect("app.windows");
    assert!(!windows.is_empty());
    for w in windows {
        assert_eq!(w["dragDropEnabled"], serde_json::Value::Bool(false), "window {}: dragDropEnabled must be false", w["label"]);
    }
}

/// v3.3.0: the page draws the MapleStory window itself (frame with cut corners, title bar), so the
/// OS draws no title bar, no frame and no shadow, and the window is see-through outside the
/// drawn corners. The size is the drawn window at 1x; Rust snaps it to whole scales at runtime.
#[test]
fn main_window_is_drawn_by_the_page() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let w = &conf["app"]["windows"][0];
    assert_eq!(w["decorations"], serde_json::Value::Bool(false));
    assert_eq!(w["transparent"], serde_json::Value::Bool(true));
    assert_eq!(w["shadow"], serde_json::Value::Bool(false));
    assert_eq!((w["width"].as_u64(), w["height"].as_u64()), (Some(994), Some(669)));
    assert!(w.get("minWidth").is_none() && w.get("minHeight").is_none(), "min size is the 1x snap, done in Rust");
}

/// The drawn title bar drags and minimises the window: the capability must allow both.
#[test]
fn capability_allows_the_drawn_title_bar() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json");
    let cap: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let perms: Vec<&str> = cap["permissions"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    for want in ["core:window:allow-start-dragging", "core:window:allow-minimize", "core:window:allow-close"] {
        assert!(perms.contains(&want), "missing {want}");
    }
    // Least privilege: Rust sizes the window itself; the page never needs set-size.
    assert!(!perms.contains(&"core:window:allow-set-size"), "the page does not resize the window");
}

/// v3.4.0: the window is a key-binding board, not a browser. No DevTools in a release build
/// (F12 / Ctrl+Shift+I): Tauri's `devtools` feature stays off (debug builds keep DevTools), and
/// the page gets no permission to toggle them.
#[test]
fn release_build_has_no_devtools() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: toml_lite::Doc = toml_lite::parse(&std::fs::read_to_string(dir.join("Cargo.toml")).unwrap());
    let features = manifest.tauri_features();
    assert!(!features.iter().any(|f| f == "devtools"), "tauri features {features:?} must not include devtools");
    let cap = std::fs::read_to_string(dir.join("capabilities/default.json")).unwrap();
    assert!(!cap.contains("devtools"), "capability must not grant a devtools permission");
}

/// v3.4.0: WebView2's browser keys (F3 / Ctrl+F find, F5 / Ctrl+R reload, Ctrl+P print, zoom, …)
/// are off. Tauri has no setting for it (wry's default is on), so `gui/runtime.rs` turns it off
/// through the WebView2 settings; web-smoke cannot see WebView2, so pin the call here.
#[test]
fn webview2_browser_keys_are_turned_off() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/gui/runtime.rs");
    let src = std::fs::read_to_string(p).unwrap();
    assert!(src.contains("SetAreBrowserAcceleratorKeysEnabled(false)"), "runtime.rs must turn WebView2 browser keys off");
}

/// Just enough of Cargo.toml to read the `tauri` dependency's feature list.
mod toml_lite {
    pub struct Doc(String);
    pub fn parse(s: &str) -> Doc {
        Doc(s.to_string())
    }
    impl Doc {
        pub fn tauri_features(&self) -> Vec<String> {
            let line = self.0.lines().find(|l| l.trim_start().starts_with("tauri = ")).expect("tauri dependency line");
            let list = line.split("features = [").nth(1).and_then(|r| r.split(']').next()).unwrap_or("");
            list.split(',').map(|f| f.trim().trim_matches('"').to_string()).filter(|f| !f.is_empty()).collect()
        }
    }
}
