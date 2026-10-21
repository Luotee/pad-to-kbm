// DualSense light bar on the board: the 燈條 dropdown in the caption row, its mode list, the 自訂
// panel (鍵鼠時 / 非鍵鼠時), the K2 colour picker, and the two strips drawn on the pad art. Display + IPC
// only: every change is one set_lightbar; Rust validates it and owns the output reports and the
// animation clock (iron rule 9). The strips are a CSS preview on the same periods.

import { invoke } from '../ipc.js';
import { state, rerender } from './store.js';
import { hook } from './events.js';
import { outputMode } from './view.js';
import { pixelSvg, withRing } from './pixel.js';
import { SPEEDS, SPEED_TXT, PRESETS, hueCells, satCells, pickerIndex, normHex, hsvHex } from './lightcolor.js';

// open: the dropdown (list, and the panel when the mode is 自訂); pick: the state whose K2 picker is open.
// drag: a press on a hue / saturation strip in progress ({ w, strip, hi, si, color, rect, id }); nothing is sent until release.
export const LB = { open: false, pick: null, drag: null, suppress: false };

const STATE_DEF = { effect: 'off', color: '#FFFFFF', speed: 'medium', rainbow_breath: false, brightness: 80 };
export function lightOf(cfg) {
  const l = (cfg && cfg.lightbar) || {};
  return { mode: l.mode || 'custom', kbm: { ...STATE_DEF, effect: 'rainbow', ...l.kbm }, other: { ...STATE_DEF, ...l.other } };
}
// What is drawn: the saved light, with the colour of an in-progress strip drag laid over it.
function lightView(cfg) {
  const l = lightOf(cfg), d = LB.drag;
  if (d) l[d.w] = { ...l[d.w], color: d.color };
  return l;
}
const same = (a, b) => String(a).toUpperCase() === String(b).toUpperCase();
const speedIx = s => Math.max(0, SPEEDS.indexOf(s.speed));

// ─── Pad art: the two strips beside the touchpad ─────────────────────────────
const BREATH_S = [6.0, 3.0, 1.5, 0.8], CYCLE_S = [12.0, 6.0, 3.0, 1.5];
function shown(lb, om) {
  if (lb.mode === 'native') return { kind: 'none' };
  if (lb.mode === 'off') return { kind: 'off' };
  const s = om === 'kbm' ? lb.kbm : lb.other, i = speedIx(s);
  if (s.effect === 'off') return { kind: 'off' };
  if (s.effect === 'rainbow') return s.rainbow_breath ? { kind: 'rainstep', p: BREATH_S[i], b: s.brightness } : { kind: 'rainbow', p: CYCLE_S[i], b: s.brightness };
  return { kind: s.effect, color: s.color, p: BREATH_S[i], b: s.brightness };
}

export function lbarHTML(st) {
  if (st.pad !== 'dualsense') return '';
  const s = shown(lightView(st.config), outputMode(st, 'dualsense'));
  let fill = '#c9ceda', cls = 'lbar', style = '', dim = '';
  if (s.kind === 'off') fill = '#454c60';
  else if (s.kind !== 'none') {
    fill = s.kind === 'rainbow' || s.kind === 'rainstep' ? '#FF2A2A' : s.color;
    cls += ' lit' + (s.kind === 'fixed' ? '' : ' ' + s.kind);
    // The board re-renders on every press: keep the animation's phase on one clock.
    const now = performance.now() / 1000, P = s.kind === 'rainstep' ? s.p * 6 : s.p;
    // 亮度 on an outer group, so the breathing opacity on the inner one does not override it.
    dim = ' style="opacity:' + (0.2 + 0.8 * s.b / 100).toFixed(2) + '"';
    style = 'color:' + fill + ';animation-duration:' + (s.kind === 'rainstep' ? P + 's,' + s.p + 's' : s.p + 's') +
      ';animation-delay:-' + (now % P).toFixed(3) + 's' + (s.kind === 'rainstep' ? ',-' + (now % s.p).toFixed(3) + 's' : '');
  }
  // Design G (no outline, no blur): lit strips get two stepped rings of the colour (.18, .38), every state a 3 px core
  // and a 1 px pale centre. 關燈 / 原廠燈 have no rings and no visible centre line.
  const ring = !(s.kind === 'off' || s.kind === 'none'), core = ring ? 'color-mix(in srgb,' + fill + ' 45%,#fff)' : fill;
  const strip = c => '<g shape-rendering="crispEdges">' +
    (ring ? '<rect x="' + (c - 2) + '" y="44" width="7" height="50" fill="' + fill + '" opacity=".18"/><rect x="' + (c - 1) + '" y="45" width="5" height="48" fill="' + fill + '" opacity=".38"/>' : '') +
    '<rect x="' + c + '" y="46" width="3" height="46" fill="' + fill + '"/><rect x="' + (c + 1) + '" y="47" width="1" height="44" fill="' + core + '"/></g>';
  return '<svg class="lbarsvg" width="300" height="210" viewBox="0 0 300 210" aria-hidden="true"><g' + dim + '><g class="' + cls + '" style="' + style + '">' + strip(103) + strip(194) + '</g></g></svg>';
}

