//! Hider (v3.6.0): keeps the real DualSense / Switch Pro hidden from games
//! with HidHide while that pad outputs a virtual Xbox pad, and undoes it
//! when Xbox output ends (iron rule 7).
//!
//! [`Hider`] is the decision logic over a [`hidhide::Control`], fed with
//! what is wanted (kinds in Xbox mode that Windows has a native driver for),
//! what is present (HID instance ids) and what the engine has connected. It
//! is unit-tested with a fake driver. [`HiderWorker`] runs it on its own
//! thread, opening the control device fresh for every round and closing it
//! right after: the device admits one handle at a time, and HidHide's own
//! client must stay usable.
//!
//! What a round changes in HidHide, in this order:
//! 1. whitelist += our exe's NT image path (unless the whitelist is
//!    inverted); if that fails nothing is blacklisted, or the app would hide
//!    the pad from itself;
//! 2. blacklist += present instance ids of wanted kinds, −= the ids we added
//!    for kinds no longer wanted. An id we add while its pad is present
//!    marks that kind "needs replug": a game may already hold it open;
//! 3. Active on if we have entries and it is off (remembered, so it is
//!    turned off again only if we turned it on and the blacklist is empty).
//!
//! Every change is recorded in `padtokbm.hidhide.json` beside the config
//! (deleted when nothing is left to undo), so a crash is undone at the next
//! start. Foreign entries are never dropped or reordered; compares are
//! case-insensitive, as the driver's.

use crate::engine::Handle;
use crate::gamepad::ControllerKind;
use crate::hidhide::{self, merge, unmerge, Control, Error, HideState};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The kind a HID instance id belongs to, if it is a pad the app hides.
fn kind_of(id: &str) -> Option<ControllerKind> {
    hidhide::kind_for_instance(id)
}

fn contains_ci(list: &[String], item: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(item))
}

// ─── Status ──────────────────────────────────────────────────────────────────

/// What the IPC reports per kind (`get_xbox_status.hidden`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HideStatus {
    /// A worker runs (Windows, not `--dry-run`).
    pub supported: bool,
    /// Kinds wanted hidden.
    pub wanted: Vec<ControllerKind>,
    /// Kinds hidden while present: one reconnect needed.
    pub needs_replug: Vec<ControllerKind>,
    /// The last round fully succeeded.
    pub ok: bool,
}

impl HideStatus {
    /// `None`: not applicable (not wanted, no native driver, no worker).
    /// `Some(false)`: needs one reconnect, or hiding is not applied yet.
    /// `Some(true)`: hidden.
    pub fn hidden(&self, kind: ControllerKind) -> Option<bool> {
        if !self.supported || !self.wanted.contains(&kind) {
            return None;
        }
        Some(self.ok && !self.needs_replug.contains(&kind))
    }
}

// ─── Decision logic ──────────────────────────────────────────────────────────

pub struct Hider {
    state: HideState,
    state_path: PathBuf,
    wanted: Vec<ControllerKind>,
    connected: Vec<ControllerKind>,
    needs_replug: Vec<ControllerKind>,
    ok: bool,
    /// Last logged failure, so a failing retry loop logs once.
    last_err: Option<String>,
    inverse_warned: bool,
}

impl Hider {
    /// Starts from the state file at `state_path` (a crash's leftovers, or
    /// nothing). Nothing is wanted until [`Hider::set_inputs`].
    pub fn new(state_path: PathBuf) -> Hider {
        let state = HideState::load(&state_path);
        if !state.is_empty() {
            tracing::info!(ids = state.ids.len(), "HidHide: undoing what the last run left ({})", state_path.display());
        }
        Hider {
            state,
            state_path,
            wanted: Vec::new(),
            connected: Vec::new(),
            needs_replug: Vec::new(),
            ok: false,
            last_err: None,
            inverse_warned: false,
        }
    }

    /// What we changed in HidHide and have not undone yet.
    pub fn state(&self) -> &HideState {
        &self.state
    }

    /// The last round fully succeeded.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// Some kind is wanted hidden.
    pub fn wants_any(&self) -> bool {
        !self.wanted.is_empty()
    }

    pub fn status(&self) -> HideStatus {
        HideStatus {
            supported: true,
            wanted: self.wanted.clone(),
            needs_replug: self.needs_replug.clone(),
            ok: self.ok,
        }
    }

    /// The kinds wanted hidden (only kinds with a native driver count) and
    /// the engine's connected kinds. A kind that left `connected`, or is no
    /// longer wanted, no longer needs a replug.
    pub fn set_inputs(&mut self, wanted: &[ControllerKind], connected: &[ControllerKind]) {
        let wanted: Vec<ControllerKind> = ControllerKind::ALL
            .into_iter()
            .filter(|k| k.has_native_driver() && wanted.contains(k))
            .collect();
        let before = &self.connected;
        self.needs_replug.retain(|k| wanted.contains(k) && !(before.contains(k) && !connected.contains(k)));
        self.wanted = wanted;
        self.connected = connected.to_vec();
    }

