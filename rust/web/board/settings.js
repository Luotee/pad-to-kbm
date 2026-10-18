// The settings window (spec 4.7): modal, four tabs by what is adjusted,
// 取消 / 儲存 close it. Ported from the mockup's settingsHTML (layout a).

import { invoke, listen } from '../ipc.js';
import { state, rerender } from './store.js';
import { esc, tagSvg, switchHTML } from './tags.js';
import { layoutFor, slotsOf } from './layout.js';
import { ICON } from './icons.js';
import { winKey } from '../chrome.js';
import { hook, selectSlot } from './events.js';
import { settingsFrom } from './actions.js';
import { PAD_NAMES, keyToTile } from './model.js';
import { LABEL } from './keys.js';

const TABS = [['gen', '一般'], ['stick', '搖桿'], ['tp', '觸控板'], ['timing', '按鍵']];
let badFields = [];

// ─── Recent events (a 40-row ring, newest first on screen) ───────────────────
const LOG = [];
function stamp(ms) {
  const d = new Date(ms || Date.now()), p = (n, w = 2) => String(n).padStart(w, '0');
  return p(d.getHours()) + ':' + p(d.getMinutes()) + ':' + p(d.getSeconds()) + '.' + p(d.getMilliseconds(), 3);
}
// `src`: what sent it — { button: id } or { macro: name } (v3.2.0); the
// chip is drawn for the pad selected when the row is shown.
function log(kind, text, ms, src = null) {
  LOG.push([stamp(ms), kind, text, src]);
  if (LOG.length > 40) LOG.shift();
  const box = document.querySelector('#board #logrows');
  if (box) box.innerHTML = logRows(6);
}
// The board's corner tag of each button id, per pad (both triggers as
// single slots so 23 / 24 have one).
const TAGS = {};
function tagOf(pad, id) {
  if (!TAGS[pad]) {
    TAGS[pad] = {};
    for (const sl of slotsOf(layoutFor(pad, { l: 'key', r: 'key' }))) if (/^\d+$/.test(sl.key)) TAGS[pad][sl.key] = sl.tag;
  }
  return TAGS[pad][String(id)];
}
function chipHTML(src) {
  if (!src) return '';
  if (src.macro !== undefined) return '<span class="chip mac">' + esc(src.macro) + '</span>';
  if (src.button === undefined) return '';
  const tag = tagOf(state.pad, src.button);
  if (!tag) return '<span class="chip pill">' + esc(String(src.button)) + '</span>';
  const face = src.button >= 0 && src.button <= 3;
  // Drawn tags (PlayStation symbols, stick / touchpad icons) as the board
  // draws them; text tags (L1, A, Share…) as plain text.
  const drawn = /^(PS|S|T):/.test(tag);
  return '<span class="chip ' + (face ? 'face' : 'pill') + (drawn ? '' : ' txt') + '">' + (drawn ? tagSvg(tag) : esc(tag)) + '</span>';
}
function logRows(n) {
  return LOG.slice(-n).reverse().map(r => {
    const chip = chipHTML(r[3]);
    return '<div class="lr"><span class="lt">' + r[0] + '</span><span class="lk k-' + r[1] + '">' + r[1] + '</span><span class="lx">' +
      (chip ? chip + '<span class="arr">→</span>' : '') + esc(r[2]) + '</span></div>';
  }).join('');
}

// ─── Touchpad finger (from touchpad-hover / touchpad-click raw positions) ────
let finger = null;
function paintTouchpad() {
  const t = document.querySelector('#board #tpbox');
  if (t && state.draft) t.innerHTML = tpHTML(state.draft, 1);
  const b = document.querySelector('#board [data-act="tpmid"]');
  if (b) b.disabled = !finger;
}

