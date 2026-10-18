// The board as one HTML string (spec 4.1), ported from the mockup's mockHTML
// for the live screen. Pure: state in, markup out.

import { layoutFor, slotsOf, groupSlots, routeHTML } from './layout.js';
import { padSvg } from './padart.js';
import { esc, tagSvg, TAGTXT, switchHTML } from './tags.js';
import { LABEL } from './keys.js';
import { boardBinds, PADS, PAD_NAMES } from './model.js';
import { keyboardHTML, mouseHTML, macroBoxHTML } from './palette.js';
import { ICON } from './icons.js';
import { editorHTML } from './macros.js';
import { settingsHTML } from './settings.js';
import { bangHTML, popHTML, vigemBlocksBanner } from './vigem.js';

export const CAPTURE_TEXT = '按鍵鼠上的鍵，或點下面的方塊（Esc 取消）';

export function triggerModes(config, pad) {
  return config?.pads?.[pad]?.trigger_mode || { l: 'page', r: 'page' };
}

export function mappingOn(config, pad) {
  const v = config?.mapping_enabled?.[pad];
  return v === undefined ? true : v;
}

// The selected pad's output mode: 'off' (關, this session), 'kbm' (鍵鼠) or
// 'pad' (手把 / Xbox), from `mapping_enabled`.
export function outputMode(st, pad) {
  if (st.offPads && st.offPads.has(pad)) return 'off';
  return mappingOn(st.config, pad) ? 'kbm' : 'pad';
}

// Any mode but 鍵鼠 locks everything below the header: the bindings do
// nothing then, so editing them only confuses (v3.2.1). An open macro editor
// stays usable until it is closed.
export function boardLocked(st) {
  return outputMode(st, st.pad) !== 'kbm' && !st.medit;
}

export const OFF_BANNER = '這支手把已關閉';
export function padBanner(pad) {
  return pad === 'ns2_pro' ? '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用' : '程式不送東西，遊戲直接用手把；下面的鍵鼠綁定這時不作用';
}

function omodeHTML(st, pad) {
  const m = outputMode(st, pad), xbox = pad === 'ns2_pro';
  const seg = (mode, cls, icon, label, title) => '<button type="button" class="' + cls + '" data-act="mode" data-mode="' + mode + '" aria-pressed="' + (m === mode) +
    '" title="' + title + '">' + icon + '<span>' + label + '</span></button>';
  return '<span class="omode" role="group" aria-label="輸出">' +
    seg('off', 'o-off', ICON.pause, '關', '這次先關掉這支手把（重開程式會恢復）') +
    seg('kbm', 'o-kbm', ICON.kbd, '鍵鼠', '手把按鍵送出鍵盤、滑鼠') +
    seg('pad', xbox ? 'o-xbox' : 'o-pad', ICON.gpad, xbox ? 'Xbox' : '手把', xbox ? '遊戲看到一支 Xbox 手把（需要 ViGEmBus）' : '程式不送東西，遊戲直接用手把') + '</span>';
}

export function disp(k) {
  if (!k) return '';
  if (k.startsWith('M:')) return k.slice(2);
  return LABEL[k] || k;
}

function slotBtn(st, key, tag, x, y, b) {
  const k = b[key] || '', mac = k.startsWith('M:'), wait = st.sel === key;
  return '<button type="button" class="slot' + (k ? ' on' : '') + (mac ? ' mac' : '') + (st.held[key] ? ' hit' : '') + (wait ? ' wait sel' : '') +
    '" style="left:' + x + 'px;top:' + y + 'px" data-slot="' + key + '" draggable="' + (k ? 'true' : 'false') + '" aria-label="' +
    esc((TAGTXT[tag] || tag) + (k ? '：' + (mac ? '巨集 ' : '') + disp(k) : '：未設定')) + '"><span class="tag">' + tagSvg(tag) + '</span>' +
    '<span class="val">' + esc(disp(k)) + '</span></button>';
}

function qcapHTML(pad, side, mode) {
  const on = mode === 'page', name = pad === 'dualsense' ? side + '2' : 'Z' + side;
  return '<span class="seg">' + name + '<button type="button" data-qs="' + side + '" data-mode="quick" aria-pressed="' + on + '">快捷頁</button>' +
    '<button type="button" data-qs="' + side + '" data-mode="single" aria-pressed="' + !on + '">單鍵</button></span>';
}