    /// One round (module docs). `present`: HID instance ids present now;
    /// `own`: our NT image path (`None`: unknown). Nothing wanted → the same
    /// as [`Hider::release_all`]. The driver is not opened when there is
    /// nothing to do.
    pub fn round<C: Control>(
        &mut self,
        open: impl FnOnce() -> Result<C, Error>,
        present: &[String],
        own: Option<&str>,
    ) -> Result<(), Error> {
        let result = if self.wanted.is_empty() { self.release_inner(open) } else { self.apply(open, present, own) };
        self.finish(result)
    }

    /// Undo everything we changed: our blacklist ids, our whitelist entry,
    /// Active if we turned it on and the blacklist is now empty. On success
    /// the state file is gone; on failure what is left stays recorded.
    pub fn release_all<C: Control>(&mut self, open: impl FnOnce() -> Result<C, Error>) -> Result<(), Error> {
        let result = self.release_inner(open);
        self.finish(result)
    }

    fn finish(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        self.ok = result.is_ok();
        match &result {
            Ok(()) => {
                if self.last_err.take().is_some() {
                    tracing::info!("HidHide: applied");
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if self.last_err.as_deref() != Some(msg.as_str()) {
                    tracing::warn!("HidHide: {msg}; retrying");
                    self.last_err = Some(msg);
                }
            }
        }
        result
    }

    fn save(&self) {
        if let Err(e) = self.state.save(&self.state_path) {
            tracing::warn!("HidHide: could not write {}: {e}", self.state_path.display());
        }
    }

    fn apply<C: Control>(
        &mut self,
        open: impl FnOnce() -> Result<C, Error>,
        present: &[String],
        own: Option<&str>,
    ) -> Result<(), Error> {
        let ctl = open()?;

        // 1. Our exe may see hidden devices.
        if !ctl.inverse()? {
            let own = own.ok_or_else(|| Error::Io(std::io::Error::other("our NT image path is unknown")))?;
            let white = ctl.whitelist()?;
            // Ours from an earlier run at another path (exe moved) goes.
            let stale: Vec<String> =
                self.state.whitelisted.iter().filter(|p| !p.eq_ignore_ascii_case(own)).cloned().collect();
            let next = merge(&unmerge(&white, &stale), &[own.to_string()]);
            if next != white {
                ctl.set_whitelist(&next)?;
                if !contains_ci(&white, own) {
                    self.state.whitelisted = Some(own.to_string());
                } else if !stale.is_empty() {
                    self.state.whitelisted = None;
                }
                self.save();
            }
        } else if let Some(own) = own {
            if !self.inverse_warned && contains_ci(&ctl.whitelist()?, own) {
                self.inverse_warned = true;
                tracing::warn!("HidHide: the inverted whitelist lists {own}: the app cannot read hidden pads");
            }
        }

        // 2. Blacklist: + present ids of wanted kinds, − ours of unwanted kinds.
        let black = ctl.blacklist()?;
        let wanted = &self.wanted;
        let to_hide: Vec<String> =
            present.iter().filter(|id| kind_of(id).is_some_and(|k| wanted.contains(&k))).cloned().collect();
        let added: Vec<String> = merge(&[], &to_hide).into_iter().filter(|id| !contains_ci(&black, id)).collect();
        let dropped: Vec<String> =
            self.state.ids.iter().filter(|id| !kind_of(id).is_some_and(|k| wanted.contains(&k))).cloned().collect();
        let next = merge(&unmerge(&black, &dropped), &added);
        if next != black {
            ctl.set_blacklist(&next)?;
        }
        if !added.is_empty() || !dropped.is_empty() {
            self.state.ids = merge(&unmerge(&self.state.ids, &dropped), &added);
            self.save();
        }
        for kind in added.iter().filter_map(|id| kind_of(id)) {
            if !self.needs_replug.contains(&kind) {
                self.needs_replug.push(kind);
            }
        }
        // A kind none of whose ids is present any more has left.
        self.needs_replug.retain(|k| present.iter().any(|id| kind_of(id) == Some(*k)));

        // 3. The driver hides nothing while Active is off.
        if !self.state.ids.is_empty() && !ctl.active()? {
            ctl.set_active(true)?;
            self.state.activated_by_us = true;
            self.save();
        }
        Ok(())
    }

    fn release_inner<C: Control>(&mut self, open: impl FnOnce() -> Result<C, Error>) -> Result<(), Error> {
        self.needs_replug.clear();
        if self.state.is_empty() {
            return Ok(());
        }
        let ctl = open()?;
        let black = ctl.blacklist()?;
        let mut next = black.clone();
        if !self.state.ids.is_empty() {
            next = unmerge(&black, &self.state.ids);
            if next != black {
                ctl.set_blacklist(&next)?;
            }
            self.state.ids.clear();
            self.save();
        }
        if let Some(own) = self.state.whitelisted.clone() {
            let white = ctl.whitelist()?;
            let without = unmerge(&white, &[own]);
            if without != white {
                ctl.set_whitelist(&without)?;
            }
            self.state.whitelisted = None;
            self.save();
        }
        if self.state.activated_by_us {
            // Foreign entries left: the user's own hiding stays on.
            if next.is_empty() && ctl.active()? {
                ctl.set_active(false)?;
            }
            self.state.activated_by_us = false;
            self.save();
        }
        Ok(())
    }
}


// ─── Worker ──────────────────────────────────────────────────────────────────

/// Where a round gets its driver handle, the present devices and our path.
/// [`RealBackend`] is HidHide itself; tests supply a fake.
pub trait Backend: Send + 'static {
    type Ctl: Control;
    fn open(&mut self) -> Result<Self::Ctl, Error>;
    fn present(&mut self) -> Vec<String>;
    fn own_path(&mut self) -> Option<String>;
}

