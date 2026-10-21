//! Tauri runtime entry: window builder + lifecycle. v0.2.0 default mode.
//!
//! `run` is the top-level entry point for the GUI binary path.
//! It starts the engine, builds the Tauri window, then blocks on the
//! event loop. When the event loop ends (✕, tray Quit or the `quit` IPC),
//! `engine.shutdown()` fires, releasing all held keys (Iron rule #3), and
//! the process exits with the event loop's exit code.

use crate::config::Config;
use crate::config_io::ConfigDoc;
use crate::engine::Engine;
use crate::safety;
use anyhow::{Context, Result};
use crossbeam_channel::unbounded;
use std::path::PathBuf;
use tauri::{Emitter, Manager};

/// Options forwarded from `main.rs` into the GUI runtime.
pub struct RunOptions {
    pub config_path: PathBuf,
    pub dry_run: bool,
}

/// Start the engine and enter the Tauri event loop.
///
/// This function is **blocking**. It returns only if the app cannot be
/// built; once the event loop has run, it ends the process itself. The loop
/// runs through `App::run_return`, not `App::run`: on Windows `App::run`
/// (tao `EventLoop::run`) calls `std::process::exit` inside and never
/// returns. After the loop, `engine.shutdown()` runs so that
/// `KeyboardSink::drop` and `release_all_held` run (Iron rule #3), then the
/// process exits with the loop's exit code.
pub fn run(cfg: Config, opts: RunOptions) -> Result<()> {
    let engine = Engine::spawn(cfg, opts.dry_run)?;
    // The `log_events` file sits beside the config.
    if let Some(dir) = opts.config_path.parent() {
        engine.handle().set_log_dir(dir.to_path_buf());
    }
    // Bind the engine's key state to the global so the panic hook (installed
    // in main::real_main) can drain it on panic. Iron Rule #3, GUI path.
    safety::register_global(engine.handle().key_state());
    // Same for the virtual Xbox pad: the hook unplugs it.
    crate::xinput_sink::register_global(engine.handle().virtual_pad());
    let handle = engine.handle();

    // Clone handle before the `move` setup closure so `.manage(handle_for_state)`
    // can still be called on the builder after the closure has captured its copy.
    let handle_for_setup = handle.clone();
    let handle_for_state = handle.clone();
    let config_path_for_setup = opts.config_path.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A second instance was launched — focus the existing window instead.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(move |app| {
            // The "main" window is declared in tauri.conf.json's `app.windows[]`
            // array; Tauri builds it automatically before setup runs. Trying to
            // also build it here would collide ("a webview with label `main`
            // already exists" — exactly the panic that bit v0.2.0-rc).
            //
            // We just wire the tray + event bridge + file watcher here.

            // The page draws the window as pixel art (v3.3.0): open it at the whole scale nearest the
            // display scale that fits the screen (gui/winscale.rs), centred.
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(Some(m)) = w.current_monitor() {
                    let area = m.work_area().size;
                    let n = crate::gui::winscale::pick_scale(m.scale_factor(), area.width, area.height);
                    let (tw, th) = crate::gui::winscale::size_for(n);
                    let _ = w.set_size(tauri::PhysicalSize::new(tw, th));
                    let _ = w.center();
                }
                // A key-binding board, not a browser: no find bar, reload, print or zoom keys.
                #[cfg(windows)]
                let _ = w.with_webview(|wv| {
                    if let Err(e) = browser_keys_off(&wv) {
                        tracing::warn!("could not turn the WebView2 browser keys off: {e}");
                    }
                });
            }

            let tray = crate::gui::tray::build(&app.handle(), handle_for_setup.clone())?;
            // Store the tray so it persists for the app lifetime and can be
            // retrieved in Task 25 via `app.state::<TrayIcon<_>>()`.
            app.manage(tray);

            // Spawn the engine→Tauri event bridge. Runs for the lifetime of
            // the process; the thread dies when the process exits (Iron rule §11).
            crate::gui::events::spawn(app.handle().clone(), handle_for_setup.clone());

            // Spec §11.3: watch the config file for external edits (user
            // opens it in Notepad while the GUI is running). On a debounced
            // change event reload via ConfigDoc, hot-rebind the live engine,
            // emit `config-changed` to the frontend; on validation failure
            // emit `validation-error` so the frontend can show a banner.
            let (notify_tx, notify_rx) = unbounded::<()>();
            let watcher = crate::gui::file_watcher::spawn(
                config_path_for_setup.clone(),
                notify_tx,
            )?;
            app.manage(watcher);

            let app_for_watcher = app.handle().clone();
            let handle_for_watcher = handle_for_setup.clone();
            let path_for_watcher = config_path_for_setup.clone();
            std::thread::spawn(move || {
                for _ in notify_rx.iter() {
                    match ConfigDoc::load(&path_for_watcher) {
                        // Our own IPC writes come back here too; the engine
                        // already has them and the IPC command emitted
                        // config-changed, so only a real external edit goes on.
                        Ok(doc) if *handle_for_watcher.config_read() == *doc.typed() => {}
                        Ok(doc) => {
                            *handle_for_watcher.config_write() = doc.typed().clone();
                            let _ = app_for_watcher.emit(
                                "config-changed",
                                serde_json::json!({ "source": "file" }),
                            );
                        }
                        Err(e) => {
                            let _ = app_for_watcher.emit(
                                "validation-error",
                                serde_json::json!({ "message": format!("{e:#}") }),
                            );
                        }
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::gui::commands::get_config,
            crate::gui::commands::set_binding,
            crate::gui::commands::swap_bindings,
            crate::gui::commands::set_trigger_mode,
            crate::gui::commands::clear_pad,
            crate::gui::commands::reset_pad,
            crate::gui::commands::set_pad_tables,
            crate::gui::commands::set_macro,
            crate::gui::commands::delete_macro,
            crate::gui::commands::rename_macro,
            crate::gui::commands::set_settings,
            crate::gui::commands::reset_settings,
            crate::gui::commands::set_mapping_enabled,
            crate::gui::commands::set_pad_off,
            crate::gui::commands::get_vigem_status,
            crate::gui::commands::install_vigem,
            crate::gui::commands::pause_mapper,
            crate::gui::commands::open_config_in_editor,
            crate::gui::commands::open_config_folder,
            crate::gui::commands::set_rumble_on_connect,
            crate::gui::commands::set_lightbar,
            crate::gui::commands::get_config_path,
            crate::gui::commands::save_stick_cal,
            crate::gui::commands::reset_stick_cal,
            crate::gui::commands::get_stick_positions,
            crate::gui::commands::quit,
            crate::gui::commands::set_capture_active,
            crate::gui::commands::disconnect_gamepad,
            crate::gui::commands::get_controller_status,
            crate::gui::commands::get_ui_prefs,
            crate::gui::commands::set_ui_prefs,
            crate::gui::commands::get_app_version,
            crate::gui::commands::open_url,
        ])
        .manage(handle_for_state)
        .manage(opts.config_path.clone())
        .on_window_event(|window, event| {
            // Clicking ✕ now fully exits the process (Windows convention).
            // `app.exit(0)` ends the Tauri event loop; `run_return` hands
            // back to `runtime::run`, which calls `engine.shutdown()` so held
            // keys release before the process exits (Iron rule #3). Users who want
            // background mapping while the window is hidden should
            // minimise to the taskbar instead, or use the tray's Open
            // entry to re-show the window after this exits.
            match event {
                tauri::WindowEvent::CloseRequested { .. } => window.app_handle().exit(0),
                // Keep the drawn window at whole scales (gui/winscale.rs): a resize snaps to the
                // nearest one that fits, a move to a monitor with another display scale re-picks.
                tauri::WindowEvent::Resized(size) => snap_window(window, *size),
                tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    if let Some((ww, wh)) = work_area(window) {
                        let (tw, th) = crate::gui::winscale::size_for(crate::gui::winscale::pick_scale(*scale_factor, ww, wh));
                        let _ = window.set_size(tauri::PhysicalSize::new(tw, th));
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            engine.shutdown();
            return Err(e).context("building the Tauri app");
        }
    };

    // `App::run` would never return here (see above): run_return does.
    let code = app.run_return(|_, _| {});

    // Iron rule #3: shut the engine down before the process exits, so held
    // keys and mouse buttons are released (and the lightbar exit frame written).
    engine.shutdown();
    std::process::exit(code);
}

fn work_area(w: &tauri::Window) -> Option<(u32, u32)> {
    let m = w.current_monitor().ok().flatten()?;
    let s = m.work_area().size;
    Some((s.width, s.height))
}

/// Snap a resized window to the nearest whole scale. Minimised (0 x 0) and maximised windows are
/// left alone (the page centres the drawing in a maximised one); a size already on a whole scale
/// is a no-op, which also ends the Resized event our own set_size causes.
fn snap_window(w: &tauri::Window, size: tauri::PhysicalSize<u32>) {
    if size.width == 0 || size.height == 0 || w.is_maximized().unwrap_or(false) || w.is_minimized().unwrap_or(false) {
        return;
    }
    let Some((ww, wh)) = work_area(w) else { return };
    let (tw, th) = crate::gui::winscale::size_for(crate::gui::winscale::snap_scale(size.width, size.height, ww, wh));
    if (tw, th) != (size.width, size.height) {
        let _ = w.set_size(tauri::PhysicalSize::new(tw, th));
    }
}

/// Turn WebView2's browser accelerator keys off (F3 / Ctrl+F, F5 / Ctrl+R,
/// Ctrl+P, zoom, …; v3.4.0). Tauri has no setting for it and wry leaves it on.
#[cfg(windows)]
fn browser_keys_off(wv: &tauri::webview::PlatformWebview) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;
    unsafe {
        let settings = wv.controller().CoreWebView2()?.Settings()?;
        settings.cast::<ICoreWebView2Settings3>()?.SetAreBrowserAcceleratorKeysEnabled(false)
    }
}
