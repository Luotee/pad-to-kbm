//! Hider (v3.6.0): keeps the real DualSense / Switch Pro hidden from games
//! with HidHide while that pad is on 鍵鼠 or Xbox, and undoes it when the
//! pad leaves both (iron rule 7).
//!
//! [`Hider`] is the decision logic over a [`hidhide::Control`], fed with
//! what is wanted (kinds in 鍵鼠 or Xbox mode that Windows has a native driver for),
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
//! 2. blacklist += present instance ids of wanted kinds and the remembered
//!    ids (`hidhide_known`) of wanted kinds, present or not, −= the ids we
//!    added for kinds no longer wanted and the ids no longer remembered that
//!    are absent. An id we add while its pad is present marks that kind
//!    "needs replug": a game may already hold it open; a remembered pad
//!    hidden while absent arrives hidden when it is turned on. Those ids
//!    ([`Hider::replug_ids`]) are the ones the engine powers off, by their
//!    Bluetooth address ([`HideStatus::replug_addrs`]);
//! 3. Active on if we have entries and it is off (remembered, so it is
//!    turned off again only if we turned it on and the blacklist is empty).
//!
//! A present pad of a wanted kind that we hid and that is not remembered yet
//! is reported once ([`Hider::take_learned`]); the worker adds its Bluetooth
//! address ([`Backend::bt_addr`]) and hands it to `on_learn`, which saves it
//! to the config ([`remember_hidden_pads`]). At the first round the worker
//! also looks up the remembered pads' addresses once, for entries saved
//! before addresses were kept.
//!
//! Every change is recorded in `padtokbm.hidhide.json` beside the config
//! (deleted when nothing is left to undo), so a crash is undone at the next
//! start. Foreign entries are never dropped or reordered; compares are
//! case-insensitive, as the driver's.