enum Msg {
    Update { wanted: Vec<ControllerKind>, connected: Vec<ControllerKind> },
    Retry,
    Stop(Sender<()>),
}

/// The engine's and the commands' side of the worker. Clones cheaply; sends
/// never block.
#[derive(Clone)]
pub struct HiderLink {
    tx: Sender<Msg>,
    status: Arc<RwLock<HideStatus>>,
}

impl HiderLink {
    /// The kinds wanted hidden and the engine's connected kinds changed.
    pub fn update(&self, wanted: Vec<ControllerKind>, connected: Vec<ControllerKind>) {
        let _ = self.tx.send(Msg::Update { wanted, connected });
    }

    /// Run a round now (after HidHide was installed).
    pub fn retry(&self) {
        let _ = self.tx.send(Msg::Retry);
    }

    pub fn status(&self) -> HideStatus {
        self.status.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// See [`HideStatus::hidden`].
    pub fn hidden(&self, kind: ControllerKind) -> Option<bool> {
        self.status.read().unwrap_or_else(|p| p.into_inner()).hidden(kind)
    }
}

/// The worker thread. [`HiderWorker::stop`] (or drop) undoes everything and
/// ends it.
pub struct HiderWorker {
    link: HiderLink,
    thread: Option<JoinHandle<()>>,
}

impl HiderWorker {
    /// Start the worker with the state file at `state_path`. It does nothing
    /// until the first [`HiderLink::update`]; after a round that did not
    /// fully succeed it retries every `retry_every`. `on_change` runs (on the
    /// worker thread) whenever the status changes.
    pub fn spawn<B: Backend>(
        state_path: PathBuf,
        backend: B,
        retry_every: Duration,
        on_change: Box<dyn Fn() + Send>,
    ) -> std::io::Result<HiderWorker> {
        let hider = Hider::new(state_path);
        let status = Arc::new(RwLock::new(hider.status()));
        let (tx, rx) = unbounded();
        let st = status.clone();
        let thread = thread::Builder::new()
            .name("hidhide".into())
            .spawn(move || run_worker(hider, backend, rx, st, retry_every, on_change))?;
        Ok(HiderWorker { link: HiderLink { tx, status }, thread: Some(thread) })
    }

    pub fn link(&self) -> HiderLink {
        self.link.clone()
    }