// ─── Draft <-> config ────────────────────────────────────────────────────────
function draftFrom(c) {
  return {
    deadzone: c.deadzone, trigger: c.trigger_threshold, tpSens: c.touchpad_cursor_sensitivity, press: [...c.min_press_ms], jitter: [...c.tick_jitter_ms],
    midX: c.touchpad_midpoint_x, midY: c.touchpad_midpoint_y, slowThr: c.touchpad_accel_slow_threshold, fastThr: c.touchpad_accel_fast_threshold,
    gainSlow: c.touchpad_accel_gain_slow, gainFast: c.touchpad_accel_gain_fast, tpDead: c.touchpad_deadzone_radius,
    freeze: c.touchpad_click_freeze_enabled, log: c.log_events, tpOn: c.touchpad_cursor_enabled,
  };
}
function settingsOf(d) {
  return {
    ...settingsFrom(state.config),
    deadzone: d.deadzone, trigger_threshold: d.trigger, touchpad_cursor_sensitivity: d.tpSens, min_press_ms: d.press, tick_jitter_ms: d.jitter,
    touchpad_midpoint_x: d.midX, touchpad_midpoint_y: d.midY, touchpad_accel_slow_threshold: d.slowThr, touchpad_accel_fast_threshold: d.fastThr,
    touchpad_accel_gain_slow: d.gainSlow, touchpad_accel_gain_fast: d.gainFast, touchpad_deadzone_radius: d.tpDead,
    touchpad_click_freeze_enabled: d.freeze, log_events: d.log, touchpad_cursor_enabled: d.tpOn,
  };
}

function badRange(d, strict) { return !(d[0] >= 0 && d[1] >= 0) || (strict ? d[0] >= d[1] : d[0] > d[1]); }

// Same rules as Config::validate. Returns [message, fields, tab] or null.
export function settingsError(d) {
  if (badRange(d.press, true)) return ['按住維持下限：前面要比後面小', ['press0', 'press1'], 'timing'];
  if (badRange(d.jitter, false)) return ['按鈕反應隨機延遲：前面不能比後面大', ['jitter0', 'jitter1'], 'timing'];
  if (!(d.tpSens >= 0.1 && d.tpSens <= 10)) return ['游標速度要在 0.1 到 10 之間', ['tpSens'], 'tp'];
  if (!(d.midX >= 1 && d.midX <= 4094)) return ['中心點 X 要在 1 到 4094 之間', ['midX'], 'tp'];
  if (!(d.midY >= 1 && d.midY <= 4094)) return ['中心點 Y 要在 1 到 4094 之間', ['midY'], 'tp'];
  if (!(d.slowThr < d.fastThr)) return ['慢速門檻要比快速門檻小', ['slowThr', 'fastThr'], 'tp'];
  if (!(d.gainSlow >= 0.1 && d.gainSlow <= 1)) return ['慢速倍率要在 0.1 到 1.0 之間', ['gainSlow'], 'tp'];
  if (!(d.gainFast >= 1 && d.gainFast <= 5)) return ['快速倍率要在 1.0 到 5.0 之間', ['gainFast'], 'tp'];
  if (!(d.tpDead >= 0 && d.tpDead <= 50)) return ['靜止不動範圍要在 0 到 50 之間', ['tpDead'], 'tp'];
  return null;
}

