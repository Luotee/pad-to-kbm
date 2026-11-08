use crate::config::Config;
use crate::engine::Engine;
use crate::safety;
use crate::shutdown::Cleanup;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct RunOptions {
    pub dry_run: bool,
    /// Folder of the config file; the `log_events` file goes there.
    pub log_dir: Option<std::path::PathBuf>,
    /// The config file; the HidHide state file goes beside it. `None`: no
    /// HidHide (the real pad is not hidden in 鍵鼠 / Xbox mode).
    pub config_path: Option<std::path::PathBuf>,
    pub shutdown: Arc<AtomicBool>,
}

pub fn run(cfg: Config, opts: RunOptions) -> Result<()> {
    // The panic hook is installed once, at the top of main::real_main, before
    // either CLI or GUI dispatch — so it is already active when we arrive here.
    // Iron Rule #3 is satisfied by that hook (abnormal path) and by
    // KeyboardSink::Drop → release_all_held (normal shutdown path).
    let engine = Engine::spawn(cfg, opts.dry_run)?;
    if let Some(dir) = opts.log_dir.clone() {
        engine.handle().set_log_dir(dir);
    }
    // Bind the engine's key state to the global so the panic hook can drain it.
    safety::register_global(engine.handle().key_state());
    // ...and to its virtual pad, so the hook can unplug it.
    crate::xinput_sink::register_global(engine.handle().virtual_pad());
    // HidHide, as in the GUI (not on a dry run, not off Windows).
    let hider = opts
        .config_path
        .as_deref()
        .and_then(|p| crate::hider::start_for(&engine.handle(), p, opts.dry_run));
    // engine.shutdown() signals the engine thread and joins it; the engine
    // thread's graceful-shutdown path drains macros and then drops
    // KeyboardSink, which calls release_all_held() — Iron Rule #3. Then the
    // HidHide worker undoes its hiding. Ctrl-C runs it below; closing the
    // console window (and, where Windows sends them, logoff / shutdown) runs
    // it from the console handler before the process ends.
    let cleanup = Arc::new(Cleanup::new(engine, hider));
    #[cfg(windows)]
    crate::shutdown::install_console_handler(cleanup.clone());
    while !opts.shutdown.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
    }
    cleanup.run("Ctrl-C");
    Ok(())
}