    /// Undo everything and end the thread, waiting at most ~2 s. Returns
    /// whether the worker confirmed.
    pub fn stop(mut self) -> bool {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> bool {
        let Some(thread) = self.thread.take() else { return true };
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        if self.link.tx.send(Msg::Stop(ack_tx)).is_err() {
            return false;
        }
        match ack_rx.recv_timeout(STOP_WAIT) {
            Ok(()) => {
                let _ = thread.join();
                true
            }
            Err(_) => {
                tracing::warn!("HidHide: worker did not stop in time; the next start undoes what is left");
                false
            }
        }
    }
}

impl Drop for HiderWorker {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

/// [`HiderWorker::stop`] waits at most this long for the worker.
const STOP_WAIT: Duration = Duration::from_secs(2);
/// The worker keeps retrying a busy driver at stop for at most this long.
const RELEASE_DEADLINE: Duration = Duration::from_millis(1500);
const RELEASE_RETRY: Duration = Duration::from_millis(100);
/// Between rounds while the last one did not fully succeed.
pub const RETRY_EVERY: Duration = Duration::from_secs(2);

fn run_worker<B: Backend>(
    mut hider: Hider,
    mut backend: B,
    rx: Receiver<Msg>,
    status: Arc<RwLock<HideStatus>>,
    retry_every: Duration,
    on_change: Box<dyn Fn() + Send>,
) {
    let publish = |st: HideStatus| {
        let mut cur = status.write().unwrap_or_else(|p| p.into_inner());
        if *cur != st {
            *cur = st;
            drop(cur);
            on_change();
        }
    };
    // No round before the first inputs: a crash's leftovers are undone only
    // for the kinds the current config does not want.
    let mut started = false;
    loop {
        let first = if started && !hider.ok() {
            match rx.recv_timeout(retry_every) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => Some(Msg::Retry),
                Err(RecvTimeoutError::Disconnected) => None,
            }
        } else {
            rx.recv().ok()
        };
        let Some(first) = first else {
            // Every link is gone: Xbox output has ended.
            release_before_exit(&mut hider, &mut backend);
            return;
        };
        let (mut run, mut stop) = (false, None);
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            match msg {
                Msg::Update { wanted, connected } => {
                    hider.set_inputs(&wanted, &connected);
                    started = true;
                    run = true;
                }
                Msg::Retry => run = true,
                Msg::Stop(ack) => stop = Some(ack),
            }
        }
        if let Some(ack) = stop {
            release_before_exit(&mut hider, &mut backend);
            publish(HideStatus::default());
            let _ = ack.send(());
            return;
        }
        if run && started {
            let (present, own) =
                if hider.wants_any() { (backend.present(), backend.own_path()) } else { (Vec::new(), None) };
            let _ = hider.round(|| backend.open(), &present, own.as_deref());
        }
        publish(hider.status());
    }
}

/// Graceful exit: undo everything, retrying a busy driver for a moment. What
/// cannot be undone now stays in the state file for the next start.
fn release_before_exit<B: Backend>(hider: &mut Hider, backend: &mut B) {
    let until = Instant::now() + RELEASE_DEADLINE;
    loop {
        match hider.release_all(|| backend.open()) {
            Ok(()) => return,
            Err(Error::Busy) | Err(Error::Io(_)) if Instant::now() + RELEASE_RETRY < until => {
                thread::sleep(RELEASE_RETRY)
            }
            Err(_) => return,
        }
    }
}

/// HidHide itself.
pub struct RealBackend {
    own: Option<String>,
    own_warned: bool,
}

impl RealBackend {
    pub fn new() -> RealBackend {
        RealBackend { own: None, own_warned: false }
    }
}

impl Default for RealBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for RealBackend {
    type Ctl = hidhide::Driver;
    fn open(&mut self) -> Result<hidhide::Driver, Error> {
        hidhide::Driver::open()
    }
    fn present(&mut self) -> Vec<String> {
        hidhide::present_hid_instance_ids()
    }
    /// Asked again until it is known.
    fn own_path(&mut self) -> Option<String> {
        if self.own.is_none() {
            match hidhide::own_image_nt_path() {
                Ok(p) => {
                    tracing::info!("HidHide: whitelisting this exe as {p}");
                    self.own = Some(p);
                }
                Err(e) if !self.own_warned => {
                    self.own_warned = true;
                    tracing::warn!("HidHide: no NT path for this exe: {e}");
                }
                Err(_) => {}
            }
        }
        self.own.clone()
    }
}

/// Start hiding for an engine whose config lives at `config_path` (state
/// file beside it) and attach it to the engine, which then feeds it. `None`
/// (nothing started, nothing touched) off Windows and on `--dry-run`.
pub fn start_for(engine: &Handle, config_path: &Path, dry_run: bool) -> Option<HiderWorker> {
    if dry_run || !cfg!(windows) {
        tracing::debug!("HidHide: not used (dry run or not Windows)");
        return None;
    }
    let path = HideState::path_beside(config_path);
    match HiderWorker::spawn(path, RealBackend::new(), RETRY_EVERY, Box::new(engine.status_notifier())) {
        Ok(worker) => {
            engine.attach_hider(worker.link());
            Some(worker)
        }
        Err(e) => {
            tracing::warn!("HidHide: could not start the worker: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use ControllerKind::{DualSense as DS, Ns2Pro as NS2, NsPro as NSP};

    const DS_USB: &str = r"HID\VID_054C&PID_0CE6&MI_03\7&1a2b&0&0000";
    const DS_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&2d1c&0&0000";
    const NSP_BT: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\9&1&0&0000";
    const NS2_BLE: &str = r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02057e_PID&2069_REV&0001\9&1&0&0000";
    const MOUSE: &str = r"HID\VID_046D&PID_C52B&MI_01&COL02\7&abc&0&0001";
    const OWN: &str = r"\Device\HarddiskVolume3\Games\PadToKBM.exe";
    const FOREIGN_APP: &str = r"\Device\HarddiskVolume3\Tools\DS4Windows.exe";
    const FOREIGN_DEV: &str = r"HID\VID_045E&PID_0B13\8&x&0&0000";

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum OpenErr {
        Busy,
        Absent,
    }

    #[derive(Debug, Default)]
    struct Fake {
        black: Vec<String>,
        white: Vec<String>,
        active: bool,
        inverse: bool,
        open_err: Option<OpenErr>,
        fail_set_whitelist: bool,
        opens: usize,
        /// Every call, in order ("bl", "set_bl", "wl", "set_wl", ...).
        calls: Vec<&'static str>,
    }

    #[derive(Clone)]
    struct FakeCtl(Arc<Mutex<Fake>>);

    impl FakeCtl {
        fn new(f: Fake) -> FakeCtl {
            FakeCtl(Arc::new(Mutex::new(f)))
        }
        fn f(&self) -> std::sync::MutexGuard<'_, Fake> {
            self.0.lock().unwrap()
        }
        fn open(&self) -> impl FnOnce() -> Result<FakeCtl, Error> + '_ {
            move || {
                let mut f = self.f();
                f.opens += 1;
                match f.open_err {
                    Some(OpenErr::Busy) => Err(Error::Busy),
                    Some(OpenErr::Absent) => Err(Error::Absent),
                    None => Ok(self.clone()),
                }
            }
        }
    }

    impl Control for FakeCtl {
        fn blacklist(&self) -> Result<Vec<String>, Error> {
            let mut f = self.f();
            f.calls.push("bl");
            Ok(f.black.clone())
        }
        fn set_blacklist(&self, list: &[String]) -> Result<(), Error> {
            let mut f = self.f();
            f.calls.push("set_bl");
            f.black = list.to_vec();
            Ok(())
        }
        fn whitelist(&self) -> Result<Vec<String>, Error> {
            let mut f = self.f();
            f.calls.push("wl");
            Ok(f.white.clone())
        }
        fn set_whitelist(&self, list: &[String]) -> Result<(), Error> {
            let mut f = self.f();
            f.calls.push("set_wl");
            if f.fail_set_whitelist {
                return Err(Error::Io(std::io::Error::other("set whitelist refused")));
            }
            f.white = list.to_vec();
            Ok(())
        }
        fn active(&self) -> Result<bool, Error> {
            let mut f = self.f();
            f.calls.push("active");
            Ok(f.active)
        }
        fn set_active(&self, on: bool) -> Result<(), Error> {
            let mut f = self.f();
            f.calls.push("set_active");
            f.active = on;
            Ok(())
        }
        fn inverse(&self) -> Result<bool, Error> {
            let mut f = self.f();
            f.calls.push("inverse");
            Ok(f.inverse)
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        path: PathBuf,
        ctl: FakeCtl,
        hider: Hider,
    }

    fn rig(f: Fake) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = HideState::path_beside(&dir.path().join("padtokbm.json"));
        let hider = Hider::new(path.clone());
        Rig { _dir: dir, path, ctl: FakeCtl::new(f), hider }
    }

    fn round(r: &mut Rig, present: &[&str]) -> Result<(), Error> {
        let ctl = r.ctl.clone();
        r.hider.round(ctl.open(), &s(present), Some(OWN))
    }

    fn release(r: &mut Rig) -> Result<(), Error> {
        let ctl = r.ctl.clone();
        r.hider.release_all(ctl.open())
    }

    fn foreign() -> Fake {
        Fake { black: s(&[FOREIGN_DEV]), white: s(&[FOREIGN_APP]), ..Default::default() }
    }

    // ── the round ──

    #[test]
    fn hides_present_ids_of_wanted_kinds_and_keeps_foreign_entries() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[MOUSE, DS_USB, NSP_BT]).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.black, s(&[FOREIGN_DEV, DS_USB]), "foreign first, in order; only the wanted kind's id");
        assert_eq!(f.white, s(&[FOREIGN_APP, OWN]));
        assert!(f.active);
        drop(f);
        assert_eq!(r.hider.state().ids, s(&[DS_USB]));
        assert_eq!(r.hider.state().whitelisted.as_deref(), Some(OWN));
        assert!(r.hider.state().activated_by_us);
        assert_eq!(HideState::load(&r.path), *r.hider.state(), "persisted");
    }

    #[test]
    fn rounds_are_idempotent() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        let (black, white) = {
            let f = r.ctl.f();
            (f.black.clone(), f.white.clone())
        };
        r.ctl.f().calls.clear();
        round(&mut r, &[DS_USB]).unwrap();
        round(&mut r, &[DS_USB]).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.black, black);
        assert_eq!(f.white, white);
        assert!(!f.calls.iter().any(|c| c.starts_with("set_")), "nothing to write: {:?}", f.calls);
    }

    #[test]
    fn whitelist_is_written_before_the_blacklist() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        let calls = r.ctl.f().calls.clone();
        let wl = calls.iter().position(|c| *c == "set_wl").expect("whitelist written");
        let bl = calls.iter().position(|c| *c == "set_bl").expect("blacklist written");
        assert!(wl < bl, "{calls:?}");
    }

    #[test]
    fn a_failed_whitelist_blocks_the_blacklist() {
        let mut r = rig(Fake { fail_set_whitelist: true, ..Default::default() });
        r.hider.set_inputs(&[DS], &[DS]);
        assert!(round(&mut r, &[DS_USB]).is_err());
        let f = r.ctl.f();
        assert!(f.black.is_empty(), "we must never hide the pad from ourselves");
        assert!(!f.active);
        drop(f);
        assert!(!r.hider.ok());
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        assert!(!r.path.exists(), "nothing changed, nothing recorded");
    }

    #[test]
    fn an_unknown_own_path_blocks_the_blacklist() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[DS]);
        let ctl = r.ctl.clone();
        assert!(r.hider.round(ctl.open(), &s(&[DS_USB]), None).is_err());
        assert!(r.ctl.f().black.is_empty());
        assert_eq!(r.hider.status().hidden(DS), Some(false));
    }

    #[test]
    fn inverse_mode_skips_the_whitelist() {
        let mut r = rig(Fake { inverse: true, white: s(&[FOREIGN_APP]), ..Default::default() });
        r.hider.set_inputs(&[DS], &[DS]);
        let ctl = r.ctl.clone();
        // Our path is not needed in inverse mode.
        r.hider.round(ctl.open(), &s(&[DS_USB]), None).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.white, s(&[FOREIGN_APP]), "an inverted whitelist lists apps that may NOT see");
        assert!(!f.calls.contains(&"set_wl"));
        assert_eq!(f.black, s(&[DS_USB]));
        drop(f);
        assert_eq!(r.hider.state().whitelisted, None);
    }

    #[test]
    fn own_path_already_whitelisted_is_not_ours_to_remove() {
        let mut r = rig(Fake { white: s(&[&OWN.to_lowercase()]), ..Default::default() });
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().white, s(&[&OWN.to_lowercase()]), "case-insensitive: no duplicate");
        assert_eq!(r.hider.state().whitelisted, None);
        release(&mut r).unwrap();
        assert_eq!(r.ctl.f().white, s(&[&OWN.to_lowercase()]), "the user's entry stays");
    }

    #[test]
    fn an_id_already_blacklisted_by_the_user_is_not_recorded_or_removed() {
        let mut r = rig(Fake { black: s(&[&DS_USB.to_lowercase()]), active: true, ..Default::default() });
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[&DS_USB.to_lowercase()]));
        assert!(r.hider.state().ids.is_empty());
        assert_eq!(r.hider.status().hidden(DS), Some(true), "already hidden: no replug");
        r.hider.set_inputs(&[], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[&DS_USB.to_lowercase()]), "not ours");
        assert!(r.ctl.f().active, "not turned on by us");
    }

    #[test]
    fn active_already_on_is_not_ours() {
        let mut r = rig(Fake { active: true, ..Default::default() });
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert!(!r.hider.state().activated_by_us);
        release(&mut r).unwrap();
        assert!(r.ctl.f().active, "the user turned it on");
    }

    // ── needs replug ──

    #[test]
    fn hiding_a_present_pad_needs_a_replug_until_it_leaves() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false), "a game may hold it open");
        // Still connected: still needs it.
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        // It left the engine (⏻): cleared.
        r.hider.set_inputs(&[DS], &[]);
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        // And back: the id is already blacklisted, so it arrives hidden.
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
    }

    #[test]
    fn needs_replug_clears_when_the_pad_is_no_longer_present() {
        // Hidden while present but not (yet) connected in the engine; then
        // it goes away without ever connecting.
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[]);
        round(&mut r, &[DS_BT]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
    }

    #[test]
    fn needs_replug_is_per_kind() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS, NSP], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        assert_eq!(r.hider.status().hidden(NSP), Some(true), "nothing of it was present");
    }

    #[test]
    fn disconnect_keeps_the_entries() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        r.hider.set_inputs(&[DS], &[]);
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]), "a reconnect is hidden before Windows hands it to the game");
        assert_eq!(r.hider.state().ids, s(&[DS_USB]));
    }

    // ── mode changes and cleanup ──

    #[test]
    fn leaving_xbox_removes_that_kinds_entries_only() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS, NSP], &[DS, NSP]);
        round(&mut r, &[DS_USB, NSP_BT]).unwrap();
        r.hider.set_inputs(&[NSP], &[DS, NSP]);
        round(&mut r, &[DS_USB, NSP_BT]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, NSP_BT]));
        assert_eq!(r.hider.state().ids, s(&[NSP_BT]));
        assert_eq!(r.hider.status().hidden(DS), None, "not wanted");
    }

    #[test]
    fn nothing_wanted_restores_the_original_lists_and_active() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB, DS_BT]).unwrap();
        r.hider.set_inputs(&[], &[DS]);
        round(&mut r, &[DS_USB, DS_BT]).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.black, s(&[FOREIGN_DEV]));
        assert_eq!(f.white, s(&[FOREIGN_APP]));
        assert!(f.active, "foreign entries are left: Active stays on");
        drop(f);
        assert!(r.hider.state().is_empty());
        assert!(!r.path.exists(), "nothing left to undo: no state file");
    }

    #[test]
    fn release_all_restores_the_exact_original_lists_and_active() {
        let mut r = rig(foreign());
        r.ctl.f().black.clear(); // empty blacklist: Active must go back off
        r.hider.set_inputs(&[DS, NSP], &[DS]);
        round(&mut r, &[DS_USB, NSP_BT, MOUSE]).unwrap();
        assert!(r.ctl.f().active);
        release(&mut r).unwrap();
        let f = r.ctl.f();
        assert!(f.black.is_empty());
        assert_eq!(f.white, s(&[FOREIGN_APP]));
        assert!(!f.active, "we turned it on and nothing is left");
        drop(f);
        assert!(r.hider.state().is_empty());
        assert!(!r.path.exists());
    }

    #[test]
    fn release_all_with_nothing_recorded_does_not_open_the_driver() {
        let mut r = rig(Fake { open_err: Some(OpenErr::Absent), ..Default::default() });
        release(&mut r).unwrap();
        r.hider.set_inputs(&[], &[]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().opens, 0);
        assert!(r.hider.ok());
    }

    #[test]
    fn a_failed_release_keeps_the_state_for_the_next_start() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        r.ctl.f().open_err = Some(OpenErr::Busy);
        assert!(matches!(release(&mut r), Err(Error::Busy)));
        assert_eq!(HideState::load(&r.path).ids, s(&[DS_USB]));
    }

    #[test]
    fn crash_recovery_undoes_unwanted_kinds_and_keeps_wanted_ones() {
        let mut r = rig(Fake {
            black: s(&[FOREIGN_DEV, DS_USB, NSP_BT]),
            white: s(&[FOREIGN_APP, OWN]),
            active: true,
            ..Default::default()
        });
        HideState { ids: s(&[DS_USB, NSP_BT]), activated_by_us: true, whitelisted: Some(OWN.into()) }
            .save(&r.path)
            .unwrap();
        r.hider = Hider::new(r.path.clone());
        // The current config wants the Switch Pro only; neither pad is here.
        r.hider.set_inputs(&[NSP], &[]);
        round(&mut r, &[MOUSE]).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.black, s(&[FOREIGN_DEV, NSP_BT]));
        assert_eq!(f.white, s(&[FOREIGN_APP, OWN]));
        drop(f);
        assert_eq!(HideState::load(&r.path).ids, s(&[NSP_BT]));
        // Then Xbox output ends: back to the user's own setup.
        release(&mut r).unwrap();
        let f = r.ctl.f();
        assert_eq!(f.black, s(&[FOREIGN_DEV]));
        assert_eq!(f.white, s(&[FOREIGN_APP]));
        assert!(f.active, "a foreign entry is left");
    }

    #[test]
    fn crash_recovery_with_nothing_wanted_releases_everything() {
        let mut r = rig(Fake { black: s(&[DS_USB]), white: s(&[OWN]), active: true, ..Default::default() });
        HideState { ids: s(&[DS_USB]), activated_by_us: true, whitelisted: Some(OWN.into()) }.save(&r.path).unwrap();
        r.hider = Hider::new(r.path.clone());
        r.hider.set_inputs(&[], &[]);
        round(&mut r, &[]).unwrap();
        let f = r.ctl.f();
        assert!(f.black.is_empty() && f.white.is_empty() && !f.active, "{f:?}");
        drop(f);
        assert!(!r.path.exists());
    }

    #[test]
    fn a_moved_exe_replaces_our_old_whitelist_entry() {
        let old = r"\Device\HarddiskVolume2\Old\PadToKBM.exe";
        let mut r = rig(Fake { white: s(&[FOREIGN_APP, old]), ..Default::default() });
        HideState { whitelisted: Some(old.into()), ..Default::default() }.save(&r.path).unwrap();
        r.hider = Hider::new(r.path.clone());
        r.hider.set_inputs(&[DS], &[]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().white, s(&[FOREIGN_APP, OWN]));
        assert_eq!(r.hider.state().whitelisted.as_deref(), Some(OWN));
    }

    // ── driver unavailable ──

    #[test]
    fn busy_or_absent_reports_not_hidden_and_the_next_round_retries() {
        for err in [OpenErr::Busy, OpenErr::Absent] {
            let mut r = rig(Fake { open_err: Some(err), ..Default::default() });
            r.hider.set_inputs(&[DS], &[]);
            assert!(round(&mut r, &[DS_USB]).is_err());
            assert!(!r.hider.ok());
            assert_eq!(r.hider.status().hidden(DS), Some(false), "{err:?}");
            r.ctl.f().open_err = None;
            round(&mut r, &[DS_USB]).unwrap();
            assert!(r.hider.ok());
            assert_eq!(r.ctl.f().black, s(&[DS_USB]));
        }
    }

    #[test]
    fn nothing_is_applied_before_the_first_round() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS], &[]);
        assert_eq!(r.hider.status().hidden(DS), Some(false));
    }

    // ── Switch 2 Pro ──

    #[test]
    fn the_switch_2_pro_is_never_hidden() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[NS2], &[NS2]);
        round(&mut r, &[NS2_BLE]).unwrap();
        assert!(r.ctl.f().black.is_empty());
        assert_eq!(r.ctl.f().opens, 0, "nothing wanted that Windows drives natively");
        assert_eq!(r.hider.status().hidden(NS2), None);
        r.hider.set_inputs(&[DS, NS2], &[NS2]);
        round(&mut r, &[NS2_BLE, DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]));
        assert_eq!(r.hider.status().hidden(NS2), None);
    }

    #[test]
    fn status_hidden_is_none_without_a_worker() {
        let st = HideStatus { supported: false, wanted: vec![DS], needs_replug: vec![], ok: true };
        assert_eq!(st.hidden(DS), None);
    }

    // ── worker ──

    struct FakeBackend {
        ctl: FakeCtl,
        present: Arc<Mutex<Vec<String>>>,
    }

    impl Backend for FakeBackend {
        type Ctl = FakeCtl;
        fn open(&mut self) -> Result<FakeCtl, Error> {
            (self.ctl.open())()
        }
        fn present(&mut self) -> Vec<String> {
            self.present.lock().unwrap().clone()
        }
        fn own_path(&mut self) -> Option<String> {
            Some(OWN.into())
        }
    }

    struct WRig {
        dir: tempfile::TempDir,
        ctl: FakeCtl,
        present: Arc<Mutex<Vec<String>>>,
        changes: Arc<std::sync::atomic::AtomicUsize>,
        worker: HiderWorker,
    }

    fn wrig(f: Fake, present: &[&str], state: Option<HideState>) -> WRig {
        let dir = tempfile::tempdir().unwrap();
        let path = HideState::path_beside(&dir.path().join("padtokbm.json"));
        if let Some(st) = state {
            st.save(&path).unwrap();
        }
        let ctl = FakeCtl::new(f);
        let present = Arc::new(Mutex::new(s(present)));
        let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = changes.clone();
        let worker = HiderWorker::spawn(
            path,
            FakeBackend { ctl: ctl.clone(), present: present.clone() },
            Duration::from_millis(20),
            Box::new(move || {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .unwrap();
        WRig { dir, ctl, present, changes, worker }
    }

    fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(3);
        while !cond() {
            assert!(Instant::now() < until, "timed out waiting for: {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_hides_on_update_and_reports_the_status() {
        let w = wrig(Fake::default(), &[DS_USB], None);
        let link = w.worker.link();
        assert_eq!(link.hidden(DS), None, "nothing wanted yet");
        link.update(vec![DS], vec![DS]);
        eventually("blacklisted", || w.ctl.f().black == s(&[DS_USB]));
        eventually("needs replug", || link.hidden(DS) == Some(false));
        assert!(w.changes.load(std::sync::atomic::Ordering::SeqCst) > 0, "on_change ran");
        link.update(vec![DS], vec![]);
        eventually("left: hidden", || link.hidden(DS) == Some(true));
    }

    #[test]
    fn worker_does_nothing_before_the_first_update() {
        // A crash's state must not be released before the config is known.
        let st = HideState { ids: s(&[DS_USB]), activated_by_us: true, whitelisted: None };
        let w = wrig(Fake { black: s(&[DS_USB]), active: true, ..Default::default() }, &[], Some(st));
        thread::sleep(Duration::from_millis(100));
        assert_eq!(w.ctl.f().opens, 0);
        w.worker.link().update(vec![DS], vec![]);
        eventually("a round ran", || w.ctl.f().opens > 0);
        assert_eq!(w.ctl.f().black, s(&[DS_USB]), "still wanted: kept");
    }

    #[test]
    fn worker_retries_until_the_driver_answers() {
        let w = wrig(Fake { open_err: Some(OpenErr::Busy), ..Default::default() }, &[DS_USB], None);
        let link = w.worker.link();
        link.update(vec![DS], vec![]);
        eventually("several attempts", || w.ctl.f().opens >= 3);
        assert_eq!(link.hidden(DS), Some(false));
        w.ctl.f().open_err = None;
        eventually("applied on a retry", || w.ctl.f().black == s(&[DS_USB]));
        let opens = w.ctl.f().opens;
        thread::sleep(Duration::from_millis(100));
        assert_eq!(w.ctl.f().opens, opens, "no polling once applied");
    }

    #[test]
    fn worker_retry_runs_a_round_for_a_new_device() {
        let w = wrig(Fake::default(), &[], None);
        let link = w.worker.link();
        link.update(vec![DS], vec![]);
        eventually("first round", || w.ctl.f().opens == 1);
        w.present.lock().unwrap().push(DS_BT.into());
        link.retry();
        eventually("hidden", || w.ctl.f().black == s(&[DS_BT]));
    }

    #[test]
    fn worker_stop_undoes_everything_and_ends() {
        let w = wrig(foreign(), &[DS_USB], None);
        w.worker.link().update(vec![DS], vec![DS]);
        eventually("hidden", || w.ctl.f().black.len() == 2);
        let t = Instant::now();
        assert!(w.worker.stop(), "confirmed");
        assert!(t.elapsed() < Duration::from_secs(2));
        let f = w.ctl.f();
        assert_eq!(f.black, s(&[FOREIGN_DEV]));
        assert_eq!(f.white, s(&[FOREIGN_APP]));
        drop(f);
        assert!(!HideState::path_beside(&w.dir.path().join("padtokbm.json")).exists());
    }

    #[test]
    fn worker_stop_while_busy_gives_up_in_time_and_keeps_the_state() {
        let w = wrig(Fake::default(), &[DS_USB], None);
        w.worker.link().update(vec![DS], vec![DS]);
        eventually("hidden", || !w.ctl.f().black.is_empty());
        w.ctl.f().open_err = Some(OpenErr::Busy);
        let t = Instant::now();
        w.worker.stop();
        assert!(t.elapsed() < Duration::from_millis(2500), "{:?}", t.elapsed());
        let path = HideState::path_beside(&w.dir.path().join("padtokbm.json"));
        assert_eq!(HideState::load(&path).ids, s(&[DS_USB]), "undone at the next start");
    }

    #[test]
    fn dropping_the_worker_stops_it() {
        let w = wrig(Fake::default(), &[DS_USB], None);
        w.worker.link().update(vec![DS], vec![DS]);
        eventually("hidden", || !w.ctl.f().black.is_empty());
        let WRig { ctl, worker, .. } = w;
        drop(worker);
        assert!(ctl.f().black.is_empty());
    }
}