// ─── HTML (mockup helpers) ───────────────────────────────────────────────────
function trSvg(v) { return '<svg width="46" height="46" viewBox="0 0 46 46" aria-hidden="true"><path d="M8 40V14q0-10 15-10t15 10v26Z" fill="#f6f7fa" stroke="#454c60" stroke-width="2.5"/><rect data-vis="trigger" x="10" y="' + (6 + v * 32).toFixed(1) + '" width="26" height="3" fill="#c4232c"/></svg>'; }
function numF(f, v, step, lab) { return '<input class="num" type="number" step="' + step + '" data-f="' + f + '" value="' + v + '" aria-label="' + lab + '" aria-invalid="' + badFields.includes(f) + '">'; }
function slider(f, lab, min, max, step, v, out) { return '<label class="sl"><span class="lbl">' + lab + '</span><input class="rng" type="range" min="' + min + '" max="' + max + '" step="' + step + '" value="' + v + '" data-f="' + f + '" aria-label="' + lab + '"><output data-out="' + f + '">' + out + '</output></label>'; }
function sw(act, label, on) { return switchHTML(act, label, on, { cls: 'lt' }); }
function tpHTML(d, k) {
  const W = 1920 * k / 10, H = 1080 * k / 10, mx = d.midX * k / 10, my = d.midY * k / 10;
  let h = '<svg width="' + W + '" height="' + H + '" viewBox="0 0 ' + W + ' ' + H + '" class="tpad" role="img" aria-label="觸控板手指位置">';
  h += '<rect x="1" y="1" width="' + (W - 2) + '" height="' + (H - 2) + '" rx="6" fill="#f6f7fa" stroke="#454c60" stroke-width="2.5"/>';
  h += '<path d="M' + mx + ',2V' + (H - 2) + 'M2,' + my + 'H' + (W - 2) + '" stroke="#a8587a" stroke-width="1.5" stroke-dasharray="4 3"/>';
  if (finger) h += '<circle cx="' + (finger[0] * k / 10) + '" cy="' + (finger[1] * k / 10) + '" r="5" fill="#ffcf2e" stroke="#7a5c00" stroke-width="2"/>';
  return h + '</svg>';
}
function tpFields(d) { return '<div class="tpxy">中心 X ' + numF('midX', d.midX, 1, '中心點 X') + ' Y ' + numF('midY', d.midY, 1, '中心點 Y') + '</div>'; }
function cfgBox() {
  return '<div class="cfgbox"><span class="cfgl">設定檔</span><code class="cfgp">' + esc(state.configPath) + '</code><div class="cfgbtns">' +
    '<button type="button" class="mini go" data-act="opencfg">' + ICON.file + '用記事本開啟</button><button type="button" class="mini" data-act="opendir">開啟所在資料夾</button>' +
    '<span class="cfgn">改完存檔，程式會自動套用</span></div></div>';
}

// Extra panes the calibration feature fills in (#stkbox / #calbar).
const stickExtras = { stk: () => '', bar: () => '' };
export function setStickExtras(stk, bar) { stickExtras.stk = stk; stickExtras.bar = bar; }

function pane(d, t) {
  if (t === 'gen') return cfgBox() + sw('slog', '記錄每個按鍵事件，存成檔案', d.log) +
    '<div class="logbox"><span class="cfgl">最近的事件</span><div id="logrows">' + logRows(6) + '</div></div>' +
    '<p class="ver">PadToKBM 手把轉鍵鼠 v' + esc(state.version) + '<br>支援 DualSense、Switch Pro、Switch 2 Pro</p>';
  if (t === 'stick') return '<div class="two"><div id="stkbox">' + stickExtras.stk(d) + '</div><div class="col">' +
    slider('deadzone', '搖桿死區（粉紅圈）', 0, 0.9, 0.05, d.deadzone, d.deadzone.toFixed(2)) +
    '<div class="trrow">' + trSvg(d.trigger) + slider('trigger', '扳機按到多深算按下', 0.05, 0.95, 0.05, d.trigger, Math.round(d.trigger * 100) + '%') + '</div></div></div>' +
    '<div id="calbar">' + stickExtras.bar(d) + '</div>';
  if (t === 'tp') return '<div class="two"><div class="col tpcol"><div id="tpbox">' + tpHTML(d, 1) + '</div><button type="button" class="mini" data-act="tpmid"' + (finger ? '' : ' disabled') + '>中心設在手指位置</button><div id="tpxy">' + tpFields(d) + '</div></div><div class="col">' +
    sw('stp', '觸控板移動滑鼠游標', d.tpOn) + slider('tpSens', '游標速度', 0.1, 10, 0.1, d.tpSens, d.tpSens.toFixed(1) + '×') +
    '<div class="agrid g2"><label class="af">慢速／快速門檻<span class="pair">' + numF('slowThr', d.slowThr, 1, '慢速門檻') + numF('fastThr', d.fastThr, 1, '快速門檻') + '</span></label>' +
    '<label class="af">慢速／快速倍率<span class="pair">' + numF('gainSlow', d.gainSlow, 0.1, '慢速倍率') + numF('gainFast', d.gainFast, 0.1, '快速倍率') + '</span></label>' +
    '<label class="af">靜止不動範圍<span class="pair">' + numF('tpDead', d.tpDead, 1, '靜止不動範圍') + '</span></label></div>' +
    sw('sfreeze', '按下觸控板時游標不動', d.freeze) + '</div></div>';
  return '<p class="ver">程式替你按鍵時，每次的按住時間和延遲都會在下面的範圍內隨機變化，讓按鍵節奏像真人，避免被遊戲的防作弊系統誤判成外掛。</p><div class="agrid">' +
    '<label class="af">按住維持下限（毫秒）<span class="pair">' + numF('press0', d.press[0], 1, '按住最短') + '–' + numF('press1', d.press[1], 1, '按住最長') + '</span></label>' +
    '<label class="af">按鈕反應隨機延遲（毫秒）<span class="pair">' + numF('jitter0', d.jitter[0], 1, '延遲最短') + '–' + numF('jitter1', d.jitter[1], 1, '延遲最長') + '</span></label></div>';
}

