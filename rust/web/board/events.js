// Pointer and keyboard wiring of the board (spec 4.5), ported from the
// mockup's handlers minus its layout editor. Every change is an action.

import { state, rerender, setPad } from './store.js';
import * as act from './actions.js';
import { boardBinds } from './model.js';
import { macroPages, dragGhostHTML } from './palette.js';
import { boardLocked } from './view.js';

const drag = { key: null, from: null, dropped: false, step: null };
const hooks = { click: [], dragover: [], drop: [], dragstart: [], dragend: [] };
// Later features (capture, macro editor, settings) register handlers that run
// first and return true when they consumed the event.
export function hook(kind, f) { hooks[kind].push(f); }
const consumed = (kind, ev, el) => hooks[kind].some(f => f(ev, el, drag));

// What a locked board ignores: everything below the header except the macro
// editor and the settings window (which are not open while it is locked).
const LOCKED = '.slot,.tile,[data-act="tp"],.qcap,.pal,.lines,[data-act="reset"],[data-act="clear"],[data-act="undo"],[data-act="redo"],[data-mpage]';

export function wire(board) {
  const gate = ev => {
    if (!boardLocked(state)) return;
    if (ev.target === board || ev.target.closest(LOCKED)) { ev.preventDefault(); ev.stopPropagation(); }
  };
  for (const kind of ['click', 'dragstart', 'drop', 'wheel']) board.addEventListener(kind, gate, { capture: true, passive: false });
  board.addEventListener('click', ev => {
    if (consumed('click', ev)) return;
    const qb = ev.target.closest('[data-qs]');
    if (qb) { act.setTriggerMode(qb.dataset.qs, qb.dataset.mode === 'quick' ? 'page' : 'key'); return; }
    const mp = ev.target.closest('[data-mpage]');
    if (mp) { state.mpage = +mp.dataset.mpage; rerender(); return; }
    const el = ev.target.closest('[data-slot],[data-tile],[data-act],[data-pad]');
    if (!el) return;
    if (el.dataset.pad) { setPad(el.dataset.pad).then(rerender); return; }
    // The first click re-renders the board, so a native dblclick never reaches
    // the slot: count clicks instead (as the mockup does).
    if (el.dataset.slot && ev.detail >= 2) { act.clearSlot(el.dataset.slot); return; }
    if (el.dataset.slot) {
      const key = el.dataset.slot;
      if (state.picked) { act.bindSlot(key, state.picked); return; }
      selectSlot(state.sel === key ? null : key);
      return;
    }
    if (el.dataset.tile) {
      const t = el.dataset.tile;
      state.picked = state.picked === t ? null : t;
      rerender();
      return;
    }
    const a = el.dataset.act, c = state.config;
    if (a === 'rumble') act.setRumble(c.rumble_on_connect === false);
    else if (a === 'mode') act.setMode(el.dataset.mode);
    else if (a === 'tp') act.setTouchpadCursor(c.touchpad_cursor_enabled === false);
    else if (a === 'reset') act.resetPad();
    else if (a === 'clear') act.clearPad();
    else if (a === 'undo') act.undo();
    else if (a === 'redo') act.redo();
    else if (a === 'cut') act.disconnectPad(el.dataset.cutpad);
    else if (a === 'mprev') { state.mpage = Math.max(0, state.mpage - 1); rerender(); }
    else if (a === 'mnext') { state.mpage = Math.min(macroPages(c) - 1, state.mpage + 1); rerender(); }
  });

  board.addEventListener('dragstart', ev => {
    if (consumed('dragstart', ev)) return;
    const t = ev.target.closest('[data-tile]');
    if (t) {
      drag.key = t.dataset.tile; drag.from = null;
      try { ev.dataTransfer.setData('text/plain', drag.key); ev.dataTransfer.effectAllowed = 'copy'; } catch (x) { /* ignore */ }
      // Drag image: the slot the key will become (reskin Q7 C). The node only lives for this dragstart.
      const g = document.createElement('div');
      g.innerHTML = dragGhostHTML(drag.key); g.firstChild.style.cssText = 'position:absolute;left:-200px;top:0';
      board.appendChild(g.firstChild);
      const ghost = board.lastChild;
      try { ev.dataTransfer.setDragImage(ghost, 19, 19); } catch (x) { /* ignore */ }
      setTimeout(() => ghost.remove(), 0);
      return;
    }
    const s = ev.target.closest('[data-slot]');
    if (s) {
      const key = s.dataset.slot;
      if (!boardBinds(state.config, state.pad)[key]) { ev.preventDefault(); return; }
      drag.from = key; drag.key = null; drag.dropped = false;
      try { ev.dataTransfer.setData('text/plain', 'slot:' + key); ev.dataTransfer.effectAllowed = 'move'; } catch (x) { /* ignore */ }
    }
  });
  board.addEventListener('dragover', ev => {
    if (consumed('dragover', ev)) return;
    if (ev.target.closest('[data-slot]')) ev.preventDefault();
  });
  board.addEventListener('drop', ev => {
    if (consumed('drop', ev)) return;
    const s = ev.target.closest('[data-slot]');
    if (!s) return;
    ev.preventDefault();
    const key = s.dataset.slot;
    if (drag.from !== null) {
      drag.dropped = true;
      const from = drag.from; drag.from = null;
      if (from !== key) act.swapSlots(from, key);
      return;
    }
    const k = drag.key; drag.key = null;
    if (k) act.bindSlot(key, k);
  });
  board.addEventListener('dragend', ev => {
    if (consumed('dragend', ev)) return;
    // A bound slot dragged out to empty space is deleted (spec 4.5).
    if (drag.from !== null && !drag.dropped) { const key = drag.from; drag.from = null; act.clearSlot(key); }
    drag.from = null; drag.dropped = false; drag.key = null;
  });
}

// Select (or deselect, with null) the slot waiting for a key. The capture
// feature hooks this to toggle the engine's capture flag.
const selectHooks = [];
export function onSelect(f) { selectHooks.push(f); }
export function selectSlot(key) {
  state.sel = key;
  state.picked = null;
  selectHooks.forEach(f => f(key));
  rerender();
}