// ─── The dropdown in the caption row ─────────────────────────────────────────
// The reference's notched 7x4 pixel arrow (D-Pad Slots ▼); the right arrow of the keys and the 自訂 row.
const ARROW = pixelSvg(['###.###', '.#####.', '..###..', '...#...'], { '#': 'currentColor' });
const ICON_L = pixelSvg(withRing(['...#', '..##', '.###', '####', '.###', '..##', '...#']), { '#': 'currentColor', r: 'iol' });
const ICON_R = pixelSvg(withRing(['#...', '##..', '###.', '####', '###.', '##..', '#...']), { '#': 'currentColor', r: 'iol' });
const MODES = [['off', '關閉'], ['native', '原廠燈'], ['custom', '自訂']];
const FX = [['off', '關燈'], ['fixed', '固定'], ['breathe', '呼吸'], ['rainbow', '彩虹']];

export function capHTML(st) {
  // An open macro editor covers the caption row: no 燈條 control over it, and the dropdown is closed.
  if (st.medit) { LB.open = false; LB.pick = null; }
  if (st.pad !== 'dualsense' || st.medit) return '';
  const lb = lightOf(st.config), text = (MODES.find(m => m[0] === lb.mode) || MODES[2])[1];
  return '<div class="lbcap"><span class="lbname">燈條</span><button type="button" class="lbdd" data-act="lbopen" aria-haspopup="dialog" aria-expanded="' + LB.open +
    '" aria-label="燈條：' + text + '"><span class="lbtxt">' + text + '</span><i class="lbsep"></i><span class="lbarr">' + ARROW + '</span></button></div>';
}

// ─── The list, the 自訂 panel and the K2 picker ──────────────────────────────
function colors(w, s) {
  let h = '<div class="lbslots">';
  for (const [nm, hex] of PRESETS) h += '<button type="button" class="lbslot" data-act="lbset" data-which="' + w + '" data-k="color" data-v="' + hex + '" aria-label="' + nm + '" title="' + nm + '" aria-pressed="' + same(hex, s.color) + '"><i style="background:' + hex + '"></i></button>';
  const own = !PRESETS.some(p => same(p[1], s.color));
  return h + '<button type="button" class="lbslot lbadd" data-act="lbpickopen" data-which="' + w + '" title="自訂" aria-label="自訂顏色" aria-expanded="' + (LB.pick === w) + '" aria-pressed="' + (own || LB.pick === w) + '">' +
    (own ? '<i style="background:' + s.color + '"></i>' : '') + '</button></div>';
}
const step = (w, k, text, label) => '<span class="lbstep" role="group" aria-label="' + label + '"><button type="button" class="lbkey" data-act="lbstep" data-which="' + w + '" data-k="' + k + '" data-d="-1" aria-label="' + label + '減少">' + ICON_L + '</button>' +
  '<span class="lbval">' + text + '</span><button type="button" class="lbkey" data-act="lbstep" data-which="' + w + '" data-k="' + k + '" data-d="1" aria-label="' + label + '增加">' + ICON_R + '</button></span>';
const opt = (l, c) => '<div class="lbopt"><span class="lbl">' + l + '</span>' + c + '</div>';