export function settingsHTML(st) {
  const d = st.draft, t = st.setTab;
  let h = '<div class="ovl"><section class="setw" role="dialog" aria-modal="true" aria-labelledby="set-t">';
  h += '<div class="whd">' + ICON.gear + '<h2 id="set-t">設定</h2><div class="sp"></div><button type="button" class="x" data-act="setclose" aria-label="關閉">' + winKey('close', 24) + '</button></div><div class="wbody"><nav class="stabs" aria-label="設定分類">';
  for (const [id, label] of TABS) h += '<button type="button" data-stab="' + id + '" aria-pressed="' + (t === id) + '">' + label + '</button>';
  h += '</nav><div class="spane">' + pane(d, t) + '</div></div>';
  h += '<div class="wft"><span class="err" role="alert">' + esc(st.setErr) + '</span><div class="sp"></div><button type="button" class="b-cancel" data-act="setclose">取消</button><button type="button" class="b-ok" data-act="setsave">儲存</button></div></section></div>';
  return h;
}

// ─── Behaviour ───────────────────────────────────────────────────────────────
const saveHooks = [];
// Calibration adds its own save step (after set_settings succeeded).
export function onSettingsSave(f) { saveHooks.push(f); }
const tabHooks = [];
export function onSettingsTab(f) { tabHooks.push(f); }

function openSettings() {
  if (state.sel !== null) selectSlot(null);
  state.draft = draftFrom(state.config);
  state.setOpen = true;
  state.setErr = '';
  badFields = [];
  rerender();
  tabHooks.forEach(f => f(state.setTab, true));
  document.querySelector('#board [data-stab="' + state.setTab + '"]')?.focus();
}
function closeSettings() {
  state.setOpen = false;
  state.draft = null;
  state.setErr = '';
  badFields = [];
  tabHooks.forEach(f => f(state.setTab, false));
  rerender();
  document.querySelector('#board .gear')?.focus();
}
async function save() {
  const e = settingsError(state.draft);
  if (e) {
    state.setErr = e[0];
    badFields = e[1];
    state.setTab = e[2];
    rerender();
    return;
  }
  try {
    await invoke('set_settings', { settings: settingsOf(state.draft) });
    for (const f of saveHooks) await f();
  } catch (err) {
    state.setErr = String(err && err.message ? err.message : err);
    rerender();
    return;
  }
  state.note = '設定已儲存';
  closeSettings();
}

