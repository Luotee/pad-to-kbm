//! Engine event → Tauri emit bridge. Runs on its own thread for the lifetime
//! of the app. Drains the engine's event channel every ~8 ms (matches the
//! engine loop tick) and re-emits as Tauri events for the frontend.

use crate::engine::{EngineEvent, Handle};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri::tray::TrayIcon;

pub fn spawn<R: Runtime>(app: AppHandle<R>, engine: Handle) {
    std::thread::spawn(move || {
        loop {
            for ev in engine.drain_events() {
                let _ = match ev {
                    EngineEvent::ControllerConnected { name, transport, kind, warnings, connected, off_pads, battery } => {
                        // Spec §10: tray icon swaps to "connected" on plug-in.
                        // Also re-sent when `warnings`, the connected pads or a
                        // battery reading change (idempotent here). `battery`:
                        // `{ "<slug>": { "pct", "charging" } }`, connected pads
                        // that report one.
                        if let Some(tray) = app.try_state::<TrayIcon<R>>() {
                            let _ = crate::gui::tray::set_connected(&tray, true);
                        }
                        app.emit("controller-status", serde_json::json!({
                            "connected": true, "name": name, "transport": transport, "kind": kind,
                            "warnings": warnings, "connected_pads": connected, "off_pads": off_pads,
                            "battery": battery
                        }))
                    }
                    EngineEvent::ControllerDisconnected => {
                        if let Some(tray) = app.try_state::<TrayIcon<R>>() {
                            let _ = crate::gui::tray::set_connected(&tray, false);
                        }
                        app.emit("controller-status", serde_json::json!({
                            "connected": false, "name": "", "transport": "", "kind": null,
                            "warnings": [], "connected_pads": [], "off_pads": engine.off_pads(),
                            "battery": {}
                        }))
                    }
                    EngineEvent::ButtonDown { id } =>
                        app.emit("button-down", serde_json::json!({ "id": id })),
                    EngineEvent::ButtonUp { id } =>
                        app.emit("button-up", serde_json::json!({ "id": id })),
                    EngineEvent::KeyEmit { ts_ms, key, action, source } =>
                        app.emit("activity", serde_json::json!({
                            "ts_ms": ts_ms, "kind": "emit", "key": key, "action": action,
                            "source": source_json(&source)
                        })),
                    EngineEvent::MacroStart { ts_ms, name, button } =>
                        app.emit("activity", serde_json::json!({
                            "ts_ms": ts_ms, "kind": "macro-start", "name": name, "button": button
                        })),
                    EngineEvent::MacroEnd { ts_ms, name, completed } =>
                        app.emit("activity", serde_json::json!({
                            "ts_ms": ts_ms, "kind": "macro-end", "name": name, "completed": completed
                        })),
                    EngineEvent::TouchpadClick { raw_x, raw_y, quadrant } =>
                        app.emit("touchpad-click", serde_json::json!({
                            "raw_x": raw_x, "raw_y": raw_y, "quadrant": quadrant
                        })),
                    EngineEvent::TouchpadHover { raw_x, raw_y, quadrant } =>
                        app.emit("touchpad-hover", serde_json::json!({
                            "raw_x": raw_x, "raw_y": raw_y, "quadrant": quadrant
                        })),
                    // Not a settings change the user made (`ui`: no 設定已變更 row).
                    EngineEvent::ConfigChanged { source } =>
                        app.emit("config-changed", serde_json::json!({ "source": source, "ui": true })),
                };
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    });
}

/// The `activity` event's `source`: `{"button": id}` or `{"macro": name}`.
pub fn source_json(src: &crate::mapper::Source) -> serde_json::Value {
    match src {
        crate::mapper::Source::Button(id) => serde_json::json!({ "button": id }),
        crate::mapper::Source::Macro(name) => serde_json::json!({ "macro": name }),
    }
}
