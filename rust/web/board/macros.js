// Macro box paging and the macro editor (spec 4.6), ported from the mockup.
// The editor works on a draft; 完成 saves it (rename_macro, then set_macro).

import { invoke } from '../ipc.js';
import { state, rerender, reloadConfig } from './store.js';
import { esc, switchHTML } from './tags.js';
import { LABEL, MOUSE_TILES } from './keys.js';
import { keyToTile, tileToKey } from './model.js';
import { ICON } from './icons.js';
import { hook, selectSlot } from './events.js';
import { PER, macroNames, macroPages } from './palette.js';

export const NAME_MAX = 8;
const DOWN_D = [40, 60], UP_D = [15, 25];

export function badRange(d, strict) {
  return !(d[0] >= 0 && d[1] >= 0) || (strict ? d[0] >= d[1] : d[0] > d[1]);
}
function isKeyTile(k) { return !k.startsWith('M:') && !MOUSE_TILES.includes(k); }

function draftOf(name) {
  const def = state.config.macros[name] || { loop: false, steps: [] };
  return { origName: name, name, loop: !!def.loop, steps: def.steps.map(s => ({ key: keyToTile(s.key), action: s.action, d: [s.delay_ms[0], s.delay_ms[1]] })), err: '' };
}

export function editorHTML(st) {
  const m = st.medit;
  let h = '<section class="med" aria-label="編輯巨集">';
  h += '<div class="mhd">' + ICON.mac + '<input class="mname" data-f="mname" value="' + esc(m.name) + '" maxlength="' + NAME_MAX + '" aria-label="巨集名稱">' +
    switchHTML('mloop', '按住時重複', m.loop, { title: '按住按鍵時一直重複', icon: ICON.loop }) +
    '<div class="sp"></div>' + (m.err ? '<span class="merr" role="alert">' + esc(m.err) + '</span>' : '') +
    '<button type="button" class="mbtn del" data-act="mdel" aria-label="刪除這個巨集" title="刪除巨集">' + ICON.trash + '</button>' +
    '<button type="button" class="mbtn ok" data-act="mdone">完成</button></div><div class="tl" data-tl="1">';
  m.steps.forEach((s, i) => {
    const down = s.action === 'down', lab = LABEL[s.key] || s.key;
    h += '<div class="st" draggable="true" data-step="' + i + '">' +
      '<button type="button" class="cap ' + s.action + '" data-act="stoggle" data-i="' + i + '" aria-label="' + esc(lab) + (down ? ' 按下' : ' 放開') + '，點一下切換" title="' + (down ? '按下' : '放開') + '（點一下切換）"><span class="k">' + esc(lab) + '</span><span class="ar">' + (down ? '▼' : '▲') + '</span></button>' +
      '<button type="button" class="dl' + (badRange(s.d, true) ? ' bad' : '') + '" data-act="sdelay" data-i="' + i + '" aria-label="之後等 ' + s.d[0] + ' 到 ' + s.d[1] + ' 毫秒" title="之後等待（毫秒）">' + ICON.clock + s.d[0] + '–' + s.d[1] + '</button>';
    if (st.mpop === i) {
      const bad = badRange(s.d, true);
      h += '<div class="dpop" role="group" aria-label="等待時間（毫秒）"><div class="row">' + ICON.clock +
        '<input class="num" type="number" min="0" step="5" data-f="dlo" data-i="' + i + '" value="' + s.d[0] + '" aria-label="最短毫秒" aria-invalid="' + bad + '">–' +
        '<input class="num" type="number" min="0" step="5" data-f="dhi" data-i="' + i + '" value="' + s.d[1] + '" aria-label="最長毫秒" aria-invalid="' + bad + '"></div>' +
        '<div class="row hint' + (bad ? ' bad' : '') + '">' + (bad ? '前面要比後面小' : '每次隨機，毫秒') + '</div></div>';
    }
    h += '</div>';
  });
  h += '<div class="st" data-step="end"><span class="cap ghost" aria-hidden="true">＋</span></div>';
  if (!m.steps.length) h += '<div class="empty"><svg width="40" height="40" viewBox="0 0 40 40" aria-hidden="true"><rect x="8" y="4" width="24" height="18" rx="4" fill="none" stroke="currentColor" stroke-width="2.5"/><path d="M20 24v12M14 30l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2.5"/></svg>把下方鍵盤的鍵拖進來，或點一下鍵</div>';
  return h + '</div></section>';
}

