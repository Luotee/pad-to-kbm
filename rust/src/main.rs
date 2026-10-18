// In release builds on Windows, link as a GUI subsystem app so double-click
// doesn't open a console window. Debug builds keep the console subsystem
// for `cargo run` ergonomics. CLI subcommands (`--cli`, `--validate`,
// `--list-buttons`) re-attach the parent console at runtime so prints
// still appear when launched from cmd.exe — see `attach_parent_console`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use clap::{ArgAction, Parser};
use crossbeam_channel;
use dualsense_mapper::app;
use dualsense_mapper::config::Config;
use dualsense_mapper::gamepad::{GamepadEvent, GamepadSource, HubCmd};
use dualsense_mapper::safety;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "PadToKBM", version, about)]
struct Cli {
    /// Path to config.json. Defaults to <exe_dir>/padtokbm.json.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Validate the config and exit.
    #[arg(long, action = ArgAction::SetTrue)]
    validate: bool,

    /// Print every synthesized action instead of pressing real keys.
    #[arg(long, action = ArgAction::SetTrue)]
    dry_run: bool,

    /// Connect to the first gamepad and print events with config labels. Exits on Ctrl-C.
    #[arg(long, action = ArgAction::SetTrue)]
    list_buttons: bool,

    /// Verbose logging.
    #[arg(long, action = ArgAction::SetTrue)]
    verbose: bool,

    /// Do not pause for "Press Enter to close" on error (CLI scripts / CI).
    #[arg(long, action = ArgAction::SetTrue)]
    no_pause: bool,

    /// Run in legacy console mode (v0.1.x behaviour). Default is GUI.
    #[arg(long, action = ArgAction::SetTrue)]
    cli: bool,
}

fn main() {
    // Always try to attach to the parent process's console. From cmd.exe
    // this gives us working stdout/stderr; from a double-click there is
    // no parent console and the call fails silently. Either way the GUI
    // subsystem release build never spawns a new black window.
    attach_parent_console();

    // We parse once up-front to know whether to pause on error. If parsing
    // itself fails, clap prints its own message and exits cleanly — pause
    // afterward so a double-click user can read what was wrong.
    let cli_parse = Cli::try_parse();
    let no_pause = match &cli_parse {
        Ok(c) => c.no_pause,
        Err(_) => false,
    };
    let cli = match cli_parse {
        Ok(c) => c,
        Err(e) => {
            // clap renders --help / --version through this same path.
            // Pause only on actual error kinds.
            let kind = e.kind();
            let _ = e.print();
            if matches!(
                kind,
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                std::process::exit(0);
            }
            if !no_pause { pause_on_error(); }
            std::process::exit(2);
        }
    };

    if let Err(e) = real_main(&cli) {
        let msg = format!("{e:#}");
        eprintln!();
        eprintln!("Error: {msg}");
        eprintln!();
        // GUI path on Windows: if the error happened before a window
        // existed (config parse, first-run write, etc.), stderr goes
        // nowhere on a double-click. Pop a MessageBox so the user sees
        // *something*. CLI mode still uses the stdin pause.
        if !cli.cli && !cli.validate && !cli.list_buttons {
            show_fatal_dialog(&msg);
        } else if !cli.no_pause {
            pause_on_error();
        }
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: AttachConsole is safe to call from any thread; if the
    // process has no parent console it returns 0 and we ignore that.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[cfg(windows)]
fn show_fatal_dialog(msg: &str) {
    use std::iter::once;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let body: Vec<u16> = msg.encode_utf16().chain(once(0)).collect();
    let title: Vec<u16> = "PadToKBM — Error"
        .encode_utf16()
        .chain(once(0))
        .collect();
    // SAFETY: pointers point into Vec<u16> that lives for the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR | MB_OK,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_dialog(_msg: &str) {}

fn real_main(cli: &Cli) -> Result<()> {
    let filter = if cli.verbose {
        EnvFilter::new("dualsense_mapper=debug,info")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Iron rule #3: panic on any thread must release held keys at the OS level.
    // Installed here — before CLI or GUI dispatch — so both paths are covered
    // from a single site. The Drop on KeyboardSink covers the normal shutdown
    // path; this hook covers the abnormal (panic) path.
    let panic_default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Err(e) = safety::emergency_release_all() {
            eprintln!("emergency key release failed: {e:#}");
        }
        // No virtual Xbox pad may outlive a panic with a frozen state.
        dualsense_mapper::xinput_sink::emergency_unplug();
        panic_default(info);
    }));

    let cfg_path = resolve_config_path(cli.config.as_deref())?;

    if cli.list_buttons {
        // For list-buttons, missing config is OK — show "<no label>".
        let cfg = Config::load_from_path(&cfg_path).ok();
        return list_buttons(cfg);
    }

    // Every other path needs a valid config; if it doesn't exist, write the
    // bundled default beside the exe. Unlike v0.1.0, we keep running with
    // that default — a double-clicked exe should "just work" out of the box.
    let just_wrote_default = if !cfg_path.exists() {
        first_run_copy(&cfg_path)?;
        true
    } else {
        false
    };
    // One file beside the exe (v3.4.0): an old ui-prefs file hands its last
    // pad to the config, then goes. Not on --validate, which only reads.
    if !cli.validate {
        dualsense_mapper::gui::ui_prefs::absorb(&cfg_path);
    }
    let cfg = Config::load_from_path(&cfg_path)
        .with_context(|| format!("loading config at {}", cfg_path.display()))?;

    if cli.validate {
        println!("OK: config at {} is valid.", cfg_path.display());
        return Ok(());
    }

    // --cli: legacy console mode (v0.1.x behaviour). Opt-in from v0.2.0.
    if cli.cli {
        print_banner(&cfg_path, cli.dry_run, just_wrote_default);
        let shutdown = Arc::new(AtomicBool::new(false));
        {
            let s = shutdown.clone();
            ctrlc::set_handler(move || s.store(true, Ordering::SeqCst))
                .context("installing Ctrl-C handler")?;
        }
        return app::run(cfg, app::RunOptions {
            dry_run: cli.dry_run,
            log_dir: cfg_path.parent().map(|d| d.to_path_buf()),
            shutdown,
        });
    }

    // Default: GUI mode (v0.2.0+).
    #[cfg(feature = "gui")]
    {
        return dualsense_mapper::gui::run(
            cfg,
            dualsense_mapper::gui::RunOptions {
                config_path: cfg_path,
                dry_run: cli.dry_run,
            },
        );
    }

    // Reached only when compiled without --features gui.
    #[cfg(not(feature = "gui"))]
    {
        anyhow::bail!(
            "This binary was built without GUI support. Re-run with --cli, \
             or rebuild with `cargo build --features gui`."
        );
    }
}

fn print_banner(cfg_path: &std::path::Path, dry_run: bool, just_wrote_default: bool) {
    println!("=================================================");
    println!(" PadToKBM v{}", env!("CARGO_PKG_VERSION"));
    println!(" Config: {}", cfg_path.display());
    if dry_run {
        println!(" Mode:   DRY-RUN (no real keystrokes sent)");
    }
    println!("-------------------------------------------------");
    if just_wrote_default {
        println!(" Wrote default config — open the file above in");
        println!(" Notepad to customize. The file has an inline");
        println!(" keyboard cheat sheet at the top.");
        println!(" Restart this program after you save changes.");
        println!("-------------------------------------------------");
    }
    println!(" Plug in DualSense via USB. Press buttons; the");
    println!(" mapped keys are sent to the focused window.");
    println!(" Press Ctrl-C or close this window to quit.");
    println!("=================================================");
    println!();
}

fn pause_on_error() {
    eprintln!("Press Enter to close this window.");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

fn resolve_config_path(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        // The chosen config is never renamed.
        return Ok(p.to_path_buf());
    }
    // Portable layout: config sits next to the executable as padtokbm.json.
    // Drop the folder, move it elsewhere, the config follows. A pre-v3.1.0
    // dualsense-mapper.json there is adopted (renamed, .bak kept).
    let exe = std::env::current_exe().context("could not determine executable path")?;
    let dir = exe.parent().context("executable has no parent directory")?;
    Ok(dualsense_mapper::file_names::adopt_all(dir))
}

fn first_run_copy(target: &std::path::Path) -> Result<()> {
    let example_text = include_str!("../config.example.json");
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating config dir {}", dir.display()))?;
    }
    std::fs::write(target, example_text)
        .with_context(|| format!("writing default config to {}", target.display()))?;
    // Don't bail — the bundled default is valid, so we continue with it.
    // The startup banner will tell the user the file was created.
    Ok(())
}

