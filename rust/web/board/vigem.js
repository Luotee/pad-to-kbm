// Xbox output prompt (v3.6.0): ViGEmBus for every pad, plus HidHide for a pad
// Windows can read itself (DualSense / Switch Pro), so the game does not see it
// twice. Display + IPC only: Rust probes, downloads, verifies and runs the
// official installers and hides the pad (iron rules 7 and 9).

import { invoke, listen } from '../ipc.js';
import { state, rerender, onPadChange } from './store.js';
import { hook } from './events.js';
import { outputMode, hasNative } from './view.js';
import * as act from './actions.js';
import { ICON } from './icons.js';

// status: the selected pad's get_xbox_status reply { vigem, hidhide, hidden }, null until asked.
// stale: a controller-status / config-changed came in, so `hidden` may have changed.
export const VG = { status: null, probing: false, again: false, stale: false, pop: false, busy: false, err: '' };
const NAME = { dualsense: 'DualSense', ns_pro: 'Switch Pro', ns2_pro: 'Switch 2 Pro' };
const DRIVER = { vigem: { name: 'ViGEmBus', cmd: 'install_vigem' }, hidhide: { name: 'HidHide', cmd: 'install_hidhide' } };

const xbox = () => outputMode(state, state.pad) === 'xbox';
// What this pad still needs: ViGEmBus always, HidHide only when Windows can read the pad itself.
function missing() {
  const s = VG.status;
  if (!s) return [];
  const m = [];
  if (s.vigem !== 'installed') m.push('vigem');
  if (hasNative(state.pad) && s.hidhide !== 'installed') m.push('hidhide');
  return m;
}

// Ask Rust about the selected pad. `open`: a pick of Xbox or the first look at a pad in Xbox
// mode, where anything missing opens the popover; otherwise only the status is refreshed (the
// reconnect banner) and a popover the user closed stays closed.
export async function probe({ open = true } = {}) {
  // One probe at a time; a pick of Xbox during one asks again when it returns.
  if (VG.probing) { if (open) VG.again = true; return; }
  VG.probing = true;
  VG.stale = false;
  const kind = state.pad;
  let s = null;
  try { s = await invoke('get_xbox_status', { kind }); } catch (e) { if (open) s = { vigem: 'missing', hidhide: 'missing', hidden: null }; }
  VG.probing = false;
  if (VG.again) { VG.again = false; return probe(); }
  if (state.pad !== kind) { VG.stale = true; rerender(); return; }   // the answer is for a pad no longer shown
  if (s) VG.status = s;
  if (open) { VG.pop = missing().length > 0; VG.err = ''; }
  else if (!missing().length) VG.pop = false;
  rerender();
}

// Header extras: the gold ! (popover closed, something missing or the live
// `vigem_missing` warning) and the popover itself.
export function bangHTML() {
  if (!xbox() || VG.pop) return '';
  const live = (state.warnings || []).includes('vigem_missing');
  if (!missing().length && !live) return '';
  return '<button type="button" class="vgbang" data-act="vgopen" aria-label="需要安裝" title="需要安裝">!</button>';
}

function row(id, what) {
  const ok = !!VG.status && VG.status[id] === 'installed';
  return '<li class="' + (ok ? 'ok' : 'no') + '"><span class="vgn">' + DRIVER[id].name + '</span><span class="vgw">' + what + '</span><span class="vgs">' + (ok ? '已安裝' : '未安裝') + '</span></li>';
}