// One state: side plate with the title, then 4 fixed rows (effect / A / B / 亮度); 關燈 merges A..亮度 into one grey line.
function section(lb, w, title) {
  const s = lb[w], speed = SPEED_TXT[speedIx(s)];
  const fx = '<span class="qsg lbfx" role="group" aria-label="效果">' + FX.map(([id, nm]) => '<button type="button" data-act="lbset" data-which="' + w + '" data-k="effect" data-v="' + id + '" aria-pressed="' + (s.effect === id) + '">' + nm + '</button>').join('') + '</span>';
  const bright = '<div class="lbb">' + opt('亮度', step(w, 'brightness', s.brightness + '%', '亮度')) + '</div>';
  let body;
  if (s.effect === 'off') body = '<div class="lbabc"><p class="lbgrey">燈條熄滅</p></div>';
  else if (s.effect === 'fixed') body = '<div class="lba">' + colors(w, s) + '</div><div class="lbb"></div>' + bright;
  else if (s.effect === 'breathe') body = '<div class="lba">' + colors(w, s) + '</div><div class="lbb">' + opt('呼吸速度', step(w, 'speed', speed, '速度')) + '</div>' + bright;
  else body = '<div class="lba">' + opt('呼吸換色', step(w, 'rainbow_breath', s.rainbow_breath ? '開' : '關', '呼吸換色')) + '</div><div class="lbb">' + opt(s.rainbow_breath ? '呼吸速度' : '換色速度', step(w, 'speed', speed, '速度')) + '</div>' + bright;
  return '<div class="lbsec"><span class="lbvt" style="padding-top:' + (title.length === 3 ? 30 : 24) + 'px">' + [...title].map(c => '<span>' + c + '</span>').join('') + '</span><div class="lbcol">' + fx + body + '</div></div>';
}

// K2: hue and saturation strips (24 cells each) dropped from the 自訂 slot, then the preview and the 色碼 field.
function pickerHTML(lb, w) {
  const s = lb[w], d = LB.drag, { hi, si } = d ? d : pickerIndex(s.color), shade = hueCells(23), val = hueCells(si), sat = satCells(hi);
  const cell = (cls, v, bg, label, on) => '<button type="button" class="' + cls + '" data-act="lbset" data-which="' + w + '" data-k="color" data-v="' + v + '" aria-label="' + label + '" aria-pressed="' + on + '" style="background:' + bg + '"></button>';
  const hb = val.map((v, k) => cell('lbhb', v, shade[k], '色相 ' + k * 15, k === hi)).join('');
  const sb = sat.map((v, k) => cell('lbsb', v, v, '飽和度 ' + (k + 1), k === si)).join('');
  return '<section class="lbcpop" style="top:' + (w === 'kbm' ? 165 : 265) + 'px" role="dialog" aria-label="自訂顏色">' +
    '<div class="lbopt2"><span class="lbl">色相</span><div class="lbbar" data-strip="hue" data-which="' + w + '">' + hb + '</div></div><div class="lbopt2"><span class="lbl">飽和</span><div class="lbbar" data-strip="sat" data-which="' + w + '">' + sb + '</div></div>' +
    '<div class="lbcrow"><i class="lbprev" style="background:' + s.color + '"></i><input class="lbhexin" data-lbhex="' + w + '" value="' + s.color + '" maxlength="7" aria-label="色碼"></div></section>';
}

export function popHTML(st) {
  if (st.pad !== 'dualsense' || st.medit || !LB.open) return '';
  const lb = lightView(st.config), i = Math.max(0, MODES.findIndex(m => m[0] === lb.mode));
  const list = '<div class="lbdl" role="listbox" aria-label="燈條模式">' + MODES.map(([id, n]) => '<button type="button" class="lbli" role="option" data-act="lbmode" data-v="' + id +
    '" aria-pressed="' + (lb.mode === id) + '" aria-selected="' + (lb.mode === id) + '">' + n + (id === 'custom' ? '<span class="lbsub">' + ICON_R + '</span>' : '') + '</button>').join('') + '</div>';
  if (lb.mode !== 'custom') return list;
  // The chosen row's blue bridges the gap into the panel's blue left edge.
  const bridge = '<i class="lbbridge" style="top:' + (99 + 2 + i * 18) + 'px"></i>';
  const pick = LB.pick && (lb[LB.pick].effect === 'fixed' || lb[LB.pick].effect === 'breathe') ? pickerHTML(lb, LB.pick) : '';
  return list + bridge + '<section class="lbpop" role="dialog" aria-label="燈條設定"><div class="lbside">' + section(lb, 'kbm', '鍵鼠時') + section(lb, 'other', '非鍵鼠時') + '</div></section>' + pick;
}