fn list_buttons(cfg: Option<Config>) -> Result<()> {
    let label_for = |id: u32| -> String {
        cfg.as_ref()
            .and_then(|c| c.pads.get("dualsense"))
            .and_then(|p| p.buttons.get(&id.to_string()))
            .map(|e| e.label.clone())
            .unwrap_or_else(|| "<no label — id not in config>".into())
    };

    let (_dummy_disconnect_tx, dummy_disconnect_rx) = crossbeam_channel::bounded::<HubCmd>(4);
    let (warn_tx, _warn_rx) = crossbeam_channel::bounded::<String>(8);
    let mut source = GamepadSource::new(
        dualsense_mapper::gamepad::CursorParams::default(),
        dummy_disconnect_rx,
        warn_tx,
    )?;
    println!("Press a button or move a stick. Ctrl-C to quit.");

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = shutdown.clone();
        ctrlc::set_handler(move || s.store(true, Ordering::SeqCst))?;
    }

    let mut buf = Vec::new();
    while !shutdown.load(Ordering::SeqCst) {
        buf.clear();
        source.poll(&mut buf);
        for ev in buf.drain(..) {
            match ev {
                GamepadEvent::Connected(_) => println!("🎮 Connected"),
                GamepadEvent::Disconnected => println!("🎮 Disconnected"),
                GamepadEvent::ButtonDown(id) =>
                    println!("  [button] id={id:<3} down       label: {:?}", label_for(id)),
                GamepadEvent::ButtonUp(id) =>
                    println!("  [button] id={id:<3} up         label: {:?}", label_for(id)),
                GamepadEvent::Stick { axis, value } =>
                    println!("  [axis  ] id={axis:<3} v={value:+.2}     (stick)"),
                GamepadEvent::Trigger { axis, value } =>
                    println!("  [trig  ] id={axis:<3} v={value:+.2}     (trigger normalized)"),
                GamepadEvent::MouseDelta { dx, dy } =>
                    println!("  [touch ] dx={dx:<+4} dy={dy:<+4}  (touchpad cursor)"),
                GamepadEvent::TouchpadClick { raw_x, raw_y, quadrant } =>
                    println!("  [tpclick] raw=({raw_x:<5},{raw_y:<5}) quadrant={quadrant}"),
                GamepadEvent::TouchpadHover { .. } => {}
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}