function open(name) {
  if (state.sel !== null) selectSlot(null);
  state.picked = null;
  state.medit = draftOf(name);
  state.mpop = null;
  rerender();
  document.querySelector('#board .mname')?.focus();
}

async function createMacro() {
  const names = new Set(macroNames(state.config));
  let n = 1;
  while (names.has('巨集 ' + n)) n++;
  const name = '巨集 ' + n;
  try {
    await invoke('set_macro', { name, def: { loop: false, steps: [] } });
    await reloadConfig();
    state.mpage = Math.floor(macroNames(state.config).indexOf(name) / PER);
    open(name);
  } catch (e) { state.note = String(e); rerender(); }
}

function insertTap(key, at) {
  state.medit.steps.splice(at, 0, { key, action: 'down', d: DOWN_D.slice() }, { key, action: 'up', d: UP_D.slice() });
  state.mpop = null;
  rerender();
}

async function done() {
  const m = state.medit;
  const bad = m.steps.findIndex(s => badRange(s.d, true));
  if (bad >= 0) { state.mpop = bad; rerender(); return; }
  // The 8-character limit applies to names typed here; an existing longer
  // name (hand-edited config) is kept as it is unless the user changed it.
  let name = m.name.trim() || m.origName;
  if (name !== m.origName) name = name.slice(0, NAME_MAX);
  if (m.loop && !m.steps.length) { m.err = '按住時重複需要至少一步'; rerender(); return; }
  try {
    if (name !== m.origName) {
      if (macroNames(state.config).includes(name)) { m.err = '已經有叫「' + name + '」的巨集'; rerender(); return; }
      await invoke('rename_macro', { old: m.origName, new: name });
    }
    await invoke('set_macro', { name, def: { loop: m.loop, steps: m.steps.map(s => ({ key: tileToKey(s.key), action: s.action, delay_ms: [s.d[0], s.d[1]] })) } });
    state.medit = null;
    state.mpop = null;
    state.note = '巨集已儲存';
  } catch (e) {
    m.err = String(e && e.message ? e.message : e);
  }
  rerender();
}

async function remove() {
  const name = state.medit.origName;
  try {
    await invoke('delete_macro', { name });
    state.medit = null;
    state.mpop = null;
    state.mpage = Math.min(state.mpage, macroPages(state.config) - 1);
    state.note = '已刪除巨集';
  } catch (e) { state.medit.err = String(e); }
  rerender();
}

