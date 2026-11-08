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
import { bangHTML, popHTML, vigemBlocksBanner, reconnectBanner, reconnectDismissable, autoOffBanner } from './vigem.js';
import { capHTML, popHTML as lbPopHTML, lbarHTML } from './lightbar.js';

export const CAPTURE_TEXT = '按鍵鼠上的鍵，或點下面的方塊（再點一次格子取消）';

export function triggerModes(config, pad) {
  return config?.pads?.[pad]?.trigger_mode || { l: 'page', r: 'page' };
}

export function mappingOn(config, pad) {
  const v = config?.mapping_enabled?.[pad];
  return v === undefined ? true : v;
}

// The selected pad's output mode: 'off' (關, this session), 'kbm' (鍵鼠), 'xbox'
// or 'pad' (手把), from `output_mode`; a pad missing there falls back to the
// pre-3.6.0 `mapping_enabled` (false = 手把, Xbox on the Switch 2 Pro) as Rust
// reads it. The Nano is an Xbox pad itself: its Xbox and 手把 are one option, stored as `pad` (Rust
// stores `pad` for an `xbox` pick too), so a hand-edited `xbox` reads as `pad`.
export function outputMode(st, pad) {
  if (st.offPads && st.offPads.has(pad)) return 'off';
  const m = st.config?.output_mode?.[pad];
  if (m === 'xbox' && pad === 'xbox_nano') return 'pad';
  if (m === 'kbm' || m === 'xbox' || m === 'pad') return m;
  return mappingOn(st.config, pad) ? 'kbm' : (pad === 'ns2_pro' ? 'xbox' : 'pad');
}
// Windows has no driver for the Switch 2 Pro, so a game cannot read it as itself.
export const hasNative = pad => pad !== 'ns2_pro';

// Any mode but 鍵鼠 locks everything below the header: the bindings do
// nothing then, so editing them only confuses (v3.2.1). An open macro editor
// stays usable until it is closed.
export function boardLocked(st) {
  return outputMode(st, st.pad) !== 'kbm' && !st.medit;
}

export const OFF_BANNER = '這支手把已關閉';
export function padBanner(mode, pad) {
  if (pad === 'xbox_nano') return '遊戲直接把它當 Xbox 手把用；下面的鍵鼠綁定這時不作用';
  return mode === 'xbox' ? '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用' : '程式不送東西，遊戲直接用手把；下面的鍵鼠綁定這時不作用';
}

function omodeHTML(st, pad) {
  if (pad === 'xbox_nano') return nanoModeHTML(st, pad);
  const m = outputMode(st, pad), nat = hasNative(pad);
  const seg = (mode, cls, icon, label, title, off) => '<button type="button" class="' + cls + '" data-act="mode" data-mode="' + mode + '" aria-pressed="' + (m === mode) + '"' +
    (off ? ' aria-disabled="true"' : '') + ' title="' + title + '">' + icon + '<span>' + label + '</span></button>';
  return '<span class="omode" role="group" aria-label="輸出">' +
    seg('off', 'o-off', ICON.pause, '關', '這次先關掉這支手把（重開程式會恢復）') +
    seg('kbm', 'o-kbm', ICON.kbd, '鍵鼠', '手把按鍵送出鍵盤、滑鼠') +
    seg('xbox', 'o-xbox', ICON.xpad, 'Xbox', '遊戲看到一支 Xbox 手把（需要 ViGEmBus' + (nat ? '、HidHide' : '') + '）') +
    seg('pad', 'o-pad', ICON.gpad, '手把', nat ? '程式不送東西，遊戲直接用手把' : 'Windows 認不得 Switch 2 Pro，遊戲讀不到它；要用手把玩請選 Xbox', !nat) + '</span>';
}