// ─── Behaviour ───────────────────────────────────────────────────────────────
async function save(lightbar) {
  try { await invoke('set_lightbar', { lightbar }); } catch (e) { state.note = '無法套用：' + String(e && e.message ? e.message : e); rerender(); }
}
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const close = () => { LB.open = false; LB.pick = null; rerender(); };

export function startLightbar(board) {
  // Click outside closes. Capture phase, so it also runs on a locked board, whose gate (events.js) stops the click.
  board.addEventListener('click', ev => {
    // The click that ends a strip drag lands on whatever the re-render left under the pointer: not a click outside, not a pick.
    if (LB.suppress) { LB.suppress = false; ev.stopPropagation(); return; }
    if (LB.open && !ev.target.closest('.lbpop, .lbdl, .lbcpop, [data-act="lbopen"]')) close();
  }, true);
  hook('click', ev => {
    const el = ev.target.closest('[data-act]'), a = el && el.dataset.act;
    if (a === 'lbopen') { if (LB.open) close(); else { LB.open = true; rerender(); } return true; }
    if (!LB.open || !ev.target.closest('.lbpop, .lbdl, .lbcpop')) return false;
    const lb = lightOf(state.config), w = el && el.dataset.which;
    if (a === 'lbmode') save({ ...lb, mode: el.dataset.v });
    else if (a === 'lbpickopen') { LB.pick = LB.pick === w ? null : w; rerender(); }
    else if (a === 'lbset') { if (el.dataset.k === 'effect' && LB.pick === w) LB.pick = null; save({ ...lb, [w]: { ...lb[w], [el.dataset.k]: el.dataset.v } }); }
    else if (a === 'lbstep') {
      const cur = { ...lb[w] }, k = el.dataset.k, d = +el.dataset.d;
      if (k === 'speed') cur.speed = SPEEDS[clamp(speedIx(cur) + d, 0, 3)];
      else if (k === 'brightness') cur.brightness = clamp(cur.brightness + 10 * d, 10, 100);
      else if (k === 'rainbow_breath') cur.rainbow_breath = !cur.rainbow_breath;
      save({ ...lb, [w]: cur });
    }
    return true;
  });
  // Hue / saturation strips: press picks the cell under the pointer, moving while held keeps picking, and release
  // sends the one set_lightbar. The pointer is captured on the board, which survives the re-renders the drag causes.
  const pickAt = ev => {
    const d = LB.drag, k = clamp(Math.floor((ev.clientX - d.rect.left) / d.rect.width * 24), 0, 23);
    if (d.strip === 'hue') d.hi = k; else d.si = k;
    const color = hsvHex(d.hi * 15, (d.si + 1) / 24, 1);
    if (color !== d.color) { d.color = color; rerender(); }
  };
  board.addEventListener('pointerdown', ev => {
    const bar = ev.button === 0 && !LB.drag && LB.open && ev.target.closest && ev.target.closest('.lbcpop .lbbar[data-strip]');
    if (!bar) return;
    const w = bar.dataset.which, { hi, si } = pickerIndex(lightOf(state.config)[w].color), r = bar.getBoundingClientRect();
    LB.drag = { w, strip: bar.dataset.strip, hi, si, color: '', rect: { left: r.left, width: r.width }, id: ev.pointerId };
    ev.preventDefault();
    try { board.setPointerCapture(ev.pointerId); } catch (e) { /* no capture: the move still tracks while over the board */ }
    pickAt(ev);
  });
  board.addEventListener('pointermove', ev => { if (LB.drag && ev.pointerId === LB.drag.id) pickAt(ev); });
  const release = ev => {
    const d = LB.drag;
    if (!d || ev.pointerId !== d.id) return;
    LB.drag = null;
    LB.suppress = true;
    setTimeout(() => { LB.suppress = false; }, 100);
    const lb = lightOf(state.config);
    if (!same(d.color, lb[d.w].color)) save({ ...lb, [d.w]: { ...lb[d.w], color: d.color } }); else rerender();
  };
  for (const kind of ['pointerup', 'pointercancel', 'lostpointercapture']) board.addEventListener(kind, release);
  // 色碼: six hex digits are sent; anything else puts the field back to the current colour.
  board.addEventListener('change', ev => {
    const w = ev.target.dataset && ev.target.dataset.lbhex;
    if (!w) return;
    const v = normHex(ev.target.value), lb = lightOf(state.config);
    if (v) save({ ...lb, [w]: { ...lb[w], color: v } }); else rerender();
  });
}
