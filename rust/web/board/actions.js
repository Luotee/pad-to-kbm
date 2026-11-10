// One IPC call per user action (spec 9). Board binding changes are recorded
// for 復原／重做 only after the backend accepted them.

import { invoke } from '../ipc.js';
import { state, rerender, setPad } from './store.js';
import { createHistory } from './history.js';
import { slotRef, slotLabel, entryForTile } from './model.js';

const hist = createHistory();
const clone = v => JSON.parse(JSON.stringify(v));

function syncHist() { state.canUndo = hist.canUndo; state.canRedo = hist.canRedo; }
function errText(e) { return String(e && e.message ? e.message : e); }

async function boardChange(cmd, args, note = '') {
  const pad = state.pad, before = clone(state.config.pads[pad]);
  state.sel = null;
  state.picked = null;
  try {
    await invoke(cmd, args);
    hist.record(pad, before);
    state.note = note;
  } catch (e) {
    state.note = '無法套用：' + errText(e);
  }
  syncHist();
  rerender();
}

export function bindSlot(key, tile) {
  const { page, id } = slotRef(key);
  return boardChange('set_binding', { pad: state.pad, page, id, entry: entryForTile(tile, slotLabel(state.config, state.pad, key)) });
}

export function clearSlot(key) { return bindSlot(key, ''); }

export function swapSlots(a, b) {
  const ra = slotRef(a), rb = slotRef(b);
  return boardChange('swap_bindings', { pad: state.pad, pageA: ra.page, idA: ra.id, pageB: rb.page, idB: rb.id });
}

export function setTriggerMode(side, mode) {
  return boardChange('set_trigger_mode', { pad: state.pad, side: side.toLowerCase(), mode });
}

export function resetPad() { return boardChange('reset_pad', { pad: state.pad }, '已恢復預設'); }
export function clearPad() { return boardChange('clear_pad', { pad: state.pad }, '已全部清除'); }

async function step(dir) {
  const current = p => clone(state.config.pads[p]);
  const e = dir === 'undo' ? hist.undo(current) : hist.redo(current);
  if (!e) return;
  try {
    await invoke('set_pad_tables', { pad: e.pad, tables: e.tables });
    if (e.pad !== state.pad) await setPad(e.pad);   // an undo switches to the pad it changes (spec 8)
    state.note = dir === 'undo' ? '已復原' : '已重做';
  } catch (err) {
    hist.dropLast(dir);
    state.note = (dir === 'undo' ? '無法復原：' : '無法重做：') + errText(err);
  }
  syncHist();
  rerender();
}
export const undo = () => step('undo');
export const redo = () => step('redo');

async function simple(cmd, args) {
  try { await invoke(cmd, args); } catch (e) { state.note = errText(e); rerender(); }
}

export function disconnectPad(pad) { return simple('disconnect_gamepad', { pad }); }
export function setRumble(enabled) { return simple('set_rumble_on_connect', { enabled }); }

// The header capsule: 關 is runtime only (set_pad_off); 鍵鼠 / Xbox / 手把 clear
// it and save output_mode. Rust decides what each mode does (iron rule 9).
async function setOff(pad, off) {
  const list = await invoke('set_pad_off', { kind: pad, off });
  if (Array.isArray(list)) state.offPads = new Set(list);
}
const modeHooks = [];
export function onModeChange(f) { modeHooks.push(f); }
export async function setMode(mode) {
  const pad = state.pad;
  try {
    if (mode === 'off') { await setOff(pad, true); modeHooks.forEach(f => f(pad, mode)); rerender(); return; }
    if (state.offPads.has(pad)) await setOff(pad, false);
    if (state.config.output_mode?.[pad] !== mode) await invoke('set_output_mode', { kind: pad, mode });
    modeHooks.forEach(f => f(pad, mode));
    rerender();
  } catch (e) { state.note = errText(e); rerender(); }
}

// The Settings struct of `set_settings`, from the live config.
export function settingsFrom(c) {
  return {
    deadzone: c.deadzone, trigger_threshold: c.trigger_threshold, min_press_ms: c.min_press_ms, tick_jitter_ms: c.tick_jitter_ms,
    log_events: c.log_events, touchpad_cursor_enabled: c.touchpad_cursor_enabled, touchpad_cursor_sensitivity: c.touchpad_cursor_sensitivity,
    touchpad_midpoint_x: c.touchpad_midpoint_x, touchpad_midpoint_y: c.touchpad_midpoint_y,
    touchpad_accel_slow_threshold: c.touchpad_accel_slow_threshold, touchpad_accel_fast_threshold: c.touchpad_accel_fast_threshold,
    touchpad_accel_gain_slow: c.touchpad_accel_gain_slow, touchpad_accel_gain_fast: c.touchpad_accel_gain_fast,
    touchpad_deadzone_radius: c.touchpad_deadzone_radius, touchpad_click_freeze_enabled: c.touchpad_click_freeze_enabled,
    cursor_stick_speed: c.cursor_stick_speed ?? 1, cursor_dpad_speed: c.cursor_dpad_speed ?? 1,
  };
}

export function setTouchpadCursor(enabled) {
  return simple('set_settings', { settings: { ...settingsFrom(state.config), touchpad_cursor_enabled: enabled } });
}
