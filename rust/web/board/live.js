// Live feedback and key capture on the board (spec 4.5). Display only: which
// slot lights is chosen from engine events, never a mapping decision (iron
// rule 9).

import { invoke, listen } from '../ipc.js';
import { state, rerender, onPadChange } from './store.js';
import { boardBinds, keyToTile, QUICK_IDS } from './model.js';
import { triggerModes } from './view.js';
import { hook, onSelect, selectSlot } from './events.js';
import * as act from './actions.js';

// ─── Real key → palette tile (from the mockup's keyName) ─────────────────────
const CODE = { ShiftLeft: 'LShift', ShiftRight: 'RShift', ControlLeft: 'LCtrl', ControlRight: 'RCtrl', AltLeft: 'LAlt', AltRight: 'RAlt',
  MetaLeft: 'Win', MetaRight: 'Win', ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→', Escape: 'Esc', Space: 'Space', Enter: 'Enter',
  Tab: 'Tab', Backspace: 'Backspace', Delete: 'Del', Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown', Minus: '-',
  Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'", Comma: ',', Period: '.', Slash: '/', Backquote: '`' };
export function keyName(ev) {
  if (CODE[ev.code]) return CODE[ev.code];
  const m = /^Key([A-Z])$/.exec(ev.code) || /^Digit(\d)$/.exec(ev.code) || /^(F\d{1,2})$/.exec(ev.code);
  return m ? m[1] : null;
}

// ─── Lit state, from three sources, merged into state.held / state.hot ──────
const pressed = new Map();   // button id -> { slot, tile, n }
const triggers = [];         // held trigger ids (23 / 24), press order
const emitted = new Set();   // tiles the engine is holding down right now
const reverse = new Set();   // tiles of real keys held on this window

function paintLit() {
  const held = {}, hot = {};
  for (const p of pressed.values()) { held[p.slot] = true; if (p.tile) hot[p.tile] = true; }
  for (const t of emitted) hot[t] = true;
  if (reverse.size) {
    const b = boardBinds(state.config, state.pad);
    for (const t of reverse) {
      hot[t] = true;
      for (const [slot, v] of Object.entries(b)) if (v === t) held[slot] = true;
    }
  }
  state.held = held;
  state.hot = hot;
  rerender();
}

// The slot a press of `id` lights: the quick page of the last held
// page-mode trigger for D-pad / face ids, else the base slot.
function slotFor(id) {
  if (QUICK_IDS.includes(id)) {
    const modes = triggerModes(state.config, state.pad);
    for (let i = triggers.length - 1; i >= 0; i--) {
      const side = triggers[i] === 23 ? 'l' : 'r';
      if (modes[side] === 'page') return (side === 'l' ? 'L' : 'R') + id;
    }
  }
  return String(id);
}

function down(id) {
  const p = pressed.get(id);
  if (p) { p.n++; return; }   // the engine re-emits analog trigger / stick edges
  if ((id === 23 || id === 24) && !triggers.includes(id)) triggers.push(id);
  const slot = slotFor(id);
  pressed.set(id, { slot, tile: boardBinds(state.config, state.pad)[slot] || '', n: 1 });
  paintLit();
}

function up(id) {
  const p = pressed.get(id);
  if (!p) return;
  if (--p.n > 0) return;
  pressed.delete(id);
  const i = triggers.indexOf(id);
  if (i >= 0) triggers.splice(i, 1);
  paintLit();
}

// ─── Reverse lookup with the synthesized-key filter (keyboard_mirror.js) ─────
const recentEmit = new Map();   // tile -> time of the last engine emit 'down'
function typing(ev) { return /INPUT|TEXTAREA|SELECT/.test(ev.target?.tagName || ''); }

// Every held key lights its tile, bound or not, in every mode (v3.4.0);
// not while typing in a text field.
function revDown(ev) {
  if (ev.repeat || typing(ev)) return;
  const k = keyName(ev);
  if (!k) return;
  const t0 = performance.now();
  // Defer 30 ms: an engine emit of the same key inside that window means the
  // engine typed it, not the user.
  setTimeout(() => {
    const e = recentEmit.get(k);
    if (e !== undefined && e >= t0 - 30) return;
    reverse.add(k);
    paintLit();
  }, 30);
}
function revUp(ev) {
  const k = keyName(ev);
  if (k && reverse.delete(k)) paintLit();
}
function clearAll() {
  if (reverse.size) { reverse.clear(); paintLit(); }
}

// ─── Key capture ─────────────────────────────────────────────────────────────
function capturing() { return state.sel !== null && !state.medit && !state.setOpen; }
function bindCaptured(tile) {
  const key = state.sel;
  selectSlot(null);
  act.bindSlot(key, tile);
}

export async function startLive() {
  let captureOn = false;
  onSelect(key => {
    captureOn = key !== null;
    invoke('set_capture_active', { active: captureOn }).catch(e => console.warn('set_capture_active', e));
  });
  onPadChange(() => {
    pressed.clear(); triggers.length = 0; reverse.clear();
    if (captureOn) selectSlot(null);   // setPad already cleared state.sel
  });
  // A tile click while a slot waits binds it (and ends capture).
  hook('click', ev => {
    const t = ev.target.closest('[data-tile]');
    if (!t || state.sel === null || state.medit) return false;
    bindCaptured(t.dataset.tile);
    return true;
  });

  await listen('button-down', p => down(p.id));
  await listen('button-up', p => up(p.id));
  await listen('activity', p => {
    if (p.kind !== 'emit' || !p.key) return;
    const t = keyToTile(p.key);
    if (p.action === 'down') { recentEmit.set(t, performance.now()); emitted.add(t); } else emitted.delete(t);
    paintLit();
  });
  await listen('controller-status', s => { if (s.connected === false) { pressed.clear(); triggers.length = 0; paintLit(); } });

  // The board has no keyboard shortcuts and keeps no browser key behaviour
  // (focus moves, Space / Enter pressing a button, F5, …): outside a text
  // field a key only lights its tile and binds a waiting slot (v3.4.0).
  const swallow = ev => { if (!typing(ev)) ev.preventDefault(); };
  document.addEventListener('keydown', swallow, true);
  document.addEventListener('keyup', swallow, true);
  document.addEventListener('keydown', ev => {
    if (!capturing() || typing(ev)) return;
    const k = keyName(ev);
    if (k) bindCaptured(k);
  }, true);
  document.addEventListener('mousedown', ev => {
    if (!capturing()) return;
    const m = ev.button === 1 ? '中鍵' : ev.button === 2 ? '右鍵' : null;
    if (m) { ev.preventDefault(); ev.stopImmediatePropagation(); bindCaptured(m); }
  }, true);
  document.addEventListener('contextmenu', ev => { if (state.sel !== null || ev.target.closest('#board')) ev.preventDefault(); }, true);
  document.addEventListener('wheel', ev => {
    if (!capturing()) return;
    ev.preventDefault();
    ev.stopImmediatePropagation();
    bindCaptured(ev.deltaY < 0 ? '上滾' : '下滾');
  }, { capture: true, passive: false });

  document.addEventListener('keydown', revDown);
  document.addEventListener('keyup', revUp);
  window.addEventListener('blur', () => {
    clearAll();
    if (state.sel !== null) selectSlot(null);
  });
}
