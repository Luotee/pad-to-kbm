// Xbox output prompt (v3.6.0): ViGEmBus for every pad, plus HidHide for a pad
// Windows can read itself (DualSense / Switch Pro), so the game does not see it
// twice. 鍵鼠 hides such a pad too (HidHide only, no ViGEmBus): asked once on its
// own, then only the gold ! and a manual pick of 鍵鼠. Display + IPC only: Rust probes, downloads, verifies and runs the
// official installers and hides the pad (iron rules 7 and 9).

import { invoke, listen } from '../ipc.js';
import { state, rerender, onPadChange } from './store.js';
import { hook } from './events.js';
import { outputMode, hasNative } from './view.js';
import * as act from './actions.js';
import { ICON } from './icons.js';

// status: the selected pad's get_xbox_status reply { vigem, hidhide, hidden, auto_off }, null until asked.
// stale: a controller-status / config-changed came in, so `hidden` may have changed.
export const VG = { status: null, probing: false, next: null, autoOpened: false, quiet: false, stale: false, pop: false, busy: false, err: '', rcDismissed: new Set() };
const NAME = { dualsense: 'DualSense', ns_pro: 'Switch Pro', ns2_pro: 'Switch 2 Pro', xbox_nano: 'Xbox Nano' };
// The key that turns the pad on (and connects it again).
const POWER_KEY = { dualsense: 'PS', xbox_nano: 'Xbox' };
const powerKey = pad => POWER_KEY[pad] || 'HOME';
const DRIVER = { vigem: { name: 'ViGEmBus', cmd: 'install_vigem' }, hidhide: { name: 'HidHide', cmd: 'install_hidhide' } };

// Which hiding the selected tab needs: Xbox mode, 鍵鼠 on a pad Windows can read itself, or none.
function hideMode() {
  const m = outputMode(state, state.pad);
  if (m === 'xbox') return 'xbox';
  if (m === 'kbm' && hasNative(state.pad)) return 'kbm';
  return null;
}
// What this pad still needs: Xbox = ViGEmBus always, plus HidHide when Windows can read the pad
// itself; 鍵鼠 = HidHide only. `mode` is passed by a caller whose config may not have caught up yet.
function missing(mode = hideMode()) {
  const s = VG.status;
  if (!s || !mode) return [];
  const m = [];
  if (mode === 'xbox' && s.vigem !== 'installed') m.push('vigem');
  // 鍵鼠 asks only when HidHide is really missing: 'unsupported' (dry run, not Windows) has no hider to install.
  if (hasNative(state.pad) && (mode === 'kbm' ? s.hidhide === 'missing' : s.hidhide !== 'installed')) m.push('hidhide');
  return m;
}

// Ask Rust about the selected pad. `open`: a pick of Xbox / 鍵鼠 or the first look at a pad in
// one of them, where anything missing opens the popover; otherwise only the status is refreshed
// (the reconnect banner) and a popover the user closed stays closed.
export async function probe({ open = true, mode = null } = {}) {
  // One probe at a time; a request during one is remembered (the latest wins, with its own
  // mode / open) and asked again when it returns.
  if (VG.probing) { VG.next = { mode, open }; return; }
  VG.probing = true;
  VG.stale = false;
  const kind = state.pad;
  let s = null;
  try { s = await invoke('get_xbox_status', { kind }); } catch (e) { if (open) s = { vigem: 'missing', hidhide: 'missing', hidden: null }; }
  VG.probing = false;
  if (VG.next) { const n = VG.next; VG.next = null; return probe(n); }
  if (state.pad !== kind) { VG.stale = true; rerender(); return; }   // the answer is for a pad no longer shown
  if (s) VG.status = s;
  const need = missing(mode || hideMode());
  if (open) { VG.pop = need.length > 0; VG.err = ''; }
  else if (!need.length) VG.pop = false;
  rerender();
}

// Header extras: the gold ! (popover closed, something missing or the live
// `vigem_missing` warning) and the popover itself.
export function bangHTML() {
  const mode = hideMode();
  if (!mode || VG.pop) return '';
  const live = mode === 'xbox' && (state.warnings || []).includes('vigem_missing');
  if (!missing().length && !live) return '';
  return '<button type="button" class="vgbang" data-act="vgopen" aria-label="需要安裝" title="需要安裝">!</button>';
}

function row(id, what) {
  const ok = !!VG.status && VG.status[id] === 'installed';
  return '<li class="' + (ok ? 'ok' : 'no') + '"><span class="vgn">' + DRIVER[id].name + '</span><span class="vgw">' + what + '</span><span class="vgs">' + (ok ? '已安裝' : '未安裝') + '</span></li>';
}

