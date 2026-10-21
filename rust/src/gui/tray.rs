//! System tray: green/grey icon + Open / Pause mapper / Quit menu.
//!
//! Tray is the always-on connection-state indicator and gives quick
//! access to Pause / Quit while the window is open. Closing the
//! window (✕) fully exits the process in v1.0.3+, so the tray's
//! `Quit` entry is a convenience duplicate of the X button rather
//! than the only exit path it was in v1.0.0..v1.0.2.

use crate::engine::Handle;
use anyhow::Result;
use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Runtime,
};

// Embed the ICO files at compile time so the binary is fully self-contained
// and `Image::from_path` CWD issues on double-clicked Windows exes are avoided.
static ICON_DISCONNECTED: &[u8] =
    include_bytes!("../../icons/tray-disconnected.ico");
static ICON_CONNECTED: &[u8] =
    include_bytes!("../../icons/tray-connected.ico");

pub fn build<R: Runtime>(app: &AppHandle<R>, engine: Handle) -> Result<TrayIcon<R>> {
    let open  = MenuItem::with_id(app, "open",  "Open",         true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause mapper", true, None::<&str>)?;
    let sep   = PredefinedMenuItem::separator(app)?;
    let quit  = MenuItem::with_id(app, "quit",  "Quit",         true, None::<&str>)?;
    let menu  = Menu::with_items(app, &[&open, &pause, &sep, &quit])?;

    let icon = Image::from_bytes(ICON_DISCONNECTED)?;

    let engine_for_menu = engine.clone();
    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event: MenuEvent| match event.id().as_ref() {
            "open" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "pause" => {
                let new = !engine_for_menu.is_paused();
                engine_for_menu.set_paused(new);
            }
            "quit" => {
                // Spec §10: shutdown path identical to Ctrl-C.
                // `app.exit(0)` ends the Tauri event loop; `run_return`
                // returns in `runtime::run`, which calls `engine.shutdown()`
                // so all held keys are released (Iron rule #3), then exits.
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(w) = tray.app_handle().get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(tray)
}

/// Swap the tray icon between the connected (green) and disconnected (grey)
/// states. Called from Task 25 when the gamepad connect/disconnect event fires.
pub fn set_connected<R: Runtime>(tray: &TrayIcon<R>, connected: bool) -> Result<()> {
    let bytes = if connected { ICON_CONNECTED } else { ICON_DISCONNECTED };
    tray.set_icon(Some(Image::from_bytes(bytes)?))?;
    Ok(())
}
