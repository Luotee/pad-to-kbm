// Pointer and keyboard wiring of the board (spec 4.5), ported from the
// mockup's handlers minus its layout editor. Every change is an action.

import { state, rerender, setPad } from './store.js';
import * as act from './actions.js';
import { boardBinds, slotOff, CURSOR_TILE, cursorGroup } from './model.js';
import { macroPages, dragGhostHTML } from './palette.js';
import { boardLocked, cursorSilHTML, triggerModes } from './view.js';
import { layoutFor } from './layout.js';

const drag = { key: null, from: null, dropped: false, step: null, cursor: false, refused: false };

// 游標 (v3.9.0) covers a group of four, so some drops cannot happen: the cursor onto a slot outside every
// group, or a slot outside every group onto a cursor slot (Rust refuses both; this only spares the call).
function dropOk(key) {
  const b = boardBinds(state.config, state.pad);
  if (drag.cursor) return !!cursorGroup(key);
  if (drag.from !== null && (b[key] === CURSOR_TILE)) return !!cursorGroup(drag.from);
  return true;
}

// While the cursor is dragged: dim what cannot take it, outline the groups that can (6A1).
function cursorDragStart(board) {
  drag.cursor = true;
  for (const s of board.querySelectorAll('.slot')) if (!cursorGroup(s.dataset.slot)) s.classList.add('cdim');
  for (const e of board.querySelectorAll('.lines,.qcap')) e.classList.add('cdim');
  board.insertAdjacentHTML('beforeend', cursorSilHTML(layoutFor(state.pad, triggerModes(state.config, state.pad)), null));
}
function cursorHover(board, key) {
  const g = key && cursorGroup(key);
  for (const e of board.querySelectorAll('.csil [data-cg]')) e.classList.toggle('hov', !!g && e.dataset.cg === g[0]);
}
function cursorDragEnd(board) {
  if (!drag.cursor) return;
  drag.cursor = false;
  for (const e of board.querySelectorAll('.cdim')) e.classList.remove('cdim');
  board.querySelector('.csil')?.remove();
}
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
    // A slot the pad never sends (slotOff) is never bound, cleared or selected.
    if (el.dataset.slot && slotOff(state.pad, el.dataset.slot)) return;
    // The first click re-renders the board, so a native dblclick never reaches
    // the slot: count clicks instead (as the mockup does).
    if (el.dataset.slot && ev.detail >= 2) { act.clearSlot(el.dataset.slot); return; }
    if (el.dataset.slot) {
      const key = el.dataset.slot;
      if (state.picked === CURSOR_TILE && !cursorGroup(key)) { state.note = '游標只能放在搖桿或十字鍵'; rerender(); return; }
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
    else if (a === 'mode') { if (el.getAttribute('aria-disabled') !== 'true') act.setMode(el.dataset.mode); }
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
      if (drag.key === CURSOR_TILE) cursorDragStart(board);
      return;
    }
    const s = ev.target.closest('[data-slot]');
    if (s) {
      const key = s.dataset.slot;
      if (!boardBinds(state.config, state.pad)[key] || slotOff(state.pad, key)) { ev.preventDefault(); return; }
      drag.from = key; drag.key = null; drag.dropped = false;
      try { ev.dataTransfer.setData('text/plain', 'slot:' + key); ev.dataTransfer.effectAllowed = 'move'; } catch (x) { /* ignore */ }
      if (boardBinds(state.config, state.pad)[key] === CURSOR_TILE) cursorDragStart(board);
    }
  });
  board.addEventListener('dragover', ev => {
    if (consumed('dragover', ev)) return;
    const s = ev.target.closest('[data-slot]');
    // A slot that cannot take this drop: no drop (the not-allowed pointer), and letting go there is a cancel,
    // never the drag-out that clears a slot.
    drag.refused = !!s && !dropOk(s.dataset.slot);
    if (drag.cursor) cursorHover(board, s && !drag.refused ? s.dataset.slot : null);
    if (s && !drag.refused) ev.preventDefault();
  });
  board.addEventListener('drop', ev => {
    if (consumed('drop', ev)) return;
    const s = ev.target.closest('[data-slot]');
    if (!s) return;
    ev.preventDefault();
    const key = s.dataset.slot;
    // Dropped on a slot that is never bound: nothing happens (a dragged slot is not cleared either).
    if (slotOff(state.pad, key)) { if (drag.from !== null) drag.dropped = true; drag.key = null; return; }
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
    cursorDragEnd(board);
    if (consumed('dragend', ev)) return;
    // A bound slot dragged out to empty space is deleted (spec 4.5); a cursor slot takes its group with it (Rust).
    if (drag.from !== null && !drag.dropped && !drag.refused) { const key = drag.from; drag.from = null; act.clearSlot(key); }
    drag.from = null; drag.dropped = false; drag.key = null; drag.refused = false;
  });
  // 游標 picked in the palette: the group under the pointer turns solid, as during its drag.
  board.addEventListener('mouseover', ev => {
    if (state.picked !== CURSOR_TILE) return;
    const s = ev.target.closest('[data-slot]');
    cursorHover(board, s && cursorGroup(s.dataset.slot) ? s.dataset.slot : null);
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