export function popHTML() {
  const mode = hideMode();
  if (!mode || !VG.pop) return '';
  const nat = hasNative(state.pad), need = missing(), pad = NAME[state.pad];
  if (mode === 'kbm') return '<div class="vgpop kbm" role="dialog" aria-label="需要安裝">' +
    '<div><b>要讓遊戲看不到 ' + pad + '、只收到鍵鼠，需要：</b></div>' +
    '<ul class="vgl">' + row('hidhide', '把原本的 ' + pad + ' 對遊戲藏起來') + '</ul>' +
    '<div class="vgnote">官方安裝程式，約 1 分鐘，會要求系統管理員權限。沒裝也能用鍵鼠，只是遊戲會同時讀到手把。</div>' +
    (VG.err ? '<div class="vgerr">' + VG.err + '</div>' : '') +
    '<div class="vgbtns"><button type="button" class="mini" data-act="vglater"' + (VG.busy ? ' disabled' : '') + '>稍後</button>' +
    '<button type="button" class="mini go" data-act="vginstall"' + (VG.busy ? ' disabled' : '') + '>' + (VG.busy ? '安裝中…' : '安裝 HidHide') + '</button></div></div>';
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
  return hideMode() === 'xbox' && (missing().length > 0 || (state.warnings || []).includes('vigem_missing'));
}

// HidHide hides a pad only from the next time it connects (`hidden: false` = not yet).
export function reconnectBanner() {
  if (hideMode() === 'kbm' && VG.status && VG.status.hidhide !== 'installed') return '';   // nothing hides the pad here
  if (!hideMode() || !hasNative(state.pad) || missing().length || !VG.status || VG.status.hidden !== false) return '';
  if (hideMode() === 'kbm' && VG.rcDismissed.has(state.pad)) return '';   // 這次不再提醒
  return '再連一次 ' + NAME[state.pad] + '，遊戲才看不到原本的手把：按分頁上的 ' + ICON.power + ' 斷開，再按 ' + powerKey(state.pad) + ' 鍵連回來';
}

// The app powered this pad off itself because it needed that reconnect (`auto_off`): say so
// while it is away. No 這次不再提醒: it goes when the pad is back.
export function autoOffBanner() {
  if (!hideMode() || !hasNative(state.pad) || missing().length || !VG.status || !VG.status.auto_off) return '';
  if (state.connected.has(state.pad)) return '';
  return '已替你把 ' + NAME[state.pad] + ' 關機，好讓遊戲看不到它。按 ' + powerKey(state.pad) + ' 鍵打開就能用了';
}

// The 鍵鼠 banner may be put off for this launch, per pad (memory only: no config, no IPC).
export function reconnectDismissable() { return hideMode() === 'kbm'; }

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

// 稍後 in 鍵鼠: never open by itself again (the ! stays while HidHide is missing).
async function dismissPrompt() {
  if (state.prefs.hidhide_prompt_dismissed) return;
  state.prefs = { ...state.prefs, hidhide_prompt_dismissed: true };
  try { await invoke('set_ui_prefs', { prefs: { hidhide_prompt_dismissed: true } }); } catch (e) { console.warn('set_ui_prefs failed', e); }
}

export async function startVigem() {
  // A pick of Xbox re-probes; so does opening the board on a pad in Xbox mode
  // (first paint with no answer yet, maybeProbe).
  // A manual pick of 鍵鼠 on a pad Windows can read opens it too, dismissed or not.
  act.onModeChange((pad, mode) => {
    // 改回鍵鼠 (the user just declined the installs) re-probes quietly: only the !.
    const quiet = VG.quiet; VG.quiet = false;
    if (mode === 'xbox' || (mode === 'kbm' && hasNative(pad))) { VG.status = null; probe({ mode, open: !quiet }); } else VG.pop = false;
  });
  // Another tab: the drivers are the same, `hidden` / `auto_off` are per pad (refreshed on the next paint).
  onPadChange(() => {
    VG.pop = false; VG.err = '';
    if (VG.status) VG.status = { ...VG.status, hidden: null, auto_off: false };
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
    if (a === 'vglater') { VG.pop = false; if (hideMode() === 'kbm') dismissPrompt(); rerender(); return true; }
    if (a === 'vgkbm') { VG.pop = false; VG.quiet = true; act.setMode('kbm'); return true; }
    if (a === 'rcdismiss') { VG.rcDismissed.add(state.pad); rerender(); return true; }
    if (a === 'vginstall') { if (!VG.busy) install(); return true; }
    return false;
  });
}

// Called on every paint: the first look at a pad in Xbox / 鍵鼠 mode, or a refresh after an event.
// In 鍵鼠 the popover opens by itself only for a connected pad, once, and not after 稍後.
// One probe at a time; none while installing (install re-probes itself).
export function maybeProbe() {
  const mode = hideMode();
  if (!mode || VG.probing || VG.busy) return;
  // 鍵鼠 opens by itself once per session, when the selected pad is connected (not before any
  // pad is there), and never after 稍後.
  const ask = mode === 'kbm' && !state.prefs.hidhide_prompt_dismissed && !VG.autoOpened && state.connected.has(state.pad);
  if (ask) VG.autoOpened = true;
  if (VG.status === null) probe({ open: mode === 'xbox' || ask });
  else if (ask) probe({ open: true });
  else if (VG.stale) probe({ open: false });
}