export function startMacros(board) {
  hook('click', ev => {
    const pen = ev.target.closest('[data-medit]');
    if (pen) { open(pen.dataset.medit); return true; }
    const el = ev.target.closest('[data-act],[data-tile]');
    if (el?.dataset.act === 'mnew') { createMacro(); return true; }
    if (!state.medit) return false;
    if (state.mpop !== null && !ev.target.closest('.dpop') && !ev.target.closest('[data-act="sdelay"]')) { state.mpop = null; rerender(); }
    if (!el) return state.mpop === null ? false : true;
    if (el.dataset.tile) {
      if (isKeyTile(el.dataset.tile)) insertTap(el.dataset.tile, state.medit.steps.length);
      return true;   // mouse / macro tiles are locked while editing
    }
    const a = el.dataset.act, i = +el.dataset.i, m = state.medit;
    if (a === 'mloop') { m.loop = !m.loop; m.err = ''; rerender(); return true; }
    if (a === 'mdone') { done(); return true; }
    if (a === 'mdel') { remove(); return true; }
    if (a === 'stoggle') { m.steps[i].action = m.steps[i].action === 'down' ? 'up' : 'down'; rerender(); return true; }
    if (a === 'sdelay') { state.mpop = state.mpop === i ? null : i; rerender(); if (state.mpop !== null) board.querySelector('.dpop .num')?.focus(); return true; }
    return ['reset', 'clear', 'undo', 'redo', 'setopen'].includes(a);   // board actions wait for 完成
  });

  // Live edits that must not re-render (a re-render would drop the caret).
  board.addEventListener('input', ev => {
    const f = ev.target.dataset?.f, m = state.medit;
    if (!m || !f) return;
    if (f === 'mname') { m.name = ev.target.value; m.err = ''; return; }
    if (f === 'dlo' || f === 'dhi') {
      const i = +ev.target.dataset.i, st = m.steps[i];
      st.d[f === 'dlo' ? 0 : 1] = Math.max(0, Math.round(+ev.target.value || 0));
      const bad = badRange(st.d, true);
      board.querySelectorAll('.dpop .num').forEach(n => n.setAttribute('aria-invalid', String(bad)));
      const pill = board.querySelector('[data-act="sdelay"][data-i="' + i + '"]');
      pill.lastChild.textContent = st.d[0] + '–' + st.d[1];
      pill.classList.toggle('bad', bad);
      const msg = board.querySelector('.dpop .row:last-child');
      msg.textContent = bad ? '前面要比後面小' : '每次隨機，毫秒';
      msg.classList.toggle('bad', bad);
    }
  });

  // Steps: drag to reorder, drag out to delete; key tiles drop in before a step.
  const sd = { step: null, dropped: false };
  const stepTarget = ev => {
    if (!ev.target.closest('[data-tl]')) return null;
    const sp = ev.target.closest('[data-step]');
    return sp && sp.dataset.step !== 'end' ? +sp.dataset.step : state.medit.steps.length;
  };
  const markDrop = at => {
    board.querySelectorAll('.drop-before').forEach(n => n.classList.remove('drop-before'));
    board.querySelector('.tl')?.classList.toggle('over', at !== null && at === state.medit.steps.length);
    if (at !== null) board.querySelector('[data-step="' + at + '"]')?.classList.add('drop-before');
  };
  hook('dragstart', (ev, _el, drag) => {
    if (!state.medit) return false;
    const sp = ev.target.closest('[data-step]');
    if (sp && sp.dataset.step !== 'end') {
      sd.step = +sp.dataset.step; sd.dropped = false;
      try { ev.dataTransfer.setData('text/plain', 'step'); ev.dataTransfer.effectAllowed = 'move'; } catch (x) { /* ignore */ }
      return true;
    }
    const t = ev.target.closest('[data-tile]');
    if (t && !isKeyTile(t.dataset.tile)) { ev.preventDefault(); return true; }
    return false;   // key tiles: the board's dragstart records drag.key
  });
  hook('dragover', (ev, _el, drag) => {
    if (!state.medit) return false;
    const at = stepTarget(ev);
    if (at !== null && (sd.step !== null || (drag.key && isKeyTile(drag.key)))) { ev.preventDefault(); markDrop(at); } else markDrop(null);
    return true;
  });
  hook('drop', (ev, _el, drag) => {
    if (!state.medit) return false;
    const at = stepTarget(ev);
    if (at === null) return true;
    ev.preventDefault();
    markDrop(null);
    const m = state.medit;
    if (sd.step !== null) {
      sd.dropped = true;
      const from = sd.step, item = m.steps.splice(from, 1)[0];
      m.steps.splice(at > from ? at - 1 : at, 0, item);
      sd.step = null; state.mpop = null; rerender();
    } else if (drag.key && isKeyTile(drag.key)) { const k = drag.key; drag.key = null; insertTap(k, at); }
    return true;
  });
  hook('dragend', () => {
    if (sd.step !== null) {
      if (!sd.dropped && state.medit) { state.medit.steps.splice(sd.step, 1); state.mpop = null; }
      sd.step = null; sd.dropped = false; rerender();
      return true;
    }
    if (state.medit) markDrop(null);
    return false;
  });

  // Mouse wheel over the macro box turns its pages (no scrolling, spec 4.6).
  board.addEventListener('wheel', ev => {
    const box = ev.target.closest('.macros');
    if (!box || box.classList.contains('locked') || state.sel !== null) return;
    ev.preventDefault();
    const np = Math.max(0, Math.min(macroPages(state.config) - 1, state.mpage + (ev.deltaY > 0 ? 1 : -1)));
    if (np !== state.mpage) { state.mpage = np; rerender(); }
  }, { passive: false });
}