// The Nano keeps the four cells, but Xbox and 手把 are one option (both send `pad`): one .xjoin holds the
// two, and its pink spans both cells when it is on.
function nanoModeHTML(st, pad) {
  const m = outputMode(st, pad), on = m === 'pad', t = 'Nano 本身就是 Xbox 手把：程式不送東西，遊戲直接讀它（不需要 ViGEmBus）';
  const b = (mode, cls, icon, label, title, p) => '<button type="button" class="' + cls + '" data-act="mode" data-mode="' + mode + '" aria-pressed="' + p + '" title="' + title + '">' + icon + '<span>' + label + '</span></button>';
  return '<span class="omode" role="group" aria-label="輸出">' + b('off', 'o-off', ICON.pause, '關', '這次先關掉這支手把（重開程式會恢復）', m === 'off') +
    b('kbm', 'o-kbm', ICON.kbd, '鍵鼠', '手把按鍵送出鍵盤、滑鼠；遊戲看不到這支手把（HidHide）', m === 'kbm') +
    '<span class="xjoin' + (on ? ' on' : '') + '">' + b('pad', 'o-xbox', ICON.xpad, 'Xbox', t, on) + b('pad', 'o-pad', ICON.gpad, '手把', t, on) + '</span></span>';
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

// L2 / R2 (ZL / ZR): a caption plate holding the name and one framed two-cell field, like the 燈條 dropdown.
function qcapHTML(pad, side, mode) {
  const on = mode === 'page', name = pad === 'dualsense' ? side + '2' : pad === 'xbox_nano' ? side + 'T' : 'Z' + side;
  return '<span class="qplate"><span class="qname">' + name + '</span><span class="qsg"><button type="button" data-qs="' + side + '" data-mode="quick" aria-pressed="' + on + '">快捷頁</button>' +
    '<button type="button" data-qs="' + side + '" data-mode="single" aria-pressed="' + !on + '">單鍵</button></span></span>';
}


// A banner is centred with translate(-50%): an odd width puts it, and the pixel font in it, on a
// half pixel. One more pixel of right padding makes it even (after each paint).
export function evenBanners(root) {
  for (const b of root.querySelectorAll('.obanner')) {
    if (b.offsetWidth % 2) b.style.paddingRight = (parseFloat(getComputedStyle(b).paddingRight) + 1) + 'px';
  }
}

export function render(st) {
  const pad = st.pad, cfg = st.config, modes = triggerModes(cfg, pad), L = layoutFor(pad, modes), b = boardBinds(cfg, pad);
  let h = '<header class="hdr"><div class="ttl">按鍵設定</div><div class="pads">';
  for (const p of PADS) {
    // The Xbox Nano's power key is greyed out and disabled: the app cannot turn a Bluetooth LE pad off.
    const c = st.connected.has(p), act = pad === p, pwr = c && act, nano = p === 'xbox_nano';
    h += '<span class="pdw' + (act ? ' act' : '') + (c ? ' c' : '') + '"><button type="button" class="pd' + (act ? ' act' : '') + '" data-pad="' + p +
      '" aria-pressed="' + act + '" aria-label="' + PAD_NAMES[p] + (c ? '（已連線）' : '（未連線）') + '"><span class="live' + (c ? '' : ' idle') + '"></span><span>' +
      PAD_NAMES[p] + '</span></button>' +
      (pwr ? (nano
        ? '<button type="button" class="pwr" disabled aria-label="斷開 ' + PAD_NAMES[p] + '（無法使用）" title="Xbox Nano 無法從電腦關機：請長按 Xbox 鍵約 6 秒">' + ICON.power + '</button>'
        : '<button type="button" class="pwr" data-act="cut" data-cutpad="' + p + '" aria-label="斷開 ' + PAD_NAMES[p] + '" title="斷開手把">' + ICON.power + '</button>') : '') + '</span>';
  }
  // The DualSense touchpad-cursor switch sits left of 連線震動 (reskin Q18), so the other controls keep
  // their place when it comes and goes; it locks with the board outside 鍵鼠 (events.js LOCKED).
  h += '</div><div class="sp"></div>' + (pad === 'dualsense' ? switchHTML('tp', '觸控板游標', cfg.touchpad_cursor_enabled !== false) : '') +
    switchHTML('rumble', '連線震動', cfg.rumble_on_connect !== false) + omodeHTML(st, pad) + bangHTML() +
    '<button type="button" class="gear" data-act="setopen" aria-label="設定" title="設定">' + ICON.gear + '</button></header>';
  h += '<div class="stage"></div>' + padSvg(pad) + lbarHTML(st);
  const om = outputMode(st, pad);
  if (om === 'off') h += '<div class="obanner gold" role="status">' + ICON.pause + '<span>' + OFF_BANNER + '</span></div>';
  else if ((om === 'xbox' || om === 'kbm') && autoOffBanner()) h += '<div class="obanner white ao" role="status"><span>' + autoOffBanner() + '</span></div>';
  else if ((om === 'xbox' || om === 'kbm') && reconnectBanner()) h += '<div class="obanner white rc" role="status"><span>' + reconnectBanner() + '</span>' + (reconnectDismissable() ? '<button type="button" class="mini" data-act="rcdismiss">這次不再提醒</button>' : '') + '</div>';
  else if ((om === 'pad' || om === 'xbox') && !vigemBlocksBanner()) h += '<div class="obanner white" role="status"><span>' + padBanner(om, pad) + '</span></div>';
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
  h += capHTML(st) + lbPopHTML(st);
  h += popHTML();
  if (st.medit) h += editorHTML(st);
  if (st.setOpen) h += settingsHTML(st);
  const lk = boardLocked(st), dis = on => (on && !lk ? '' : ' disabled');
  h += '<footer class="ftr"><button type="button" class="b-def" data-act="reset"' + dis(true) + '>恢復預設</button><button type="button" class="b-clr" data-act="clear"' + dis(true) + '>全部清除</button>' +
    '<span class="note">' + esc(st.note) + '</span><div class="sp"></div>' +
    '<button type="button" class="b-hist" data-act="undo" title="復原"' + dis(st.canUndo) + '>' + ICON.undo + '復原</button>' +
    '<button type="button" class="b-hist redo" data-act="redo" title="重做"' + dis(st.canRedo) + '><span class="flip">' + ICON.undo + '</span>重做</button></footer>';
  return h;
}

export { slotsOf };