use crate::config::KnownPad;
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
    /// HID instance ids of pads the app hides (any kind with a native
    /// driver) present at the last round; empty while nothing is wanted.
    /// The settings' 記住的手把 marks these 連線中.
    pub present: Vec<String>,
    /// The Bluetooth addresses (`BTH_ADDR`) of the pads hidden while present
    /// ([`Hider::replug_ids`]) that have one, filled in by the worker. The
    /// engine powers these off, and only these.
    pub replug_addrs: Vec<(ControllerKind, u64)>,
    /// The Bluetooth addresses of the `present` pads that have one, filled
    /// in by the worker: the engine sees a pad it powered off come back.
    pub present_addrs: Vec<(ControllerKind, u64)>,
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
    /// Ids added while present and present since: a game may hold them open
    /// (their kinds are `HideStatus::needs_replug`).
    replug_ids: Vec<String>,
    /// The config's remembered ids (`hidhide_known`).
    known: Vec<String>,
    /// Learned this session, not in `known` yet (the config write is on its
    /// way, or failed): treated as remembered.
    pending: Vec<String>,
    /// Ids that left `known` and are not yet out of the blacklist.
    forgotten: Vec<String>,
    /// Learned, not yet taken by [`Hider::take_learned`].
    learned: Vec<KnownPad>,
    /// Pads present at the last round (`HideStatus::present`).
    present: Vec<String>,
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
            replug_ids: Vec::new(),
            known: Vec::new(),
            pending: Vec::new(),
            forgotten: Vec::new(),
            learned: Vec::new(),
            present: Vec::new(),
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
            needs_replug: ControllerKind::ALL
                .into_iter()
                .filter(|k| self.replug_ids.iter().any(|id| kind_of(id) == Some(*k)))
                .collect(),
            ok: self.ok,
            present: self.present.clone(),
            replug_addrs: Vec::new(),
            present_addrs: Vec::new(),
        }
    }

    /// The ids hidden while present and present at every round since: a
    /// game may hold these open. Another pad of the same kind hidden earlier
    /// is not among them. An id leaves when a round no longer finds it
    /// (powered off, unplugged) or its kind is no longer wanted.
    pub fn replug_ids(&self) -> Vec<String> {
        self.replug_ids.clone()
    }

    /// The config's remembered ids, as last set.
    pub fn known(&self) -> &[String] {
        &self.known
    }

    /// The kinds wanted hidden (only kinds with a native driver count). A
    /// kind no longer wanted no longer needs a replug. (The engine's
    /// connected kinds only start a round: whether a pad left is what the
    /// next round finds present.)
    pub fn set_inputs(&mut self, wanted: &[ControllerKind]) {
        let wanted: Vec<ControllerKind> = ControllerKind::ALL
            .into_iter()
            .filter(|k| k.has_native_driver() && wanted.contains(k))
            .collect();
        self.replug_ids.retain(|id| kind_of(id).is_some_and(|k| wanted.contains(&k)));
        self.wanted = wanted;
    }

    /// The config's remembered ids (`hidhide_known`). An id that left the
    /// list is taken out of the blacklist at the next round unless its pad
    /// is present and wanted.
    pub fn set_known(&mut self, ids: &[String]) {
        let gone: Vec<String> = self.known.iter().filter(|id| !contains_ci(ids, id)).cloned().collect();
        self.forgotten = merge(&unmerge(&self.forgotten, ids), &gone);
        self.pending.retain(|id| !contains_ci(ids, id));
        self.known = ids.to_vec();
    }

    /// Pads learned since the last call: present, of a wanted kind, hidden
    /// by us, not remembered yet. Each is reported once.
    pub fn take_learned(&mut self) -> Vec<KnownPad> {
        std::mem::take(&mut self.learned)
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
        self.present = merge(&[], &present.iter().filter(|id| kind_of(id).is_some()).cloned().collect::<Vec<_>>());
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

        // 2. Blacklist: + present and remembered ids of wanted kinds,
        //    − ours of unwanted kinds and ours forgotten while absent.
        let black = ctl.blacklist()?;
        let wanted = &self.wanted;
        let is_wanted = |id: &String| kind_of(id).is_some_and(|k| wanted.contains(&k));
        let present_wanted: Vec<String> = merge(&[], &present.iter().filter(|id| is_wanted(id)).cloned().collect::<Vec<_>>());
        let remembered = merge(&self.known, &self.pending);
        let to_hide = merge(&present_wanted, &remembered.iter().filter(|id| is_wanted(id)).cloned().collect::<Vec<_>>());
        let added: Vec<String> = to_hide.iter().filter(|id| !contains_ci(&black, id)).cloned().collect();
        let dropped: Vec<String> = self
            .state
            .ids
            .iter()
            .filter(|id| !is_wanted(id) || (contains_ci(&self.forgotten, id) && !contains_ci(&to_hide, id)))
            .cloned()
            .collect();
        let next = merge(&unmerge(&black, &dropped), &added);
        if next != black {
            ctl.set_blacklist(&next)?;
        }
        if !added.is_empty() || !dropped.is_empty() {
            self.state.ids = merge(&unmerge(&self.state.ids, &dropped), &added);
            self.save();
        }
        self.forgotten.clear();
        // Only a pad present when hidden may be held open by a game.
        let held: Vec<String> = added.iter().filter(|id| contains_ci(present, id)).cloned().collect();
        self.replug_ids = merge(&self.replug_ids, &held);
        // Learn: present, ours, not remembered yet.
        for id in &present_wanted {
            if contains_ci(&self.state.ids, id) && !contains_ci(&remembered, id) {
                if let Some(kind) = kind_of(id) {
                    self.pending.push(id.clone());
                    self.learned.push(KnownPad { kind: kind.slug().to_string(), id: id.clone(), addr: None });
                }
            }
        }
        // An id no longer present has gone (off, unplugged): a game lost it.
        self.replug_ids.retain(|id| contains_ci(present, id));

        // 3. The driver hides nothing while Active is off.
        if !self.state.ids.is_empty() && !ctl.active()? {
            ctl.set_active(true)?;
            self.state.activated_by_us = true;
            self.save();
        }
        Ok(())
    }

    fn release_inner<C: Control>(&mut self, open: impl FnOnce() -> Result<C, Error>) -> Result<(), Error> {
        self.replug_ids.clear();
        self.present.clear();
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
    /// The Bluetooth address of the pad with HID instance id `id`, 12
    /// upper-case hex digits (`hidhide::bt_addr_of_instance`); `None`: USB,
    /// or not found.
    fn bt_addr(&mut self, id: &str) -> Option<String>;
}

enum Msg {
    Update { wanted: Vec<ControllerKind> },
    Known(Vec<String>),
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
    /// The kinds wanted hidden or the engine's connected kinds changed: a
    /// round runs. `connected` is not decided on: a pad that connected or
    /// left is what the round finds present or not.
    pub fn update(&self, wanted: Vec<ControllerKind>, connected: Vec<ControllerKind>) {
        drop(connected);
        let _ = self.tx.send(Msg::Update { wanted });
    }

    /// The config's remembered ids (`hidhide_known`) changed. Runs a round
    /// once the first [`HiderLink::update`] has come.
    pub fn set_known(&self, ids: Vec<String>) {
        let _ = self.tx.send(Msg::Known(ids));
    }

    /// Kinds that need one reconnect, after a round that fully succeeded
    /// (empty otherwise). The engine powers such a pad off once
    /// (`engine::run_loop`).
    pub fn needs_replug(&self) -> Vec<ControllerKind> {
        let st = self.status.read().unwrap_or_else(|p| p.into_inner());
        if st.supported && st.ok { st.needs_replug.clone() } else { Vec::new() }
    }

