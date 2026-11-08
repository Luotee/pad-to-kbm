// Board state, fed from IPC. The store never edits the config itself: every
// change goes through a command, and `config-changed` re-reads it (spec 9).

import { invoke, listen } from '../ipc.js';
import { PADS } from './model.js';

export const state = {
  config: null,
  connected: new Set(),
  offPads: new Set(), // pads switched off for this session (header 關; runtime only)
  warnings: [],       // controller status warnings (e.g. vigem_missing)
  pad: 'dualsense',
  sel: null,          // slot waiting for a key (capture)
  picked: null,       // palette tile picked by click, waiting for a slot
  held: {},           // slotKey -> true while its pad button is held
  hot: {},            // tile -> true while lit
  note: '',
  mpage: 0,
  medit: null,        // macro editor draft
  mpop: null,         // open delay popover (step index)
  setOpen: false,
  setTab: 'gen',
  draft: null,        // settings draft
  setErr: '',
  canUndo: false,
  canRedo: false,
  prefs: {},
  version: '',
  configPath: '',
};

let renderFn = () => {};
const padChangeHooks = [];
export function onRender(f) { renderFn = f; }
export function rerender() { renderFn(); }
export function onPadChange(f) { padChangeHooks.push(f); }

// The connected pads from a ControllerStatus (get_controller_status) or a
// controller-status event payload.
export function connectedFrom(s) {
  if (!s) return new Set();
  if (Array.isArray(s.connected_pads)) return new Set(s.connected_pads);
  if (Array.isArray(s.connected)) return new Set(s.connected);
  return new Set(s.connected !== false && s.kind ? [s.kind] : []);
}

// The pads switched off from a status / event payload (`off_pads`); a payload
// without the field leaves the current set alone.
export function offFrom(s, cur = new Set()) {
  return s && Array.isArray(s.off_pads) ? new Set(s.off_pads) : cur;
}

export async function setPad(pad, { save = true } = {}) {
  if (!PADS.includes(pad)) return;
  const changed = pad !== state.pad;
  state.pad = pad;
  state.sel = null;
  state.picked = null;
  state.held = {};
  if (changed) padChangeHooks.forEach(f => f(pad));
  if (save && state.prefs.last_pad !== pad) {
    state.prefs = { ...state.prefs, last_pad: pad };
    try { await invoke('set_ui_prefs', { prefs: { last_pad: pad } }); } catch (e) { console.warn('set_ui_prefs failed', e); }
  }
}

export async function reloadConfig() {
  state.config = await invoke('get_config');
}

export async function init() {
  const [config, status, prefs, version, configPath] = await Promise.all([
    invoke('get_config'),
    invoke('get_controller_status').catch(() => null),
    invoke('get_ui_prefs').catch(() => ({})),
    invoke('get_app_version').catch(() => ''),
    invoke('get_config_path').catch(() => ''),
  ]);
  state.config = config;
  state.connected = connectedFrom(status);
  state.offPads = offFrom(status);
  state.warnings = (status && status.warnings) || [];
  state.prefs = prefs || {};
  state.version = version || '';
  state.configPath = configPath || '';
  const owner = status && status.kind;
  state.pad = PADS.includes(state.prefs.last_pad) ? state.prefs.last_pad : PADS.includes(owner) ? owner : 'dualsense';
  if (owner && PADS.includes(owner) && state.connected.has(owner) && !state.connected.has(state.pad)) state.pad = owner;

  await listen('config-changed', async () => {
    await reloadConfig();
    rerender();
  });
  await listen('controller-status', async s => {
    const before = state.connected;
    state.connected = connectedFrom(s);
    state.offPads = offFrom(s, state.offPads);
    state.warnings = (s && s.warnings) || [];
    const fresh = [...state.connected].filter(p => !before.has(p));
    if (fresh.length) await setPad(fresh[0]);   // a pad that connects is auto-selected (spec 4.5)
    rerender();
  });
}