export function render(st) {
  const pad = st.pad, cfg = st.config, modes = triggerModes(cfg, pad), L = layoutFor(pad, modes), b = boardBinds(cfg, pad);
  let h = '<header class="hdr"><div class="ttl">按鍵設定</div><div class="pads">';
  for (const p of PADS) {
    const c = st.connected.has(p), act = pad === p;
    h += '<span class="pdw' + (act ? ' act' : '') + (c ? ' c' : '') + '"><button type="button" class="pd' + (act ? ' act' : '') + '" data-pad="' + p +
      '" aria-pressed="' + act + '" aria-label="' + PAD_NAMES[p] + (c ? '（已連線）' : '（未連線）') + '"><span class="live' + (c ? '' : ' idle') + '"></span><span>' +
      PAD_NAMES[p] + '</span></button>' +
      (c ? '<button type="button" class="pwr" data-act="cut" data-cutpad="' + p + '" aria-label="斷開 ' + PAD_NAMES[p] + '" title="斷開手把">' + ICON.power + '</button>' : '') + '</span>';
  }
  // The DualSense touchpad-cursor switch sits left of 連線震動 (reskin Q18), so the other controls keep
  // their place when it comes and goes; it locks with the board outside 鍵鼠 (events.js LOCKED).
  h += '</div><div class="sp"></div>' + (pad === 'dualsense' ? switchHTML('tp', '觸控板游標', cfg.touchpad_cursor_enabled !== false) : '') +
    switchHTML('rumble', '連線震動', cfg.rumble_on_connect !== false) + omodeHTML(st, pad) + bangHTML() +
    '<button type="button" class="gear" data-act="setopen" aria-label="設定" title="設定">' + ICON.gear + '</button></header>';
  h += '<div class="stage"></div>' + padSvg(pad);
  const om = outputMode(st, pad);
  if (om === 'off') h += '<div class="obanner gold" role="status">' + ICON.pause + '<span>' + OFF_BANNER + '</span></div>';
  else if (om === 'pad' && !vigemBlocksBanner()) h += '<div class="obanner white" role="status"><span>' + padBanner(pad) + '</span></div>';
  const hot = {};
  for (const g of L.groups) {
    if (g.q) h += '<div class="qcap" style="left:' + (g.x - 124) + 'px;top:' + (g.y - 66) + 'px">' + qcapHTML(pad, g.q, g.q === 'L' ? modes.l : modes.r) + '</div>';
    // One dark plate under every slot; touching slots merge into one group base (MapleStory quick slot).
    for (const s of groupSlots(g)) h += '<i class="plate" style="left:' + (s.x - 2) + 'px;top:' + (s.y - 2) + 'px"></i>';
    for (const s of groupSlots(g)) {
      h += slotBtn(st, s.key, s.tag, s.x, s.y, b);
      if (st.held[s.key]) hot[g.id] = true;
    }
  }
  // Lines last: always on top (spec 4.3).
  h += '<svg class="lines" viewBox="0 0 980 640" aria-hidden="true">' + L.routes.map(r => routeHTML(r.pts, r.r, r.m, !!hot[r.g])).join('') + '</svg>';
  const lock = st.medit ? ' locked' : '';
  const waiting = st.sel !== null && !st.medit;
  // Tiles already bound on this pad are drawn pale (display only, iron rule 9).
  const used = new Set(Object.values(b).filter(v => typeof v === 'string' && v));
  // The capture hint is for screen readers only (the board shows the blue slot).
  h += '<section class="pal">' + (waiting ? '<div class="capbar" role="status">' + CAPTURE_TEXT + '</div>' : '') + keyboardHTML(st, used) +
    '<div class="m3s' + lock + '">' + mouseHTML(st, used) + '</div>' + macroBoxHTML(st, lock, used) + '</section>';
  h += popHTML();
  if (st.medit) h += editorHTML(st);
  if (st.setOpen) h += settingsHTML(st);
  const lk = boardLocked(st), dis = on => (on && !lk ? '' : ' disabled');
  h += '<footer class="ftr"><button type="button" class="b-def" data-act="reset"' + dis(true) + '>恢復預設</button><button type="button" class="b-clr" data-act="clear"' + dis(true) + '>全部清除</button>' +
    '<span class="note">' + esc(st.note) + '</span><div class="sp"></div>' +
    '<button type="button" class="b-hist" data-act="undo" title="復原（Ctrl+Z）"' + dis(st.canUndo) + '>' + ICON.undo + '復原</button>' +
    '<button type="button" class="b-hist redo" data-act="redo" title="重做（Ctrl+Y）"' + dis(st.canRedo) + '><span class="flip">' + ICON.undo + '</span>重做</button></footer>';
  return h;
}

export { slotsOf };
