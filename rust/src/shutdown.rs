//! The one cleanup every exit path runs (v3.6.0): shut the engine down, then
//! undo the HidHide hiding. Normal exit, a failed app build, Windows logging
//! off or shutting down (GUI: `WM_ENDSESSION`, `gui/session_end.rs`) and the
//! CLI's console close / logoff / shutdown events all call
//! [`Cleanup::run`]; only the first call does anything.
//!
//! Order is iron rule 3: the engine goes first (held keys and mouse buttons
//! released, virtual pad unplugged), then the real pad comes back, so a game
//! never sees a stuck key or both pads.

use crate::engine::Engine;
use crate::hider::HiderWorker;
use std::sync::Mutex;

/// Something [`Cleanup`] finishes exactly once. Returns whether it finished
/// in time.
pub trait Part: Send + 'static {
    fn finish(self) -> bool;
}

impl Part for Engine {
    fn finish(self) -> bool {
        self.shutdown();
        true
    }
}

impl Part for HiderWorker {
    fn finish(self) -> bool {
        // Waits at most ~2 s; false when the worker did not confirm. What it
        // could not undo, the next start does from the state file.
        self.stop()
    }
}

/// Holds the engine and the HidHide worker until the first [`Cleanup::run`].
pub struct Cleanup<E: Part = Engine, H: Part = HiderWorker> {
    parts: Mutex<Option<(E, Option<H>)>>,
}

impl<E: Part, H: Part> Cleanup<E, H> {
    pub fn new(engine: E, hider: Option<H>) -> Self {
        Cleanup { parts: Mutex::new(Some((engine, hider))) }
    }

    /// The first call shuts the engine down, then stops the hider, and logs
    /// `cleanup (<reason>): ...`; later calls do nothing. Returns whether this
    /// call did the work. A concurrent second caller waits until the first
    /// has finished, so every caller returns with the cleanup done.
    pub fn run(&self, reason: &str) -> bool {
        self.run_outcome(reason).is_some()
    }

    /// [`Cleanup::run`], telling what happened: `None` when an earlier call
    /// already did it; otherwise `Some` of the hider's result (`None`: no
    /// hider, `Some(false)`: its undo timed out).
    pub fn run_outcome(&self, reason: &str) -> Option<Option<bool>> {
        // Held for the whole cleanup on purpose (see above). A panic in a
        // part poisons it; the parts are gone by then, so carry on.
        let mut parts = self.parts.lock().unwrap_or_else(|p| p.into_inner());
        let (engine, hider) = parts.take()?;
        engine.finish();
        let hider = hider.map(Part::finish);
        tracing::info!("{}", message(reason, hider));
        Some(hider)
    }
}

/// The log line [`Cleanup::run`] writes (gui-smoke greps for it).
fn message(reason: &str, hider: Option<bool>) -> String {
    let tail = match hider {
        None => "",
        Some(true) => ", HidHide entries removed",
        Some(false) => ", HidHide undo timed out (next start finishes it)",
    };
    format!("cleanup ({reason}): engine shut down{tail}")
}

/// Console control events (wincon.h). Ctrl-C (0) and Ctrl-Break (1) are the
/// `ctrlc` handler's.
const CTRL_CLOSE_EVENT: u32 = 2;
const CTRL_LOGOFF_EVENT: u32 = 5;
const CTRL_SHUTDOWN_EVENT: u32 = 6;

/// The cleanup reason for a console control event, or `None` for the events
/// the console handler leaves to the next handler.
pub fn console_reason(event: u32) -> Option<&'static str> {
    match event {
        CTRL_CLOSE_EVENT => Some("console close"),
        CTRL_LOGOFF_EVENT => Some("logoff"),
        CTRL_SHUTDOWN_EVENT => Some("shutdown"),
        _ => None,
    }
}

/// The CLI's console handler runs this cleanup.
#[cfg(windows)]
static CONSOLE_CLEANUP: std::sync::OnceLock<std::sync::Arc<Cleanup>> = std::sync::OnceLock::new();

/// CLI (Windows): closing the console window, logging off or shutting down
/// would end the process with the hiding still in place. Register a console
/// handler that runs `cleanup` synchronously for those events, then answers
/// TRUE (the process ends right after). Ctrl-C / Ctrl-Break return FALSE,
/// so the `ctrlc` crate's handler, registered earlier and therefore asked
/// after this one, still sets the shutdown flag as before. Call once.
///
/// Windows does not send the logoff / shutdown events to a process that has
/// loaded user32.dll; the CLI has (enigo's SendInput), so for those two this
/// handler is a fallback and the next start undoes what is left.
#[cfg(windows)]
pub fn install_console_handler(cleanup: std::sync::Arc<Cleanup>) {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    if CONSOLE_CLEANUP.set(cleanup).is_err() {
        tracing::warn!("console handler already installed");
        return;
    }
    // SAFETY: registers a plain extern fn; it reads only the static above.
    if unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) } == 0 {
        tracing::warn!("could not install the console handler: {}", std::io::Error::last_os_error());
    }
}