export function popHTML() {
  if (!xbox() || !VG.pop) return '';
  const nat = hasNative(state.pad), need = missing(), pad = NAME[state.pad];
  const go = VG.busy ? '安裝中…' : need.length > 1 ? '安裝這兩個' : '安裝 ' + DRIVER[need[0] || 'vigem'].name;
  return '<div class="vgpop" role="dialog" aria-label="需要安裝">' +
    '<div><b>要讓遊戲把 ' + pad + ' 當成 Xbox 手把，需要' + (nat ? '這兩個' : '') + '：</b></div>' +
    '<ul class="vgl">' + row('vigem', '產生一支 Xbox 手把給遊戲') +
    (nat ? row('hidhide', '把原本的 ' + pad + ' 對遊戲藏起來') : '') + '</ul>' +
    '<div class="vgnote">' + (need.length > 1 ? '都是官方安裝程式，各約 1 分鐘' : '官方安裝程式，約 1 分鐘') + '，會要求系統管理員權限。裝好之前，這支手把不會送出任何東西。</div>' +
    (VG.err ? '<div class="vgerr">' + VG.err + '</div>' : '') +
    '<div class="vgbtns"><button type="button" class="mini" data-act="vglater"' + (VG.busy ? ' disabled' : '') + '>稍後</button>' +
    '<button type="button" class="mini" data-act="vgkbm"' + (VG.busy ? ' disabled' : '') + '>改回鍵鼠</button>' +
    '<button type="button" class="mini go" data-act="vginstall"' + (VG.busy ? ' disabled' : '') + '>' + go + '</button></div></div>';
}

// The banner under the header is hidden while something is known missing:
// the popover / ! says what is wrong instead.
export function vigemBlocksBanner() {
  return xbox() && (missing().length > 0 || (state.warnings || []).includes('vigem_missing'));
}

// HidHide hides a pad only from the next time it connects (`hidden: false` = not yet).
export function reconnectBanner() {
  if (!xbox() || !hasNative(state.pad) || missing().length || !VG.status || VG.status.hidden !== false) return '';
  return '再連一次 ' + NAME[state.pad] + '，遊戲才看不到原本的手把：按分頁上的 ' + ICON.power + ' 斷開，再按 ' + (state.pad === 'dualsense' ? 'PS' : 'HOME') + ' 鍵連回來';
}

// Each missing driver in turn (ViGEmBus first), then the status again. A failure
// keeps the popover open with the reason.
async function install() {
  const kind = state.pad;
  VG.busy = true; VG.err = ''; rerender();
  try {
    for (const id of missing()) {
      const s = await invoke(DRIVER[id].cmd);
      VG.status = { ...VG.status, [id]: s };
      if (s !== 'installed') { VG.err = DRIVER[id].name + ' 還沒裝好，可以再試一次（狀態：' + s + '）'; break; }
    }
  } catch (e) {
    VG.err = '安裝失敗：' + String(e && e.message ? e.message : e);
  }
  if (!VG.err) {
    try { VG.status = await invoke('get_xbox_status', { kind }); } catch (e) { /* keep the installers' answers */ }
    if (missing().length) VG.err = '還沒裝好，可以再試一次';
    else VG.pop = false;
  }
  VG.busy = false;
  rerender();
}

export async function startVigem() {
  // A pick of Xbox re-probes; so does opening the board on a pad in Xbox mode
  // (first paint with no answer yet, maybeProbe).
  act.onModeChange((pad, mode) => { if (mode === 'xbox') { VG.status = null; probe(); } else VG.pop = false; });
  // Another tab: the drivers are the same, `hidden` is per pad (refreshed on the next paint).
  onPadChange(() => {
    VG.pop = false; VG.err = '';
    if (VG.status) VG.status = { ...VG.status, hidden: null };
    VG.stale = true;
  });
  // A reconnect (controller-status) or a mode / config change can clear the reconnect banner.
  const touch = () => { VG.stale = true; maybeProbe(); };
  await listen('controller-status', touch);
  await listen('config-changed', touch);
  hook('click', ev => {
    const el = ev.target.closest('[data-act]');
    if (!el) return false;
    const a = el.dataset.act;
    if (a === 'vgopen') { VG.pop = true; VG.err = ''; rerender(); return true; }
    if (a === 'vglater') { VG.pop = false; rerender(); return true; }
    if (a === 'vgkbm') { VG.pop = false; act.setMode('kbm'); return true; }
    if (a === 'vginstall') { if (!VG.busy) install(); return true; }
    return false;
  });
}

// Called on every paint: the first look at a pad in Xbox mode, or a refresh after an event.
// One probe at a time; none while installing (install re-probes itself).
export function maybeProbe() {
  if (!xbox() || VG.probing || VG.busy) return;
  if (VG.status === null) probe();
  else if (VG.stale) probe({ open: false });
}