    /// The Bluetooth addresses of the pads behind [`HiderLink::needs_replug`]
    /// ([`HideStatus::replug_addrs`]), after a round that fully succeeded
    /// (empty otherwise). The engine powers off these, never every pad of
    /// the kind.
    pub fn replug_addrs(&self) -> Vec<(ControllerKind, u64)> {
        let st = self.status.read().unwrap_or_else(|p| p.into_inner());
        if st.supported && st.ok { st.replug_addrs.clone() } else { Vec::new() }
    }

    /// The Bluetooth addresses of the pads present at the last round
    /// ([`HideStatus::present_addrs`]).
    pub fn present_addrs(&self) -> Vec<(ControllerKind, u64)> {
        self.status.read().unwrap_or_else(|p| p.into_inner()).present_addrs.clone()
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
    /// worker thread) whenever the status changes; `on_learn` with the pads
    /// a round learned ([`Hider::take_learned`]).
    pub fn spawn<B: Backend>(
        state_path: PathBuf,
        backend: B,
        retry_every: Duration,
        on_change: Box<dyn Fn() + Send>,
        on_learn: Box<dyn Fn(Vec<KnownPad>) + Send>,
    ) -> std::io::Result<HiderWorker> {
        let hider = Hider::new(state_path);
        let status = Arc::new(RwLock::new(hider.status()));
        let (tx, rx) = unbounded();
        let st = status.clone();
        let thread = thread::Builder::new()
            .name("hidhide".into())
            .spawn(move || run_worker(hider, backend, rx, st, retry_every, on_change, on_learn))?;
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
    on_learn: Box<dyn Fn(Vec<KnownPad>) + Send>,
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
    // The remembered pads' addresses are looked up once, after the first round.
    let mut looked_up_known = false;
    // Bluetooth addresses looked up this launch (`None`: has none).
    let mut addrs: Vec<(String, Option<String>)> = Vec::new();
    let mut addr_of = |backend: &mut B, id: &str| -> Option<String> {
        if let Some((_, a)) = addrs.iter().find(|(i, _)| i.eq_ignore_ascii_case(id)) {
            return a.clone();
        }
        let a = backend.bt_addr(id);
        addrs.push((id.to_string(), a.clone()));
        a
    };
    // The present ids the last round saw: while something is wanted, the
    // worker re-reads them every `retry_every` and runs a round when they
    // changed (a second pad of a connected kind turned on, a pad powered
    // off), opening the driver only then.
    let mut last_present: Vec<String> = Vec::new();
    loop {
        let (mut run, mut poll) = (false, false);
        let first = if started && (!hider.ok() || hider.wants_any()) {
            match rx.recv_timeout(retry_every) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => {
                    if hider.ok() {
                        poll = true;
                    } else {
                        run = true;
                    }
                    None
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // Every link is gone: Xbox output has ended.
                    release_before_exit(&mut hider, &mut backend);
                    return;
                }
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => {
                    release_before_exit(&mut hider, &mut backend);
                    return;
                }
            }
        };
        let mut stop = None;
        for msg in first.into_iter().chain(rx.try_iter()) {
            match msg {
                Msg::Update { wanted } => {
                    hider.set_inputs(&wanted);
                    started = true;
                    run = true;
                }
                Msg::Known(ids) => {
                    hider.set_known(&ids);
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
        let mut polled = None;
        if poll && !run && hider.wants_any() {
            let now = backend.present();
            if !same_ids(&now, &last_present) {
                run = true;
                polled = Some(now);
            }
        }
        if run && started {
            let (present, own) = if hider.wants_any() {
                (polled.unwrap_or_else(|| backend.present()), backend.own_path())
            } else {
                (Vec::new(), None)
            };
            last_present = present.clone();
            let _ = hider.round(|| backend.open(), &present, own.as_deref());
            let mut learned = hider.take_learned();
            for k in &mut learned {
                k.addr = addr_of(&mut backend, &k.id);
            }
            if !looked_up_known {
                looked_up_known = true;
                // Entries saved before addresses were kept get theirs.
                for id in hider.known().to_vec() {
                    let Some(kind) = kind_of(&id) else { continue };
                    if learned.iter().any(|k| k.id.eq_ignore_ascii_case(&id)) {
                        continue;
                    }
                    if let Some(a) = addr_of(&mut backend, &id) {
                        learned.push(KnownPad { kind: kind.slug().to_string(), id, addr: Some(a) });
                    }
                }
            }
            if !learned.is_empty() {
                on_learn(learned);
            }
        }
        let mut st = hider.status();
        let mut addrs_of = |ids: &[String]| -> Vec<(ControllerKind, u64)> {
            ids.iter()
                .filter_map(|id| Some((kind_of(id)?, hidhide::bt_addr_value(&addr_of(&mut backend, id)?)?)))
                .collect()
        };
        st.replug_addrs = addrs_of(&hider.replug_ids());
        st.present_addrs = addrs_of(&st.present);
        publish(st);
    }
}

/// The same ids, in any order and case.
fn same_ids(a: &[String], b: &[String]) -> bool {
    let norm = |v: &[String]| {
        let mut v: Vec<String> = v.iter().map(|x| x.to_ascii_lowercase()).collect();
        v.sort();
        v.dedup();
        v
    };
    norm(a) == norm(b)
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
    fn bt_addr(&mut self, id: &str) -> Option<String> {
        hidhide::bt_addr_of_instance(id)
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

/// Save pads the hider learned to `hidhide_known` in the config at
/// `config_path` and the live config, under the config file lock that every
/// IPC mutator takes too (`config_io::file_lock`), so neither overwrites the
/// other's change. At most [`crate::config::HIDHIDE_KNOWN_MAX`], the oldest
/// dropped; a listed pad without an address gets the one `found` carries.
/// Returns whether it wrote (`false`: nothing new).
pub fn remember_hidden_pads(engine: &Handle, config_path: &Path, found: &[KnownPad]) -> anyhow::Result<bool> {
    let _file = crate::config_io::file_lock();
    let mut doc = crate::config_io::ConfigDoc::load(config_path)?;
    let before = &doc.typed().hidhide_known;
    let next = crate::config::remember_known(before, found);
    if next == *before {
        return Ok(false);
    }
    doc.set_hidhide_known(next.clone());
    crate::config_io::write_atomic(config_path, &doc)?;
    engine.config_write().hidhide_known = next;
    Ok(true)
}

/// The worker's `on_learn` for an engine whose config lives at
/// `config_path`: [`remember_hidden_pads`], then `config-changed` through
/// the engine's event bridge. Holds the engine weakly.
pub fn learn_saver(engine: &Handle, config_path: &Path) -> Box<dyn Fn(Vec<KnownPad>) + Send> {
    let weak = engine.downgrade();
    let path = config_path.to_path_buf();
    Box::new(move |found: Vec<KnownPad>| {
        let Some(engine) = weak.upgrade() else { return };
        match remember_hidden_pads(&engine, &path, &found) {
            Ok(true) => {
                tracing::info!(pads = found.len(), "HidHide: remembered for the next start");
                engine.announce_config_change("hidhide");
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("HidHide: could not remember the hidden pad in {}: {e:#}", path.display()),
        }
    })
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
    let learn = learn_saver(engine, config_path);
    match HiderWorker::spawn(path, RealBackend::new(), RETRY_EVERY, Box::new(engine.status_notifier()), learn) {
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
        r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        let calls = r.ctl.f().calls.clone();
        let wl = calls.iter().position(|c| *c == "set_wl").expect("whitelist written");
        let bl = calls.iter().position(|c| *c == "set_bl").expect("blacklist written");
        assert!(wl < bl, "{calls:?}");
    }

    #[test]
    fn a_failed_whitelist_blocks_the_blacklist() {
        let mut r = rig(Fake { fail_set_whitelist: true, ..Default::default() });
        r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[DS]);
        let ctl = r.ctl.clone();
        assert!(r.hider.round(ctl.open(), &s(&[DS_USB]), None).is_err());
        assert!(r.ctl.f().black.is_empty());
        assert_eq!(r.hider.status().hidden(DS), Some(false));
    }

    #[test]
    fn inverse_mode_skips_the_whitelist() {
        let mut r = rig(Fake { inverse: true, white: s(&[FOREIGN_APP]), ..Default::default() });
        r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().white, s(&[&OWN.to_lowercase()]), "case-insensitive: no duplicate");
        assert_eq!(r.hider.state().whitelisted, None);
        release(&mut r).unwrap();
        assert_eq!(r.ctl.f().white, s(&[&OWN.to_lowercase()]), "the user's entry stays");
    }

    #[test]
    fn an_id_already_blacklisted_by_the_user_is_not_recorded_or_removed() {
        let mut r = rig(Fake { black: s(&[&DS_USB.to_lowercase()]), active: true, ..Default::default() });
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[&DS_USB.to_lowercase()]));
        assert!(r.hider.state().ids.is_empty());
        assert_eq!(r.hider.status().hidden(DS), Some(true), "already hidden: no replug");
        r.hider.set_inputs(&[]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[&DS_USB.to_lowercase()]), "not ours");
        assert!(r.ctl.f().active, "not turned on by us");
    }

    #[test]
    fn active_already_on_is_not_ours() {
        let mut r = rig(Fake { active: true, ..Default::default() });
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert!(!r.hider.state().activated_by_us);
        release(&mut r).unwrap();
        assert!(r.ctl.f().active, "the user turned it on");
    }

    // ── needs replug ──

    #[test]
    fn hiding_a_present_pad_needs_a_replug_until_it_leaves() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false), "a game may hold it open");
        // Still present: still needs it, whether or not the engine has it
        // (the ⏻ on a USB pad drops only the app's handle).
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        // Gone (powered off, unplugged): a game lost it, cleared.
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        // And back: the id is already blacklisted, so it arrives hidden.
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
    }

    #[test]
    fn needs_replug_clears_when_the_pad_is_no_longer_present() {
        // Hidden while present but not (yet) connected in the engine; then
        // it goes away without ever connecting.
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_BT]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
    }

    #[test]
    fn needs_replug_is_per_kind() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS, NSP]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(false));
        assert_eq!(r.hider.status().hidden(NSP), Some(true), "nothing of it was present");
    }

    #[test]
    fn disconnect_keeps_the_entries() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]), "a reconnect is hidden before Windows hands it to the game");
        assert_eq!(r.hider.state().ids, s(&[DS_USB]));
    }

    // ── remembered pads (`hidhide_known`) ──

    fn known(kind: &str, id: &str) -> KnownPad {
        KnownPad { kind: kind.into(), id: id.into(), addr: None }
    }

    /// A second Bluetooth DualSense (the maintainer owns two).
    const DS_BT2: &str = r"HID\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\9&77&0&0000";
    const ADDR_A: &str = "7C66EF8223D2";
    const ADDR_B: &str = "A0B1C2D3E4F5";

    #[test]
    fn only_the_ids_hidden_while_present_need_the_replug() {
        // A is hidden already (an earlier round); B is turned on now.
        let mut r = rig(Fake { black: s(&[DS_BT]), active: true, ..Default::default() });
        r.hider.set_known(&s(&[DS_BT]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_BT, DS_BT2]).unwrap();
        assert_eq!(r.hider.status().needs_replug, vec![DS]);
        assert_eq!(r.hider.replug_ids(), s(&[DS_BT2]), "A was hidden before: not B's replug");
        // B goes (powered off), A stays: B no longer needs it, and the kind
        // no longer needs a replug although A is still there.
        round(&mut r, &[DS_BT]).unwrap();
        assert!(r.hider.replug_ids().is_empty());
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        // B back on: already hidden, no replug.
        round(&mut r, &[DS_BT, DS_BT2]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        // Nothing wanted: none.
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_BT2]).unwrap();
        assert_eq!(r.hider.replug_ids(), s(&[DS_BT2]));
        r.hider.set_inputs(&[]);
        round(&mut r, &[DS_BT2]).unwrap();
        assert!(r.hider.replug_ids().is_empty());
    }

    #[test]
    fn a_remembered_absent_pad_is_hidden_at_start_without_a_replug() {
        let mut r = rig(foreign());
        r.hider.set_known(&s(&[DS_BT]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, DS_BT]), "hidden before it is turned on");
        assert_eq!(r.hider.state().ids, s(&[DS_BT]), "recorded: undone like any other entry");
        assert_eq!(r.hider.status().hidden(DS), Some(true), "not present when hidden: no replug");
        assert!(r.hider.take_learned().is_empty(), "already remembered");
        // Turned on later: it arrives hidden.
        r.ctl.f().calls.clear();
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[MOUSE, DS_BT]).unwrap();
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        assert!(!r.ctl.f().calls.contains(&"set_bl"), "nothing to change");
        release(&mut r).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV]), "exit undoes it");
    }

    #[test]
    fn a_present_pad_we_hide_is_learned_once() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB, MOUSE]).unwrap();
        assert_eq!(r.hider.take_learned(), vec![known("dualsense", DS_USB)]);
        round(&mut r, &[DS_USB, MOUSE]).unwrap();
        assert!(r.hider.take_learned().is_empty(), "reported once");
        // Until the config has it, it counts as remembered: it stays hidden
        // after it leaves.
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]));
        // The config caught up; it is no longer pending.
        r.hider.set_known(&s(&[DS_USB]));
        round(&mut r, &[MOUSE]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]));
    }

    #[test]
    fn an_id_we_added_earlier_is_learned_but_a_users_entry_is_not() {
        // Ours from a crashed run, present now: remembered.
        let mut r = rig(Fake { black: s(&[DS_USB]), active: true, ..Default::default() });
        HideState { ids: s(&[DS_USB]), ..Default::default() }.save(&r.path).unwrap();
        r.hider = Hider::new(r.path.clone());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.hider.take_learned(), vec![known("dualsense", DS_USB)]);
        // The user's own entry: not ours, not remembered.
        let mut r = rig(Fake { black: s(&[DS_USB]), active: true, ..Default::default() });
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert!(r.hider.take_learned().is_empty());
    }

    #[test]
    fn remembered_ids_of_a_kind_no_longer_wanted_are_removed() {
        let mut r = rig(foreign());
        r.hider.set_known(&s(&[DS_BT, NSP_BT]));
        r.hider.set_inputs(&[DS, NSP]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, DS_BT, NSP_BT]));
        r.hider.set_inputs(&[NSP]); // DualSense on 手把
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, NSP_BT]));
        assert_eq!(r.hider.state().ids, s(&[NSP_BT]));
    }

    #[test]
    fn forgetting_an_absent_pad_removes_its_entry() {
        let mut r = rig(foreign());
        r.hider.set_known(&s(&[DS_BT, DS_USB]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, DS_BT, DS_USB]));
        r.hider.set_known(&s(&[DS_USB]));
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, DS_USB]));
        assert_eq!(r.hider.state().ids, s(&[DS_USB]));
        r.hider.set_known(&[]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV]));
        assert!(r.hider.state().ids.is_empty());
    }

    #[test]
    fn forgetting_a_present_pad_keeps_it_hidden_and_learns_it_again() {
        let mut r = rig(Fake::default());
        r.hider.set_known(&s(&[DS_USB]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        r.hider.set_known(&[]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]), "present and wanted: still hidden");
        assert_eq!(r.hider.take_learned(), vec![known("dualsense", DS_USB)], "and remembered again");
    }

    #[test]
    fn a_remembered_absent_id_added_beside_a_present_hidden_pad_needs_no_replug() {
        // DS_USB is ours and blacklisted already (an earlier run); DS_BT is
        // remembered, off, and added this round. Only an id added while
        // present may be held open by a game.
        let mut r = rig(Fake { black: s(&[DS_USB]), active: true, ..Default::default() });
        HideState { ids: s(&[DS_USB]), ..Default::default() }.save(&r.path).unwrap();
        r.hider = Hider::new(r.path.clone());
        r.hider.set_known(&s(&[DS_USB, DS_BT]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB, DS_BT]));
        assert_eq!(r.hider.status().hidden(DS), Some(true));
    }

    #[test]
    fn the_status_lists_the_present_pads() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[MOUSE, DS_USB, NSP_BT]).unwrap();
        assert_eq!(r.hider.status().present, s(&[DS_USB, NSP_BT]), "pads the app hides, any kind; no mouse");
        r.hider.set_inputs(&[]);
        round(&mut r, &[DS_USB]).unwrap();
        assert!(r.hider.status().present.is_empty(), "nothing wanted: not looked up");
    }

    #[test]
    fn remembered_ids_compare_case_insensitively() {
        let mut r = rig(Fake::default());
        r.hider.set_known(&s(&[&DS_USB.to_lowercase()]));
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().black.len(), 1);
        r.ctl.f().calls.clear();
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black.len(), 1, "no duplicate");
        assert!(!r.ctl.f().calls.contains(&"set_bl"));
        assert_eq!(r.hider.status().hidden(DS), Some(true));
        assert!(r.hider.take_learned().is_empty(), "already remembered");
        // Forgetting by another case removes it.
        r.hider.set_known(&[]);
        round(&mut r, &[]).unwrap();
        assert!(r.ctl.f().black.is_empty());
    }

    // ── mode changes and cleanup ──

    #[test]
    fn leaving_xbox_removes_that_kinds_entries_only() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS, NSP]);
        round(&mut r, &[DS_USB, NSP_BT]).unwrap();
        r.hider.set_inputs(&[NSP]);
        round(&mut r, &[DS_USB, NSP_BT]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[FOREIGN_DEV, NSP_BT]));
        assert_eq!(r.hider.state().ids, s(&[NSP_BT]));
        assert_eq!(r.hider.status().hidden(DS), None, "not wanted");
    }

    #[test]
    fn nothing_wanted_restores_the_original_lists_and_active() {
        let mut r = rig(foreign());
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[DS_USB, DS_BT]).unwrap();
        r.hider.set_inputs(&[]);
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
        r.hider.set_inputs(&[DS, NSP]);
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
        r.hider.set_inputs(&[]);
        round(&mut r, &[DS_USB]).unwrap();
        assert_eq!(r.ctl.f().opens, 0);
        assert!(r.hider.ok());
    }

    #[test]
    fn a_failed_release_keeps_the_state_for_the_next_start() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[NSP]);
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
        r.hider.set_inputs(&[]);
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
        r.hider.set_inputs(&[DS]);
        round(&mut r, &[]).unwrap();
        assert_eq!(r.ctl.f().white, s(&[FOREIGN_APP, OWN]));
        assert_eq!(r.hider.state().whitelisted.as_deref(), Some(OWN));
    }

    // ── driver unavailable ──

    #[test]
    fn busy_or_absent_reports_not_hidden_and_the_next_round_retries() {
        for err in [OpenErr::Busy, OpenErr::Absent] {
            let mut r = rig(Fake { open_err: Some(err), ..Default::default() });
            r.hider.set_inputs(&[DS]);
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
        r.hider.set_inputs(&[DS]);
        assert_eq!(r.hider.status().hidden(DS), Some(false));
    }

    // ── Switch 2 Pro ──

    #[test]
    fn the_switch_2_pro_is_never_hidden() {
        let mut r = rig(Fake::default());
        r.hider.set_inputs(&[NS2]);
        round(&mut r, &[NS2_BLE]).unwrap();
        assert!(r.ctl.f().black.is_empty());
        assert_eq!(r.ctl.f().opens, 0, "nothing wanted that Windows drives natively");
        assert_eq!(r.hider.status().hidden(NS2), None);
        r.hider.set_inputs(&[DS, NS2]);
        round(&mut r, &[NS2_BLE, DS_USB]).unwrap();
        assert_eq!(r.ctl.f().black, s(&[DS_USB]));
        assert_eq!(r.hider.status().hidden(NS2), None);
    }

    #[test]
    fn status_hidden_is_none_without_a_worker() {
        let st = HideStatus { supported: false, wanted: vec![DS], needs_replug: vec![], ok: true, present: vec![], replug_addrs: vec![], present_addrs: vec![] };
        assert_eq!(st.hidden(DS), None);
    }

    // ── worker ──

    struct FakeBackend {
        ctl: FakeCtl,
        present: Arc<Mutex<Vec<String>>>,
        /// (instance id, Bluetooth address) the look-up finds.
        addrs: Arc<Mutex<Vec<(String, String)>>>,
        lookups: Arc<std::sync::atomic::AtomicUsize>,
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
        fn bt_addr(&mut self, id: &str) -> Option<String> {
            self.lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.addrs.lock().unwrap().iter().find(|(i, _)| i.eq_ignore_ascii_case(id)).map(|(_, a)| a.clone())
        }
    }

    struct WRig {
        dir: tempfile::TempDir,
        ctl: FakeCtl,
        present: Arc<Mutex<Vec<String>>>,
        changes: Arc<std::sync::atomic::AtomicUsize>,
        learned: Arc<Mutex<Vec<KnownPad>>>,
        addrs: Arc<Mutex<Vec<(String, String)>>>,
        lookups: Arc<std::sync::atomic::AtomicUsize>,
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
        let learned = Arc::new(Mutex::new(Vec::new()));
        let l = learned.clone();
        let addrs = Arc::new(Mutex::new(Vec::new()));
        let lookups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker = HiderWorker::spawn(
            path,
            FakeBackend { ctl: ctl.clone(), present: present.clone(), addrs: addrs.clone(), lookups: lookups.clone() },
            Duration::from_millis(20),
            Box::new(move || {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            Box::new(move |found| l.lock().unwrap().extend(found)),
        )
        .unwrap();
        WRig { dir, ctl, present, changes, learned, addrs, lookups, worker }
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
        // Unplugged: the round no longer finds it.
        w.present.lock().unwrap().clear();
        link.update(vec![DS], vec![]);
        eventually("left: hidden", || link.hidden(DS) == Some(true));
    }

    #[test]
    fn worker_reports_learned_pads_and_applies_the_remembered_list() {
        let w = wrig(foreign(), &[DS_USB], None);
        let link = w.worker.link();
        link.set_known(s(&[DS_BT]));
        thread::sleep(Duration::from_millis(60));
        assert_eq!(w.ctl.f().opens, 0, "the remembered list alone starts nothing");
        link.update(vec![DS], vec![DS]);
        eventually("both hidden", || w.ctl.f().black == s(&[FOREIGN_DEV, DS_USB, DS_BT]));
        eventually("learned", || *w.learned.lock().unwrap() == vec![known("dualsense", DS_USB)]);
        // The config now has both; then the user forgets the absent one.
        link.set_known(s(&[DS_BT, DS_USB]));
        link.set_known(s(&[DS_USB]));
        eventually("forgotten: removed", || w.ctl.f().black == s(&[FOREIGN_DEV, DS_USB]));
        assert_eq!(w.learned.lock().unwrap().len(), 1, "reported once");
    }

    fn set_addrs(w: &WRig, list: &[(&str, &str)]) {
        *w.addrs.lock().unwrap() = list.iter().map(|(i, a)| (i.to_string(), a.to_string())).collect();
    }

    #[test]
    fn worker_learns_a_pad_with_its_bluetooth_address() {
        let w = wrig(Fake::default(), &[DS_BT2, DS_USB], None);
        set_addrs(&w, &[(DS_BT2, ADDR_B)]);
        w.worker.link().update(vec![DS], vec![DS]);
        eventually("learned", || w.learned.lock().unwrap().len() == 2);
        let got = w.learned.lock().unwrap().clone();
        assert!(got.contains(&KnownPad { kind: "dualsense".into(), id: DS_BT2.into(), addr: Some(ADDR_B.into()) }), "{got:?}");
        assert!(got.contains(&known("dualsense", DS_USB)), "USB: no address; {got:?}");
    }

    #[test]
    fn worker_looks_up_the_remembered_pads_addresses_once_at_start() {
        // Remembered before addresses were kept: the first round reports the
        // ones it can find (absent pads too: the devnode stays).
        let w = wrig(Fake::default(), &[], None);
        set_addrs(&w, &[(DS_BT, ADDR_A)]);
        let link = w.worker.link();
        link.set_known(s(&[DS_BT, DS_USB]));
        link.update(vec![DS], vec![]);
        eventually("reported", || !w.learned.lock().unwrap().is_empty());
        thread::sleep(Duration::from_millis(60));
        assert_eq!(*w.learned.lock().unwrap(), vec![KnownPad { kind: "dualsense".into(), id: DS_BT.into(), addr: Some(ADDR_A.into()) }]);
        let n = w.lookups.load(std::sync::atomic::Ordering::SeqCst);
        link.update(vec![DS], vec![DS]);
        link.set_known(s(&[DS_BT]));
        eventually("rounds ran", || w.ctl.f().opens >= 2);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(w.learned.lock().unwrap().len(), 1, "once per launch");
        assert_eq!(w.lookups.load(std::sync::atomic::Ordering::SeqCst), n, "no more look-ups");
    }

    #[test]
    fn worker_reports_the_addresses_of_the_pads_hidden_while_present() {
        // A hidden already, B turned on now; both Bluetooth.
        let w = wrig(Fake { black: s(&[DS_BT]), active: true, ..Default::default() }, &[DS_BT, DS_BT2], None);
        set_addrs(&w, &[(DS_BT, ADDR_A), (DS_BT2, ADDR_B)]);
        let link = w.worker.link();
        link.update(vec![DS], vec![DS]);
        eventually("needs replug", || link.hidden(DS) == Some(false));
        assert_eq!(link.replug_addrs(), vec![(DS, 0xA0B1_C2D3_E4F5)], "B only");
        // A USB pad hidden while present: no address.
        let w = wrig(Fake::default(), &[DS_USB], None);
        let link = w.worker.link();
        link.update(vec![DS], vec![DS]);
        eventually("needs replug", || link.hidden(DS) == Some(false));
        assert!(link.replug_addrs().is_empty());
    }

    #[test]
    fn worker_runs_a_round_when_the_present_pads_change() {
        // A is hidden and connected; B, never seen, is turned on: no engine
        // update comes (the connected kinds do not change).
        let w = wrig(Fake { black: s(&[DS_BT]), active: true, ..Default::default() }, &[DS_BT], None);
        let link = w.worker.link();
        link.update(vec![DS], vec![DS]);
        eventually("first round", || w.ctl.f().opens == 1);
        eventually("A arrived hidden", || link.hidden(DS) == Some(true));
        thread::sleep(Duration::from_millis(150));
        assert_eq!(w.ctl.f().opens, 1, "same pads: no round, the driver is not opened");
        w.present.lock().unwrap().push(DS_BT2.into());
        eventually("B hidden without an update", || w.ctl.f().black == s(&[DS_BT, DS_BT2]));
        eventually("B needs the replug", || link.hidden(DS) == Some(false));
        // B goes (powered off): seen without an update too.
        *w.present.lock().unwrap() = s(&[DS_BT]);
        eventually("B gone: no replug", || link.hidden(DS) == Some(true));
        let opens = w.ctl.f().opens;
        thread::sleep(Duration::from_millis(150));
        assert_eq!(w.ctl.f().opens, opens, "no round while nothing changes");
    }

    #[test]
    fn worker_reports_the_present_pads_addresses() {
        let w = wrig(Fake::default(), &[DS_BT, DS_USB, MOUSE], None);
        set_addrs(&w, &[(DS_BT, ADDR_A)]);
        let link = w.worker.link();
        link.update(vec![DS], vec![]);
        eventually("a round ran", || link.status().present.len() == 2);
        assert_eq!(link.status().present_addrs, vec![(DS, 0x7C66_EF82_23D2)], "USB: none");
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