export async function startSettings(board) {
  hook('click', ev => {
    const el = ev.target.closest('[data-act],[data-stab]');
    if (el?.dataset.act === 'setopen') { openSettings(); return true; }
    if (!state.setOpen) return false;
    if (!el) return true;
    if (el.dataset.stab) {
      state.setTab = el.dataset.stab;
      rerender();
      tabHooks.forEach(f => f(state.setTab, true));
      board.querySelector('[data-stab="' + state.setTab + '"]')?.focus();
      return true;
    }
    const a = el.dataset.act, d = state.draft;
    if (a === 'setclose') closeSettings();
    else if (a === 'setsave') save();
    else if (a === 'opencfg') invoke('open_config_in_editor').catch(e => { state.setErr = String(e); rerender(); });
    else if (a === 'opendir') invoke('open_config_folder').catch(e => { state.setErr = String(e); rerender(); });
    else if (a === 'slog') { d.log = !d.log; rerender(); }
    else if (a === 'sfreeze') { d.freeze = !d.freeze; rerender(); }
    else if (a === 'stp') { d.tpOn = !d.tpOn; rerender(); }
    else if (a === 'tpmid' && finger) {
      d.midX = finger[0]; d.midY = finger[1];
      paintTouchpad();
      const xy = board.querySelector('#tpxy');
      if (xy) xy.innerHTML = tpFields(d);
    } else return el.dataset.act ? false : true;
    return true;
  });
  // Live edits that must not re-render (a re-render would drop the slider or caret).
  board.addEventListener('input', ev => {
    const f = ev.target.dataset?.f, d = state.draft;
    if (!state.setOpen || !f || !d) return;
    const n = +ev.target.value;
    if (f === 'deadzone' || f === 'trigger' || f === 'tpSens') {
      d[f] = n;
      const out = board.querySelector('[data-out="' + f + '"]');
      if (out) out.textContent = f === 'deadzone' ? n.toFixed(2) : f === 'trigger' ? Math.round(n * 100) + '%' : n.toFixed(1) + '×';
      const vis = board.querySelector('[data-vis="trigger"]');
      if (vis && f === 'trigger') vis.setAttribute('y', (6 + n * 32).toFixed(1));
      return;
    }
    if (/^(press|jitter)[01]$/.test(f)) d[f.slice(0, -1)][+f.slice(-1)] = n; else d[f] = n;
    if (f === 'midX' || f === 'midY') paintTouchpad();
    state.setErr = '';
    badFields = [];
    const er = board.querySelector('.setw .err');
    if (er) er.textContent = '';
    board.querySelectorAll('.setw .num[aria-invalid="true"]').forEach(x => x.setAttribute('aria-invalid', 'false'));
  });

  await listen('activity', p => {
    if (p.kind === 'emit') {
      const t = keyToTile(p.key), shown = LABEL[t] || t;   // the name printed on the palette key
      log(p.action === 'down' ? '按下' : '放開', '送出 ' + shown + (p.action === 'down' ? ' 按下' : ' 放開'), p.ts_ms, p.source || null);
    }
    else if (p.kind === 'macro-start') log('巨集', '巨集「' + p.name + '」開始', p.ts_ms, p.button === undefined ? null : { button: p.button });
    else if (p.kind === 'macro-end') log('巨集', '巨集「' + p.name + '」結束', p.ts_ms);
  });
  let lastKind = null;
  await listen('controller-status', s => {
    if (s.connected && s.kind) { if (s.kind !== lastKind) log('連線', (PAD_NAMES[s.kind] || s.kind) + ' 已連線'); lastKind = s.kind; }
    else if (lastKind) { log('連線', (PAD_NAMES[lastKind] || lastKind) + ' 已斷開'); lastKind = null; }
  });
  await listen('config-changed', () => log('設定', '設定已變更'));
  await listen('touchpad-hover', p => { finger = p.quadrant === 255 ? null : [p.raw_x, p.raw_y]; paintTouchpad(); });
  await listen('touchpad-click', p => { finger = [p.raw_x, p.raw_y]; paintTouchpad(); });
}