#[cfg(windows)]
unsafe extern "system" fn console_handler(event: u32) -> windows_sys::Win32::Foundation::BOOL {
    let Some(reason) = console_reason(event) else { return 0 };
    if let Some(cleanup) = CONSOLE_CLEANUP.get() {
        // No unwinding across this extern fn.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cleanup.run(reason)));
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    type Log = Arc<StdMutex<Vec<&'static str>>>;

    struct Fake {
        name: &'static str,
        log: Log,
        ok: bool,
    }

    impl Part for Fake {
        fn finish(self) -> bool {
            self.log.lock().unwrap().push(self.name);
            self.ok
        }
    }

    fn parts(with_hider: bool) -> (Cleanup<Fake, Fake>, Log) {
        let log: Log = Arc::default();
        let engine = Fake { name: "engine", log: log.clone(), ok: true };
        let hider = with_hider.then(|| Fake { name: "hider", log: log.clone(), ok: true });
        (Cleanup::new(engine, hider), log)
    }

    #[test]
    fn cleanup_runs_once() {
        let (c, log) = parts(true);
        assert!(c.run("session end"));
        assert!(!c.run("exit"));
        assert!(!c.run("exit"));
        let log = log.lock().unwrap();
        assert_eq!(log.iter().filter(|n| **n == "engine").count(), 1);
        assert_eq!(log.iter().filter(|n| **n == "hider").count(), 1);
    }

    #[test]
    fn cleanup_order_engine_before_hider() {
        let (c, log) = parts(true);
        c.run("exit");
        assert_eq!(*log.lock().unwrap(), vec!["engine", "hider"]);
    }

    #[test]
    fn cleanup_without_hider_shuts_the_engine_down() {
        let (c, log) = parts(false);
        assert!(c.run("exit"));
        assert_eq!(*log.lock().unwrap(), vec!["engine"]);
    }

    #[test]
    fn cleanup_concurrent_callers_run_it_once() {
        let (c, log) = parts(true);
        let c = Arc::new(c);
        let did: usize = (0..8)
            .map(|_| {
                let c = c.clone();
                std::thread::spawn(move || c.run("race"))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap() as usize)
            .sum();
        assert_eq!(did, 1);
        assert_eq!(*log.lock().unwrap(), vec!["engine", "hider"]);
    }

    #[test]
    fn cleanup_message_names_hidhide_only_with_a_hider() {
        assert_eq!(message("session end", None), "cleanup (session end): engine shut down");
        assert_eq!(
            message("console close", Some(true)),
            "cleanup (console close): engine shut down, HidHide entries removed"
        );
    }

    #[test]
    fn console_events_close_logoff_shutdown_run_the_cleanup() {
        assert_eq!(console_reason(2), Some("console close"));
        assert_eq!(console_reason(5), Some("logoff"));
        assert_eq!(console_reason(6), Some("shutdown"));
        // Ctrl-C / Ctrl-Break stay with the ctrlc handler.
        assert_eq!(console_reason(0), None);
        assert_eq!(console_reason(1), None);
    }

    #[cfg(windows)]
    #[test]
    fn console_event_numbers_match_windows() {
        use windows_sys::Win32::System::Console as c;
        assert_eq!(
            (CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT),
            (c::CTRL_CLOSE_EVENT, c::CTRL_LOGOFF_EVENT, c::CTRL_SHUTDOWN_EVENT)
        );
    }

    #[test]
    fn cleanup_message_says_when_the_hidhide_undo_timed_out() {
        assert_eq!(
            message("exit", Some(false)),
            "cleanup (exit): engine shut down, HidHide undo timed out (next start finishes it)"
        );
    }

    #[test]
    fn cleanup_reports_a_hider_that_did_not_stop() {
        let log: Log = Arc::default();
        let engine = Fake { name: "engine", log: log.clone(), ok: true };
        let hider = Fake { name: "hider", log: log.clone(), ok: false };
        let c = Cleanup::new(engine, Some(hider));
        assert_eq!(c.run_outcome("exit"), Some(Some(false)));
        assert_eq!(c.run_outcome("exit"), None);
        assert_eq!(*log.lock().unwrap(), vec!["engine", "hider"]);
    }
}
