// web-smoke.mjs — headless click-through of the key-binding board (rust/web/).
//
// Implements the "GUI frontend verification (browser click-through)" recipe
// in CLAUDE.md. Run by scripts/verify.sh (layer "web"); usable on its own:
//
//   NODE_PATH=<dir>/node_modules node scripts/verify/web-smoke.mjs \
//     --web-dir rust/web --config rust/config.example.json \
//     --out-dir rust/target/verify --chrome <path/to/chrome> --version 3.0.0
//
// The page cannot load without window.__TAURI__ (ipc.js throws), so a mock
// is injected via addInitScript before any module script runs. The mock keeps
// the config in memory, applies every mutating command the board uses, emits
// `config-changed` after each one (iron rule 10 — the real backend does too)
// and records every call with its arguments.
//
// Exit code 0 = every check passed; 1 = at least one check failed.

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright-core');

// ─── Args ────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const k = argv[i];
    if (!k.startsWith('--')) throw new Error(`unexpected argument ${k}`);
    out[k.slice(2)] = argv[i + 1];
  }
  for (const req of ['web-dir', 'config', 'out-dir', 'chrome']) {
    if (!out[req]) throw new Error(`missing --${req}`);
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
const webDir = path.resolve(args['web-dir']);
const outDir = path.resolve(args['out-dir']);
const exampleConfig = JSON.parse(fs.readFileSync(args.config, 'utf8'));
const appVersion = args.version || '0.0.0-mock';
// Serve with the app's own Content-Security-Policy, so anything WebView2
// would block (inline script, a foreign font, …) fails here as a console error.
const tauriConf = JSON.parse(fs.readFileSync(path.join(webDir, '..', 'tauri.conf.json'), 'utf8'));
const csp = tauriConf.app?.security?.csp || '';
fs.mkdirSync(outDir, { recursive: true });

// ─── Result bookkeeping ──────────────────────────────────────────────────────

const checks = [];   // { name, ok, detail }
function check(name, ok, detail = '') {
  checks.push({ name, ok: !!ok, detail });
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ' — ' + detail : ''}`);
}

// ─── Static server ───────────────────────────────────────────────────────────

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png',
  '.ico': 'image/x-icon', '.woff2': 'font/woff2',
};

const httpMisses = [];
function startServer() {
  const server = http.createServer((req, res) => {
    const urlPath = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    const rel = urlPath === '/' ? 'index.html' : urlPath.replace(/^\/+/, '');
    const file = path.resolve(webDir, rel);
    if (!file.startsWith(webDir + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      httpMisses.push(urlPath);
      res.writeHead(404);
      res.end('not found');
      return;
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream', ...(csp ? { 'Content-Security-Policy': csp } : {}) });
    fs.createReadStream(file).pipe(res);
  });
  return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve(server)));
}

// ─── Tauri mock (runs inside the page, before module scripts) ────────────────

function tauriMock({ config, version }) {
  const listeners = {};
  let nextId = 1;
  const clone = v => JSON.parse(JSON.stringify(v));
  const defaults = clone(config.pads);   // the example's pads = the default tables
  const state = {
    config: clone(config),
    prefs: { drawer_open: false, last_tab: null, last_pad: null },
    status: { name: 'DualSense', transport: 'BT', kind: 'dualsense', warnings: [], connected: ['dualsense'], off_pads: [] },
    sticks: null,
    vigem: 'installed',          // get_vigem_status reply
    vigemAfterInstall: 'installed',
  };
  window.__mock = state;
  window.__mockCalls = [];
  window.__mockErrors = [];
  window.__mockFail = {};   // cmd -> error string: the next call of cmd rejects
  window.addEventListener('error', e => window.__mockErrors.push(`error: ${e.message}`));
  window.addEventListener('unhandledrejection', e =>
    window.__mockErrors.push(`unhandledrejection: ${e.reason && (e.reason.stack || e.reason)}`));

  function emit(name, payload) {
    for (const l of (listeners[name] || []).slice()) l.cb({ event: name, id: l.id, payload });
  }
  const changed = () => setTimeout(() => emit('config-changed', { source: 'ipc' }), 0);
  window.__mockEmit = emit;
  const FIELD = { base: 'buttons', quick_l: 'quick_l', quick_r: 'quick_r' };
  const LABELS = {};
  for (const [id, e] of Object.entries(config.pads.dualsense.buttons)) LABELS[id] = e.label;

  function put(pad, page, id, entry) {
    const t = state.config.pads[pad][FIELD[page]];
    if (page !== 'base' && entry.type === 'unbound') delete t[String(id)];
    else t[String(id)] = clone(entry);
  }
  function get(pad, page, id) {
    return state.config.pads[pad][FIELD[page]][String(id)] || { label: LABELS[id] || '', type: 'unbound' };
  }
  function renameRefs(oldName, newName) {
    for (const p of Object.values(state.config.pads)) for (const f of ['buttons', 'quick_l', 'quick_r']) {
      for (const [id, e] of Object.entries(p[f] || {})) {
        if (e.type === 'macro' && e.value === oldName) {
          if (newName) e.value = newName; else if (f === 'buttons') p[f][id] = { label: e.label, type: 'unbound' }; else delete p[f][id];
        }
      }
    }
  }

  async function invoke(cmd, a) {
    window.__mockCalls.push({ cmd, args: a === undefined ? null : clone(a) });
    if (window.__mockFail[cmd]) { const m = window.__mockFail[cmd]; delete window.__mockFail[cmd]; throw m; }
    const c = state.config;
    switch (cmd) {
      case 'get_controller_status': return clone(state.status);
      case 'get_config': return clone(c);
      case 'get_ui_prefs': return clone(state.prefs);
      case 'set_ui_prefs': state.prefs = clone(a.prefs); return null;
      case 'get_app_version': return version;
      case 'get_config_path': return 'D:\\Games\\PadToKBM\\padtokbm.json';
      case 'get_stick_positions': return state.sticks ? clone(state.sticks) : null;
      case 'set_binding': put(a.pad, a.page, a.id, a.entry); changed(); return null;
      case 'swap_bindings': {
        const ea = get(a.pad, a.pageA, a.idA), eb = get(a.pad, a.pageB, a.idB);
        put(a.pad, a.pageA, a.idA, { ...eb, label: ea.label });
        put(a.pad, a.pageB, a.idB, { ...ea, label: eb.label });
        changed(); return null;
      }
      case 'set_trigger_mode': c.pads[a.pad].trigger_mode[a.side] = a.mode; changed(); return null;
      case 'clear_pad': {
        const p = c.pads[a.pad];
        for (const e of Object.values(p.buttons)) { delete e.value; e.type = 'unbound'; }
        p.quick_l = {}; p.quick_r = {};
        changed(); return null;
      }
      case 'reset_pad': c.pads[a.pad] = clone(defaults[a.pad]); changed(); return null;
      case 'set_pad_tables': c.pads[a.pad] = clone(a.tables); changed(); return null;
      case 'set_macro': c.macros[a.name] = clone(a.def); changed(); return null;
      case 'rename_macro':
        if (c.macros[a.new]) throw `macro '${a.new}' already exists`;
        c.macros[a.new] = c.macros[a.old]; delete c.macros[a.old]; renameRefs(a.old, a.new); changed(); return null;
      case 'delete_macro': delete c.macros[a.name]; renameRefs(a.name, null); changed(); return null;
      case 'set_settings': Object.assign(c, clone(a.settings)); changed(); return null;
      case 'set_mapping_enabled': c.mapping_enabled[a.kind] = a.enabled; changed(); return null;
      case 'get_vigem_status': return state.vigem;
      case 'install_vigem': await new Promise(r => setTimeout(r, 150)); state.vigem = state.vigemAfterInstall; return state.vigem;
      case 'set_pad_off': {
        // Runtime only (no config-changed); the engine re-sends the status.
        const off = new Set(state.status.off_pads || []);
        if (a.off) off.add(a.kind); else off.delete(a.kind);
        state.status.off_pads = ['dualsense', 'ns_pro', 'ns2_pro'].filter(k => off.has(k));
        setTimeout(() => emit('controller-status', { connected: true, ...clone(state.status), connected_pads: state.status.connected }), 0);
        return clone(state.status.off_pads);
      }
      case 'set_rumble_on_connect': c.rumble_on_connect = a.enabled; changed(); return null;
      case 'save_stick_cal': c.stick_cal = c.stick_cal || {}; c.stick_cal[a.pad] = clone(a.cal); changed(); return null;
      case 'reset_stick_cal': if (c.stick_cal) delete c.stick_cal[a.pad]; changed(); return null;
      default: return null;
    }
  }

  // The drawn title bar (v3.3.0) drives the window through the Tauri window API.
  const winCall = cmd => async () => { window.__mockCalls.push({ cmd, args: {} }); };
  const appWindow = { minimize: winCall('window.minimize'), close: winCall('window.close'), startDragging: winCall('window.startDragging') };
  window.__TAURI__ = {
    core: { invoke },
    window: { getCurrentWindow: () => appWindow },
    event: {
      listen: async (name, cb) => {
        const id = nextId++;
        (listeners[name] = listeners[name] || []).push({ id, cb });
        return () => { listeners[name] = (listeners[name] || []).filter(l => l.id !== id); };
      },
    },
  };
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

const calls = page => page.evaluate(() => window.__mockCalls);
const lastCall = async (page, cmd) => (await calls(page)).filter(c => c.cmd === cmd).pop();
const countCalls = async (page, cmd) => (await calls(page)).filter(c => c.cmd === cmd).length;
const settle = page => page.waitForTimeout(120);
const slot = (page, key) => page.locator(`#board .slot[data-slot="${key}"]`);
const tile = (page, key) => page.locator(`#board [data-tile="${key}"]`);
const slotText = async (page, key) => ((await slot(page, key).locator('.val').textContent()) || '').trim();

// ─── Main ────────────────────────────────────────────────────────────────────

const server = await startServer();
const base = `http://127.0.0.1:${server.address().port}/`;
const browser = await chromium.launch({ executablePath: args.chrome, headless: true });
const consoleErrors = [];
const pageErrors = [];
const dialogs = [];

try {
  // The window is the drawn MapleStory window, 994x669 at 1x (board 980x640 at 7,22; chrome.js).
  const context = await browser.newContext({ viewport: { width: 994, height: 669 } });
  await context.addInitScript(tauriMock, { config: exampleConfig, version: appVersion });
  const page = await context.newPage();
  page.on('pageerror', e => pageErrors.push(String(e && (e.stack || e))));
  page.on('console', m => {
    if (m.type() !== 'error') return;
    const url = (m.location() && m.location().url) || '';
    if (/Failed to load resource/.test(m.text()) && /favicon\.ico/.test(url)) return;
    consoleErrors.push(`${m.text()} @ ${url}`);
  });
  page.on('dialog', async d => { dialogs.push(`${d.type()}: ${d.message()}`); await d.dismiss(); });

  await page.goto(base, { waitUntil: 'load' });
  await page.locator('#board .hdr').waitFor({ timeout: 10000 });
  await page.waitForFunction(() => document.querySelectorAll('#board .slot').length > 20, null, { timeout: 10000 });
  await settle(page);
  await page.screenshot({ path: path.join(outDir, 'web-main.png') });

  // ── Task 5: header, pads, binding actions, undo ───────────────────────────
  check('header title 按鍵設定', ((await page.locator('#board .hdr .ttl').textContent()) || '').trim() === '按鍵設定');
  const padNames = await page.locator('#board .hdr [data-pad]').allTextContents();
  check('three pad buttons', JSON.stringify(padNames.map(s => s.trim())) === JSON.stringify(['DualSense', 'Switch Pro', 'Switch 2 Pro']), padNames.join('|'));
  const lights = await page.locator('#board .hdr [data-pad] .live').evaluateAll(els => els.map(e => e.classList.contains('idle') ? 'idle' : 'live'));
  check('DualSense light live, others idle', JSON.stringify(lights) === '["live","idle","idle"]', lights.join(','));
  check('one ⏻ (connected pad only)', (await page.locator('#board .hdr .pwr').count()) === 1);

  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  const ns2Slots = await page.locator('#board .slot').evaluateAll(els => els.map(e => e.dataset.slot));
  check('Switch 2 Pro shows 30/31/32 and no touchpad slots', ['30', '31', '32'].every(k => ns2Slots.includes(k)) && !ns2Slots.includes('25'));
  check('pad choice saved as last_pad', (await lastCall(page, 'set_ui_prefs'))?.args?.prefs?.last_pad === 'ns2_pro');
  check('Switch 2 Pro maps to keys in the example config (v3.2.0 default)', !(await page.locator('#board').getAttribute('class')).split(' ').includes('off'));
  await page.evaluate(() => { delete window.__mock.config.mapping_enabled.ns2_pro; });
  await page.evaluate(() => window.__mockEmit('config-changed', { source: 'external' }));
  await settle(page);
  check('missing mapping_enabled.ns2_pro → keys (not off)', !(await page.locator('#board').getAttribute('class')).split(' ').includes('off'));

  await tile(page, 'Q').dragTo(slot(page, '0'));
  await settle(page);
  const bindQ = await lastCall(page, 'set_binding');
  check('drag Q onto slot 0 → set_binding (ns2_pro, base, 0, key q)',
    bindQ?.args?.pad === 'ns2_pro' && bindQ.args.page === 'base' && bindQ.args.id === 0 && bindQ.args.entry.type === 'key' && bindQ.args.entry.value === 'q',
    JSON.stringify(bindQ?.args));
  check('slot 0 shows Q', (await slotText(page, '0')) === 'Q', await slotText(page, '0'));

  const swaps = await countCalls(page, 'swap_bindings');
  await slot(page, '0').dragTo(slot(page, '1'));
  await settle(page);
  const sw = await lastCall(page, 'swap_bindings');
  check('drag slot 0 onto slot 1 → one swap_bindings', (await countCalls(page, 'swap_bindings')) === swaps + 1 &&
    sw.args.pageA === 'base' && sw.args.idA === 0 && sw.args.pageB === 'base' && sw.args.idB === 1, JSON.stringify(sw?.args));
  check('after swap slot 1 shows Q', (await slotText(page, '1')) === 'Q');

  await slot(page, '1').dragTo(page.locator('#board .stage'), { targetPosition: { x: 300, y: 12 } });
  await settle(page);
  const del = await lastCall(page, 'set_binding');
  check('drag slot out → unbound', del?.args?.id === 1 && del.args.entry.type === 'unbound', JSON.stringify(del?.args));

  await slot(page, '11').dblclick();
  await settle(page);
  const dbl = await lastCall(page, 'set_binding');
  check('double-click clears a slot', dbl?.args?.id === 11 && dbl.args.entry.type === 'unbound', JSON.stringify(dbl?.args));

  await page.click('#board .qcap [data-qs="L"][data-mode="single"]');
  await settle(page);
  const tm = await lastCall(page, 'set_trigger_mode');
  check('單鍵 → set_trigger_mode (l, key)', tm?.args?.pad === 'ns2_pro' && tm.args.side === 'l' && tm.args.mode === 'key', JSON.stringify(tm?.args));
  check('ZL page folds into slot 23', (await slot(page, '23').count()) === 1 && (await slot(page, 'L11').count()) === 0);

  await page.click('#board [data-act="clear"]');
  await settle(page);
  check('全部清除 → clear_pad', (await lastCall(page, 'clear_pad'))?.args?.pad === 'ns2_pro');
  check('復原 enabled after a change', await page.locator('#board [data-act="undo"]').isEnabled());
  await page.click('#board [data-act="undo"]');
  await settle(page);
  const undo = await lastCall(page, 'set_pad_tables');
  check('復原 → set_pad_tables restores the table before 全部清除',
    undo?.args?.pad === 'ns2_pro' && undo.args.tables.buttons['15'].type === 'key', JSON.stringify(undo?.args?.tables?.buttons?.['15']));
  check('重做 enabled after undo', await page.locator('#board [data-act="redo"]').isEnabled());
  await page.click('#board [data-act="redo"]');
  await settle(page);
  const redo = await lastCall(page, 'set_pad_tables');
  check('重做 → set_pad_tables with the cleared table', redo?.args?.tables?.buttons?.['15']?.type === 'unbound');
  check('重做 disabled at the end', !(await page.locator('#board [data-act="redo"]').isEnabled()));

  await page.click('#board [data-act="reset"]');
  await settle(page);
  check('恢復預設 → reset_pad', (await lastCall(page, 'reset_pad'))?.args?.pad === 'ns2_pro');

  // Review focus 2: undo switches to the pad it changes.
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);
  await tile(page, 'W').dragTo(slot(page, '2'));
  await settle(page);
  await page.click('#board [data-pad="ns_pro"]');
  await settle(page);
  await page.keyboard.press('Control+z');
  await settle(page);
  const ux = await lastCall(page, 'set_pad_tables');
  const shownPad = await page.locator('#board .hdr [data-pad][aria-pressed="true"]').getAttribute('data-pad');
  check('undo switches to the pad it changes (Ctrl+Z)', ux?.args?.pad === 'dualsense' && shownPad === 'dualsense', `${ux?.args?.pad} ${shownPad}`);

  // Review focus 1: an external config-changed keeps the selected pad.
  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  await page.evaluate(() => {
    window.__mock.config.pads.ns2_pro.buttons['9'] = { label: 'L1', type: 'key', value: 'F7' };
    window.__mockEmit('config-changed', { source: 'watcher' });
  });
  await settle(page);
  const stillNs2 = await page.locator('#board .hdr [data-pad][aria-pressed="true"]').getAttribute('data-pad');
  check('external config-changed keeps the selected pad', stillNs2 === 'ns2_pro' && (await slotText(page, '9')) === 'F7', `${stillNs2} ${await slotText(page, '9')}`);

  // Failed undo: the entry is dropped and nothing else changes.
  await tile(page, 'E').dragTo(slot(page, '2'));
  await settle(page);
  await page.evaluate(() => { window.__mockFail.set_pad_tables = 'unknown macro "gone"'; });
  await page.click('#board [data-act="undo"]');
  await settle(page);
  const note = ((await page.locator('#board .ftr .note').textContent()) || '').trim();
  check('a refused undo shows the reason', /無法復原/.test(note) && /gone/.test(note), note);
  check('a refused undo leaves the slot as it was', (await slotText(page, '2')) === 'E');

  // Header switches and ⏻.
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);
  await page.click('#board .hdr .pwr');
  await settle(page);
  check('⏻ → disconnect_gamepad (dualsense)', (await lastCall(page, 'disconnect_gamepad'))?.args?.pad === 'dualsense');
  await page.click('#board [data-act="rumble"]');
  await settle(page);
  check('連線震動 → set_rumble_on_connect false', (await lastCall(page, 'set_rumble_on_connect'))?.args?.enabled === false);
  // Output capsule 關｜鍵鼠｜手把 (v3.2.0).
  const modeOf = async () => page.locator('#board .omode [aria-pressed="true"]').getAttribute('data-mode');
  const banner = async () => ((await page.locator('#board .obanner').textContent().catch(() => '')) || '').trim();
  const dimmed = () => page.evaluate(() => document.getElementById('board').classList.contains('off'));
  const locked = () => page.evaluate(() => document.getElementById('board').classList.contains('lock'));
  const bannerBox = () => page.evaluate(() => {
    const b = document.querySelector('#board .obanner'); if (!b) return null;
    const r = b.getBoundingClientRect(); return { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
  });
  const bannerCentred = async () => { const c = await bannerBox(); return !!c && Math.abs(c.x - 497) <= 1 && Math.abs(c.y - 250) <= 1; };   // stage centre (490, 228) + board at (7, 22)
  // Click / drag by coordinates, so whatever covers the target gets the event (as a real mouse would).
  const centre = async loc => { const r = await loc.boundingBox(); return [r.x + r.width / 2, r.y + r.height / 2]; };
  const mouseClick = async loc => { const [x, y] = await centre(loc); await page.mouse.click(x, y); await settle(page); };
  const MUT = ['set_binding', 'swap_bindings', 'set_pad_tables', 'clear_pad', 'reset_pad', 'set_settings', 'set_trigger_mode', 'set_macro', 'set_capture_active'];
  async function expectLocked(what) {
    check(`${what} locks the board`, await locked());
    const before = (await calls(page)).filter(c => MUT.includes(c.cmd)).length;
    await mouseClick(tile(page, 'Q'));
    check(`${what}: tile click does not pick`, (await page.locator('#board .tile.picked').count()) === 0);
    await mouseClick(slot(page, '5'));
    check(`${what}: slot click does not wait for a key`, (await page.locator('#board .slot.wait').count()) === 0 && (await page.locator('#board .capbar').count()) === 0);
    await tile(page, 'W').dragTo(slot(page, '6'), { force: true, timeout: 3000 }).catch(() => {});
    await settle(page);
    if (await page.locator('#board [data-act="tp"]').count()) await mouseClick(page.locator('#board [data-act="tp"]'));
    if (await page.locator('#board .qcap [data-mode="single"]').count()) await mouseClick(page.locator('#board .qcap [data-mode="single"]').first());
    for (const a of ['reset', 'clear', 'undo', 'redo']) check(`${what}: ${a} disabled`, await page.locator(`#board [data-act="${a}"]`).isDisabled());
    await page.keyboard.press('Control+z');
    await settle(page);
    const after = (await calls(page)).filter(c => MUT.includes(c.cmd)).length;
    check(`${what}: no edit reached the backend`, after === before, `${before} → ${after}`);
    check(`${what}: header still works (capsule clickable)`, await page.locator('#board .omode [data-mode="kbm"]').isEnabled());
  }
  check('capsule shows 鍵鼠 for a fresh config', (await modeOf()) === 'kbm' && (await page.locator('#board .omode button').count()) === 3);
  check('no 鍵鼠對應 switch any more', (await page.locator('#board [data-act="map"]').count()) === 0);
  check('DualSense third segment reads 手把', ((await page.locator('#board .omode [data-mode="pad"]').textContent()) || '').trim() === '手把');
  await page.click('#board .omode [data-mode="off"]');
  await settle(page);
  const po = await lastCall(page, 'set_pad_off');
  check('關 sends set_pad_off(pad,true)', po?.args?.kind === 'dualsense' && po.args.off === true, JSON.stringify(po?.args));
  check('關 shows the gold banner', (await banner()) === '這支手把已關閉' && (await page.locator('#board .obanner.gold').count()) === 1 && (await modeOf()) === 'off', await banner());
  check('關 dims the board', await dimmed());
  await page.screenshot({ path: path.join(outDir, 'web-mode-off.png') });
  check('關 does not write the config', (await countCalls(page, 'set_mapping_enabled')) === 0);
  // Not 鍵鼠 → the board is locked (v3.2.1): nothing below the header edits.
  check('關 banner sits at the stage centre (490, 228 on the board)', await bannerCentred(), JSON.stringify(await bannerBox()));
  await expectLocked('關');
  await page.click('#board .omode [data-mode="pad"]');
  await settle(page);
  const offBack = await lastCall(page, 'set_pad_off'), me = await lastCall(page, 'set_mapping_enabled');
  check('手把 clears 關 (set_pad_off false)', offBack?.args?.kind === 'dualsense' && offBack.args.off === false, JSON.stringify(offBack?.args));
  check('手把 sends set_mapping_enabled(pad,false)', me?.args?.kind === 'dualsense' && me.args.enabled === false, JSON.stringify(me?.args));
  check('手把 shows the white banner', (await banner()) === '程式不送東西，遊戲直接用手把；下面的鍵鼠綁定這時不作用' && (await page.locator('#board .obanner.white').count()) === 1, await banner());
  check('手把 dims the board', (await dimmed()) && (await modeOf()) === 'pad');
  check('手把 banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
  await expectLocked('手把');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  const mk = await lastCall(page, 'set_mapping_enabled');
  check('鍵鼠 sends set_mapping_enabled(pad,true)', mk?.args?.kind === 'dualsense' && mk.args.enabled === true, JSON.stringify(mk?.args));
  check('鍵鼠: no banner, not dimmed', (await page.locator('#board .obanner').count()) === 0 && !(await dimmed()));
  check('鍵鼠 unlocks the board', !(await locked()) && !(await page.locator('#board [data-act="clear"]').isDisabled()));
  {
    const nb = await countCalls(page, 'set_binding');
    await tile(page, 'Q').dragTo(slot(page, '5'));
    await settle(page);
    check('鍵鼠: drag tile → set_binding sent again', (await countCalls(page, 'set_binding')) === nb + 1);
  }
  // A slot waiting for a key is cancelled when the board locks.
  await slot(page, '0').click();
  await settle(page);
  await page.click('#board .omode [data-mode="off"]');
  await settle(page);
  check('關 cancels a waiting slot (capture off, no bar)', (await page.locator('#board .slot.wait').count()) === 0 &&
    (await page.locator('#board .capbar').count()) === 0 && (await lastCall(page, 'set_capture_active'))?.args?.active === false);
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  check('NS2 third segment reads Xbox', ((await page.locator('#board .omode [data-mode="pad"]').textContent()) || '').trim() === 'Xbox');
  await page.evaluate(() => { window.__mock.config.mapping_enabled.ns2_pro = false; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  await page.screenshot({ path: path.join(outDir, 'web-mode-xbox.png') });
  check('NS2 in Xbox mode shows the Xbox banner', (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
  check('Xbox banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
  await expectLocked('Xbox');
  await page.evaluate(() => { window.__mock.config.mapping_enabled.ns2_pro = true; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: ['ns2_pro'] }); });
  await settle(page);
  check('off_pads from controller-status selects 關', (await modeOf()) === 'off');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  // ViGEmBus prompt (Switch 2 Pro, Xbox).
  const pop = () => page.locator('#board .vgpop');
  await page.evaluate(() => { window.__mock.vigem = 'missing'; });
  await page.click('#board .omode [data-mode="pad"]');
  await settle(page);
  check('Xbox with ViGEmBus missing opens the popover', (await pop().count()) === 1 && /這台還沒有 ViGEmBus/.test(await pop().textContent()) && (await countCalls(page, 'get_vigem_status')) >= 1);
  await page.screenshot({ path: path.join(outDir, 'web-vigem.png') });
  check('popover has 稍後 / 改回鍵鼠 / 安裝 ViGEmBus',
    JSON.stringify(await page.locator('#board .vgpop button').allTextContents()) === JSON.stringify(['稍後', '改回鍵鼠', '安裝 ViGEmBus']));
  await page.click('#board [data-act="vglater"]');
  await settle(page);
  await page.screenshot({ path: path.join(outDir, 'web-vigem-bang.png') });
  check('稍後 closes it and leaves the !', (await pop().count()) === 0 && (await page.locator('#board .hdr .vgbang').count()) === 1);
  await page.click('#board .hdr .vgbang');
  await settle(page);
  check('the ! reopens it', (await pop().count()) === 1);
  await page.evaluate(() => { window.__mock.vigemAfterInstall = 'missing'; });
  await page.click('#board [data-act="vginstall"]');
  await page.waitForTimeout(40);
  check('安裝 shows 安裝中…', ((await page.locator('#board [data-act="vginstall"]').textContent()) || '').trim() === '安裝中…');
  await page.waitForTimeout(300);
  check('a failed install keeps the popover with an error line', (await pop().count()) === 1 && (await page.locator('#board .vgpop .vgerr').count()) === 1);
  await page.evaluate(() => { window.__mock.vigemAfterInstall = 'installed'; });
  await page.click('#board [data-act="vginstall"]');
  await page.waitForTimeout(350);
  check('安裝 calls install_vigem and closes on installed', (await countCalls(page, 'install_vigem')) === 2 && (await pop().count()) === 0 && (await page.locator('#board .hdr .vgbang').count()) === 0);
  check('installed → Xbox banner shows', (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
  // 改回鍵鼠 from the popover.
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  await page.evaluate(() => { window.__mock.vigem = 'missing'; });
  await page.click('#board .omode [data-mode="pad"]');
  await settle(page);
  await page.click('#board [data-act="vgkbm"]');
  await settle(page);
  const back = await lastCall(page, 'set_mapping_enabled');
  check('改回鍵鼠 sends set_mapping_enabled(ns2_pro,true)', back?.args?.kind === 'ns2_pro' && back.args.enabled === true && (await pop().count()) === 0, JSON.stringify(back?.args));
  await page.evaluate(() => { window.__mock.vigem = 'installed'; });
  await page.click('#board .omode [data-mode="pad"]');
  await settle(page);
  check('installed → no popover', (await pop().count()) === 0 && (await page.locator('#board .hdr .vgbang').count()) === 0);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'ns2_pro', connected_pads: ['dualsense', 'ns2_pro'], warnings: ['vigem_missing'], off_pads: [] }); });
  await settle(page);
  check('live vigem_missing warning shows the !', (await page.locator('#board .hdr .vgbang').count()) === 1);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }); });
  await page.click('#board [data-pad="ns2_pro"]');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);

  check('header fits 980 px (no overflow)', await page.evaluate(() => { const h = document.querySelector('#board .hdr'); return h.scrollWidth <= h.clientWidth; }));
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);

  // ── The drawn window (v3.3.0): title bar, minimise / close, touchpad switch in the header ──
  check('title bar reads PAD TO KBM', ((await page.locator('#wtbar .wt').textContent()) || '') === 'PAD TO KBM');
  await page.click('#wtbar [data-win="min"]');
  check('title-bar minimise calls window.minimize', (await countCalls(page, 'window.minimize')) === 1);
  await page.mouse.move(300, 10); await page.mouse.down(); await page.mouse.up();
  check('title-bar drag calls window.startDragging', (await countCalls(page, 'window.startDragging')) >= 1);
  await page.focus('#wtbar [data-win="min"]');
  await page.keyboard.press('Shift');
  check('title-bar keys draw no focus ring (Q13)', await page.evaluate(() => getComputedStyle(document.querySelector('#wtbar [data-win="min"]')).outlineStyle === 'none'));
  await page.click('#wtbar [data-win="close"]');
  check('title-bar close calls window.close', (await countCalls(page, 'window.close')) === 1);
  check('touchpad switch sits in the header left of 連線震動 (DualSense)',
    await page.evaluate(() => { const t = document.querySelector('#board .hdr [data-act="tp"]'), r = document.querySelector('#board .hdr [data-act="rumble"]');
      return !!t && !!r && t.nextElementSibling === r && !document.querySelector('#board .stage ~ [data-act="tp"], #board .tstack'); }));
  check('touchpad switch is a 關｜開 pair', ((await page.locator('#board .hdr [data-act="tp"] .track').textContent()) || '') === '關開');
  await page.click('#board [data-pad="ns_pro"]');
  await settle(page);
  check('no touchpad switch on the Switch Pro tab', (await page.locator('#board [data-act="tp"]').count()) === 0);
  // 手把 / Xbox: the header's right-hand controls keep their place across the tabs.
  const hdrSpots = {};
  for (const p of ['ns_pro', 'ns2_pro', 'dualsense']) {
    await page.click(`#board [data-pad="${p}"]`);
    await settle(page);
    hdrSpots[p] = await page.evaluate(() => ['[data-act="rumble"]', '.omode', '.omode [data-mode="pad"]'].map(s => {
      const r = document.querySelector('#board .hdr ' + s).getBoundingClientRect(); return [r.left, r.width].join('/'); }).join(' '));
  }
  check('header controls do not move between pad tabs (手把 / Xbox)', new Set(Object.values(hdrSpots)).size === 1, JSON.stringify(hdrSpots));

  // ── Layout: the window never scrolls; the board always fits ──────────────
  for (const [w, h] of [[880, 560], [994, 669], [1280, 800], [1600, 1000], [1988, 1338]]) {
    await page.setViewportSize({ width: w, height: h });
    await page.waitForTimeout(150);
    const m = await page.evaluate(() => {
      const r = document.getElementById('board').getBoundingClientRect();
      const d = document.documentElement;
      return { sw: d.scrollWidth, cw: d.clientWidth, sh: d.scrollHeight, ch: d.clientHeight, l: r.left, t: r.top, r: r.right, b: r.bottom, iw: innerWidth, ih: innerHeight };
    });
    check(`no scroll and board fits @${w}x${h}`,
      m.sw === m.cw && m.sh === m.ch && m.l >= -0.5 && m.t >= -0.5 && m.r <= m.iw + 0.5 && m.b <= m.ih + 0.5, JSON.stringify(m));
    // The drawn window (v3.3.0): a whole scale when the viewport allows one (n = 994x669 multiples),
    // centred on whole pixels with a steel margin when bigger, a fractional fit only below 1x.
    const win = await page.evaluate(() => {
      const r = document.getElementById('win').getBoundingClientRect();
      return { x: r.left, y: r.top, w: r.width, h: r.height, iw: innerWidth, ih: innerHeight, margin: document.getElementById('app').classList.contains('margin') };
    });
    const n = Math.floor(Math.min(w / 994, h / 669) + 0.01);
    const whole = n >= 1 ? Math.abs(win.w - 994 * n) < 0.01 && Math.abs(win.h - 669 * n) < 0.01 && Number.isInteger(win.x) && Number.isInteger(win.y)
      : win.w <= w + 0.5 && win.h <= h + 0.5;
    check(`window drawn at a whole scale @${w}x${h}`, whole && win.margin === (win.x > 0 || win.y > 0), JSON.stringify(win));
  }
  await page.setViewportSize({ width: 994, height: 669 });
  const ob = await page.evaluate(() => ({
    html: getComputedStyle(document.documentElement).overscrollBehaviorX,
    body: getComputedStyle(document.body).overscrollBehaviorX,
  }));
  check('overscroll-behavior none on html/body', ob.html === 'none' && ob.body === 'none', JSON.stringify(ob));

  // ── Task 6: live feedback and key capture ─────────────────────────────────
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);
  await page.evaluate(() => window.__mockEmit('button-down', { id: 12 }));
  await settle(page);
  // Reskin Q10/Q12: a held key that is also a pale bound key shows rose, not pale.
  {
    // Final review I2: a picked key that is also bound (pale) shows the blue picked look.
    const u = await page.evaluate(() => document.querySelector('#board .kbd .tile.used')?.dataset.tile);
    await page.click(`#board .kbd [data-tile="${u}"]`);
    await settle(page);
    check('picked bound key is blue over pale', await page.evaluate(k => {
      const t = document.querySelector(`#board .kbd [data-tile="${k}"]`);
      return t.classList.contains('picked') && t.classList.contains('used') && getComputedStyle(t).backgroundImage.includes('rgb(26, 144, 230)');
    }, u), u);
    await page.click(`#board .kbd [data-tile="${u}"]`);
    await settle(page);
  }
  check('held bound key is rose over pale', await page.evaluate(() => {
    const t = document.querySelector('#board .pal .tile.hit.used'); if (!t) return false;
    return getComputedStyle(t).backgroundImage.includes('rgb(233, 103, 133)');
  }));
  check('button-down lights its slot and its line', (await slot(page, '12').getAttribute('class')).includes('hit') &&
    (await page.locator('#board .lines g.hit').count()) === 1);
  check('button-down lights the bound tile', (await tile(page, '↓').getAttribute('class')).includes('hit'));
  await page.evaluate(() => window.__mockEmit('button-up', { id: 12 }));
  await settle(page);
  check('button-up clears the highlight', !(await slot(page, '12').getAttribute('class')).includes('hit') &&
    (await page.locator('#board .lines g.hit').count()) === 0);
  await page.evaluate(() => { window.__mockEmit('button-down', { id: 23 }); window.__mockEmit('button-down', { id: 13 }); });
  await settle(page);
  check('page-mode L2 held: D-pad press lights the quick_l slot', (await slot(page, 'L13').getAttribute('class')).includes('hit') &&
    !(await slot(page, '13').getAttribute('class')).includes('hit'));
  await page.evaluate(() => { window.__mockEmit('button-up', { id: 13 }); window.__mockEmit('button-up', { id: 23 }); });
  await settle(page);
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'q', action: 'down' }));
  await settle(page);
  check('emitted key lights its tile', (await tile(page, 'Q').getAttribute('class')).includes('hit'));
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'q', action: 'up' }));
  await settle(page);
  check('emitted key-up clears its tile', !(await tile(page, 'Q').getAttribute('class')).includes('hit'));

  await page.keyboard.down('ArrowUp');
  await page.waitForTimeout(80);
  check('reverse lookup: a real key lights its tile and the slots bound to it',
    (await tile(page, '↑').getAttribute('class')).includes('hit') && (await slot(page, '11').getAttribute('class')).includes('hit') &&
    (await slot(page, '15').getAttribute('class')).includes('hit'));
  await page.keyboard.up('ArrowUp');
  await settle(page);
  check('reverse lookup clears on key-up', !(await slot(page, '11').getAttribute('class')).includes('hit'));
  await page.evaluate(() => {
    document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyW', key: 'w', bubbles: true }));
    setTimeout(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'w', action: 'down' }), 5);
    setTimeout(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'w', action: 'up' }), 15);
  });
  await page.waitForTimeout(100);   // past the 30 ms window; the real key is still "down"
  const synthLit = (await tile(page, 'W').getAttribute('class')).includes('hit');
  await page.evaluate(() => document.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyW', key: 'w', bubbles: true })));
  await settle(page);
  check('reverse lookup ignores the engine\'s own synthesized key', synthLit === false, 'W lit after the engine typed it: ' + synthLit);

  const capBefore = await countCalls(page, 'set_capture_active');
  await slot(page, '3').click();
  await settle(page);
  check('clicking a slot starts key capture', (await lastCall(page, 'set_capture_active'))?.args?.active === true &&
    (await countCalls(page, 'set_capture_active')) === capBefore + 1);
  check('capture bar text is exact', ((await page.locator('#board .capbar').textContent()) || '').trim() === '按鍵鼠上的鍵，或點下面的方塊（Esc 取消）');
  {
    // Screen readers only (reskin Q8): the blue slot shows the wait; the text stays for assistive tech.
    const c = await page.evaluate(() => { const e = document.querySelector('#board .capbar'), r = e.getBoundingClientRect(); return { w: r.width, h: r.height, role: e.getAttribute('role') }; });
    check('capture text is for screen readers only (role=status, 1px box)', c.role === 'status' && c.w <= 1 && c.h <= 1, JSON.stringify(c));
  }
  check('waiting slot has no keycap badge (reskin Q8)', (await slot(page, '3').locator('.kc').count()) === 0 && (await slot(page, '3').getAttribute('class')).includes('sel'));
  await page.keyboard.press('f');
  await settle(page);
  const capBind = await lastCall(page, 'set_binding');
  check('pressed key binds the waiting slot', capBind?.args?.id === 3 && capBind.args.entry.value === 'f', JSON.stringify(capBind?.args));
  check('capture ends after binding', (await lastCall(page, 'set_capture_active'))?.args?.active === false && (await page.locator('#board .capbar').count()) === 0);
  await slot(page, '2').click();
  await settle(page);
  await slot(page, '2').click({ button: 'right' });
  await settle(page);
  const rc = await lastCall(page, 'set_binding');
  check('right click while waiting binds 右鍵', rc?.args?.id === 2 && rc.args.entry.type === 'mouse' && rc.args.entry.value === 'right', JSON.stringify(rc?.args));
  await slot(page, '1').click();
  await settle(page);
  await page.mouse.move(470, 500);
  await page.mouse.wheel(0, -120);
  await settle(page);
  const wh = await lastCall(page, 'set_binding');
  check('wheel up while waiting binds 滾輪上', wh?.args?.id === 1 && wh.args.entry.value === 'wheel-up', JSON.stringify(wh?.args));
  const nBind = await countCalls(page, 'set_binding');
  await slot(page, '0').click();
  await settle(page);
  await page.keyboard.press('Escape');
  await settle(page);
  check('Esc cancels capture without binding', (await countCalls(page, 'set_binding')) === nBind &&
    (await lastCall(page, 'set_capture_active'))?.args?.active === false);
  await slot(page, '0').click();
  await settle(page);
  await page.click('#board [data-pad="ns_pro"]');
  await settle(page);
  const endPad = (await lastCall(page, 'set_capture_active'))?.args?.active === false && (await page.locator('#board .capbar').count()) === 0;
  await slot(page, '0').click();
  await settle(page);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await settle(page);
  const endBlur = (await lastCall(page, 'set_capture_active'))?.args?.active === false && (await page.locator('#board .capbar').count()) === 0;
  check('capture ends on pad switch and on blur', endPad && endBlur && (await countCalls(page, 'set_binding')) === nBind, `${endPad} ${endBlur}`);
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);

  // ── Task 7: macro box and editor ──────────────────────────────────────────
  await page.evaluate(() => {
    const m = window.__mock.config.macros;
    for (const n of ['a1', 'a2', 'a3', 'a4', 'a5', 'a6', 'a7', 'a8', 'a9']) m[n] = { loop: false, steps: [{ key: 'z', action: 'down', delay_ms: [40, 60] }, { key: 'z', action: 'up', delay_ms: [15, 25] }] };
    window.__mockEmit('config-changed', { source: 'watcher' });
  });
  await settle(page);
  check('macro box page 1: 8 macro tiles, no ＋', (await page.locator('#board .macros .tile.mac').count()) === 8 && (await page.locator('#board .macros .mnew').count()) === 0);
  await page.locator('#board .macros').hover();
  await page.mouse.wheel(0, 120);
  await settle(page);
  check('wheel turns the macro page: 1 tile + ＋', (await page.locator('#board .macros .tile.mac').count()) === 1 && (await page.locator('#board .macros .mnew').count()) === 1);
  await page.click('#board .macros .mnew');
  await settle(page);
  const nm = await lastCall(page, 'set_macro');
  check('＋ creates 巨集 1 with no steps', nm?.args?.name === '巨集 1' && nm.args.def.steps.length === 0 && nm.args.def.loop === false, JSON.stringify(nm?.args));
  check('the editor opens on the new macro', (await page.locator('#board .med').count()) === 1 && (await page.locator('#board .med .mname').inputValue()) === '巨集 1');
  check('mouse and macro tiles are locked while editing', (await page.locator('#board .m3s.locked').count()) === 1 && (await page.locator('#board .macros.locked').count()) === 1);
  await tile(page, 'A').click();
  await settle(page);
  const stepsA = await page.locator('#board .med .st[data-step]:not([data-step="end"])').count();
  const pills = await page.locator('#board .med [data-act="sdelay"]').allTextContents();
  check('a keyboard click appends a down+up pair with default delays', stepsA === 2 && pills[0].includes('40–60') && pills[1].includes('15–25'), pills.join('|'));
  await page.click('#board .med [data-act="sdelay"][data-i="0"]');
  await page.locator('#board .dpop [data-f="dhi"]').fill('40');
  await page.locator('#board .dpop [data-f="dhi"]').dispatchEvent('input');
  await page.click('#board .med .mhd .mname');
  const before = await countCalls(page, 'set_macro');
  await page.click('#board .med [data-act="mdone"]');
  await settle(page);
  check('完成 is refused while a delay has min >= max (its popover opens)', (await countCalls(page, 'set_macro')) === before &&
    (await page.locator('#board .dpop').count()) === 1 && (await page.locator('#board .med').count()) === 1);
  await page.locator('#board .dpop [data-f="dhi"]').fill('70');
  await page.locator('#board .dpop [data-f="dhi"]').dispatchEvent('input');
  await page.keyboard.press('Escape');
  await page.locator('#board .med .mname').fill('連發');
  await page.locator('#board .med .mname').dispatchEvent('input');
  await page.click('#board .med [data-act="mloop"]');
  await page.click('#board .med [data-act="mdone"]');
  await settle(page);
  const seq = (await calls(page)).map(c => c.cmd).filter(c => /macro/.test(c)).slice(-2);
  const saved = await lastCall(page, 'set_macro');
  check('rename + 完成 → rename_macro then set_macro', seq.join(',') === 'rename_macro,set_macro' &&
    saved.args.name === '連發' && saved.args.def.loop === true && saved.args.def.steps[0].key === 'a' &&
    JSON.stringify(saved.args.def.steps[0].delay_ms) === '[40,70]', seq.join(',') + ' ' + JSON.stringify(saved?.args));
  check('the editor closes after 完成', (await page.locator('#board .med').count()) === 0);
  await page.locator('#board [data-medit="連發"]').click();
  await settle(page);
  await page.click('#board .med [data-act="mdel"]');
  await settle(page);
  check('trash → delete_macro', (await lastCall(page, 'delete_macro'))?.args?.name === '連發' && (await page.locator('#board .med').count()) === 0);
  await page.evaluate(() => {
    const c = window.__mock.config;
    c.macros['<b>x"y</b>'] = { loop: false, steps: [] };
    c.pads.dualsense.buttons['0'] = { label: 'Cross', type: 'macro', value: '<b>x"y</b>' };
    window.__mockEmit('config-changed', { source: 'watcher' });
  });
  await settle(page);
  await page.click('#board .macros .mnew');
  await settle(page);
  await page.click('#board .med [data-act="mloop"]');
  const nSet = await countCalls(page, 'set_macro');
  await page.click('#board .med [data-act="mdone"]');
  await settle(page);
  check('完成 refuses a looping macro with no steps', (await countCalls(page, 'set_macro')) === nSet &&
    /至少一步/.test((await page.locator('#board .med .merr').textContent()) || ''));
  await page.click('#board .med [data-act="mdel"]');
  await settle(page);
  await page.evaluate(() => {
    window.__mock.config.macros['macro_attack_combo'] = { loop: false, steps: [{ key: 'z', action: 'down', delay_ms: [40, 60] }, { key: 'z', action: 'up', delay_ms: [15, 25] }] };
    window.__mockEmit('config-changed', { source: 'watcher' });
  });
  await settle(page);
  {
    // Final review I1: a name wider than its tile shows its start and ends in an ellipsis (not cut on both sides).
    await page.evaluate(() => {
      window.__mock.config.macros['火焰連擊大招必殺'] = { loop: false, steps: [{ key: 'z', action: 'down', delay_ms: [40, 60] }, { key: 'z', action: 'up', delay_ms: [15, 25] }] };
      window.__mockEmit('config-changed', { source: 'watcher' });
    });
    await settle(page);
    const starts = await page.evaluate(() => [...document.querySelectorAll('#board .mt .tile')].map(t => {
      const n = [...t.childNodes].find(c => c.nodeType === 3), r = document.createRange();
      r.setStart(n, 0); r.setEnd(n, 1);
      const a = r.getBoundingClientRect(), b = t.getBoundingClientRect();
      return { name: t.textContent, ok: a.left >= b.left + 1 && getComputedStyle(t).textOverflow === 'ellipsis' };
    }));
    check('long macro names show their first character (ellipsis at the end)', starts.length > 0 && starts.every(x => x.ok), JSON.stringify(starts));
    await page.evaluate(() => { delete window.__mock.config.macros['火焰連擊大招必殺']; window.__mockEmit('config-changed', { source: 'watcher' }); });
    await settle(page);
  }
  const nRename = await countCalls(page, 'rename_macro');
  await page.locator('#board [data-medit="macro_attack_combo"]').click();
  await settle(page);
  await page.click('#board .med [data-act="mdone"]');
  await settle(page);
  check('完成 on an untouched long-named macro does not rename it', (await countCalls(page, 'rename_macro')) === nRename &&
    (await lastCall(page, 'set_macro'))?.args?.name === 'macro_attack_combo', JSON.stringify((await lastCall(page, 'set_macro'))?.args?.name));
  check('macro name is escaped in tile and slot', (await page.locator('#board b').count()) === 0 && (await slotText(page, '0')) === '<b>x"y</b>');

  // ── Task 8: settings window ───────────────────────────────────────────────
  await page.click('#board [data-act="setopen"]');
  await settle(page);
  check('gear opens the settings window', (await page.locator('#board .setw').count()) === 1);
  const tabs = await page.locator('#board .stabs [data-stab]').allTextContents();
  check('four tabs 一般/搖桿/觸控板/按鍵', JSON.stringify(tabs.map(t => t.trim())) === '["一般","搖桿","觸控板","按鍵"]', tabs.join('|'));
  check('settings version line names PadToKBM 手把轉鍵鼠', /^PadToKBM 手把轉鍵鼠 v/.test(((await page.locator('#board .ver').textContent()) || '').trim()), await page.locator('#board .ver').textContent());
  check('一般 shows the config path and both open buttons', /padtokbm\.json/.test(await page.locator('#board .cfgp').textContent()) &&
    (await page.locator('#board [data-act="opencfg"]').count()) === 1 && (await page.locator('#board [data-act="opendir"]').count()) === 1);
  await page.click('#board [data-act="opencfg"]');
  await page.click('#board [data-act="opendir"]');
  await settle(page);
  check('用記事本開啟 / 開啟所在資料夾 invoke their commands', (await countCalls(page, 'open_config_in_editor')) === 1 && (await countCalls(page, 'open_config_folder')) === 1);
  for (let i = 0; i < 8; i++) {
    await page.evaluate(i => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'k' + i, action: i % 2 ? 'up' : 'down' }), i);
  }
  await settle(page);
  const rows = await page.locator('#board #logrows .lr').allTextContents();
  check('最近的事件: at most 6 rows, newest first', rows.length === 6 && /K7/i.test(rows[0]) && /放開/.test(rows[0]), rows.slice(0, 2).join(' | '));
  // v3.2.0: each row names its source (button glyph chip / macro chip).
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'j', action: 'down', source: { button: 3 } }));
  await settle(page);
  const r0 = page.locator('#board #logrows .lr').first();
  check('activity row shows the button chip', (await r0.locator('.chip.face svg').count()) === 1 && /→\s*送出 J 按下/.test(await r0.textContent()), await r0.innerHTML());
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'Space', action: 'down', source: { button: 9 } }));
  await settle(page);
  check('non-face button: grey pill with its tag', ((await page.locator('#board #logrows .lr').first().locator('.chip.pill').textContent()) || '').trim() === 'L1');
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'j', action: 'up', source: { macro: '連發' } }));
  await settle(page);
  const r1 = page.locator('#board #logrows .lr').first();
  check('macro row shows the macro chip', ((await r1.locator('.chip.mac').textContent()) || '').trim() === '連發' && /→\s*送出 J 放開/.test(await r1.textContent()), await r1.innerHTML());
  await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'macro-start', name: '連發', button: 0 }));
  await settle(page);
  const r2 = page.locator('#board #logrows .lr').first();
  await page.screenshot({ path: path.join(outDir, 'web-events.png') });
  check('macro start row: button chip → 巨集「連發」開始', (await r2.locator('.chip.face').count()) === 1 && /→\s*巨集「連發」開始/.test(await r2.textContent()), await r2.textContent());
  for (const t of ['stick', 'tp', 'timing', 'gen']) {
    await page.click(`#board [data-stab="${t}"]`);
    await settle(page);
  }
  check('every tab switches its pane', (await page.locator('#board [data-stab="gen"][aria-pressed="true"]').count()) === 1);
  await page.click('#board [data-stab="timing"]');
  await page.locator('#board [data-f="press0"]').fill('25');
  await page.locator('#board [data-f="press0"]').dispatchEvent('input');
  await page.locator('#board [data-f="press1"]').fill('25');
  await page.locator('#board [data-f="press1"]').dispatchEvent('input');
  await page.click('#board [data-stab="gen"]');
  const nSettings = await countCalls(page, 'set_settings');
  await page.click('#board [data-act="setsave"]');
  await settle(page);
  check('invalid 按住維持下限 is refused: message, outline, its tab', (await countCalls(page, 'set_settings')) === nSettings &&
    /按住維持下限/.test(await page.locator('#board .setw .err').textContent()) &&
    (await page.locator('#board [data-f="press0"][aria-invalid="true"]').count()) === 1 &&
    (await page.locator('#board [data-stab="timing"][aria-pressed="true"]').count()) === 1);
  await page.locator('#board [data-f="press1"]').fill('30');
  await page.locator('#board [data-f="press1"]').dispatchEvent('input');
  await page.click('#board [data-stab="stick"]');
  await page.locator('#board [data-f="deadzone"]').fill('0.6');
  await page.locator('#board [data-f="deadzone"]').dispatchEvent('input');
  await page.click('#board [data-act="setsave"]');
  await settle(page);
  const ss = await lastCall(page, 'set_settings');
  check('儲存 sends set_settings with the edits', Math.abs(ss?.args?.settings?.deadzone - 0.6) < 1e-6 &&
    JSON.stringify(ss.args.settings.min_press_ms) === '[25,30]' && (await page.locator('#board .setw').count()) === 0, JSON.stringify(ss?.args?.settings));
  await page.click('#board [data-act="setopen"]');
  await settle(page);
  await page.click('#board [data-stab="stick"]');
  await page.locator('#board [data-f="deadzone"]').fill('0.2');
  await page.locator('#board [data-f="deadzone"]').dispatchEvent('input');
  const nS2 = await countCalls(page, 'set_settings');
  await page.keyboard.press('Escape');
  await settle(page);
  check('Esc closes without saving', (await page.locator('#board .setw').count()) === 0 && (await countCalls(page, 'set_settings')) === nS2);
  await page.click('#board [data-act="setopen"]');
  await settle(page);
  await page.click('#board [data-stab="tp"]');
  await page.evaluate(() => window.__mockEmit('touchpad-click', { raw_x: 1200, raw_y: 300, quadrant: 26 }));
  await settle(page);
  check('touchpad dot follows the finger', (await page.locator('#board .tpad circle').count()) === 1);
  await page.click('#board [data-act="tpmid"]');
  await settle(page);
  check('中心設在手指位置 copies the finger position', (await page.locator('#board [data-f="midX"]').inputValue()) === '1200' &&
    (await page.locator('#board [data-f="midY"]').inputValue()) === '300');
  await page.click('#board .setw .x');
  await settle(page);
  check('✕ closes the window', (await page.locator('#board .setw').count()) === 0);

  // ── Stick calibration (設定 → 搖桿) ───────────────────────────────────────
  await page.click('#board [data-pad="dualsense"]');
  await page.evaluate(() => { window.__mock.sticks = null; });
  await page.click('#board [data-act="setopen"]');
  await page.click('#board [data-stab="stick"]');
  await page.waitForTimeout(150);
  const dots = await page.locator('#board .stk .sdot').evaluateAll(els => els.map(e => [e.getAttribute('cx'), e.getAttribute('cy')]));
  check('no pad: centred dots, calibrate disabled', dots.length === 2 && dots.every(d => +d[0] === 0 && +d[1] === 0) &&
    await page.locator('#board [data-act="calgo"]').isDisabled() && /先連上這支手把/.test(await page.locator('#board #calbar').textContent()), JSON.stringify(dots));
  await page.evaluate(() => { window.__mock.sticks = { pad: 'dualsense', raw: [0.05, -0.02, 0.05, -0.02], cal: [0.05, -0.02, 0.05, -0.02] }; });
  await page.waitForTimeout(150);
  check('pad streaming: calibrate enabled', await page.locator('#board [data-act="calgo"]').isEnabled());
  await page.click('#board [data-act="calgo"]');
  await page.waitForTimeout(150);
  check('step 1 asks to let go of both sticks', /1／3/.test(await page.locator('#board #calbar').textContent()) && /放開兩支搖桿/.test(await page.locator('#board #calbar').textContent()));
  await page.waitForTimeout(3300);
  const bar2 = await page.locator('#board #calbar').textContent();
  check('step 2 after 3 s of rest; crosshair hidden', /2／3/.test(bar2) && /八個方向/.test(bar2) && (await page.locator('#board .stk .xhair').count()) === 0, bar2);
  await page.evaluate(async () => {
    for (let a = 0; a <= 720; a += 15) {
      const x = 0.05 + 0.9 * Math.cos(a * Math.PI / 180), y = -0.02 + 0.9 * Math.sin(a * Math.PI / 180);
      window.__mock.sticks = { pad: 'dualsense', raw: [x, y, x, y], cal: [x, y, x, y] };
      await new Promise(r => setTimeout(r, 40));
    }
  });
  await page.waitForTimeout(150);
  const bar3 = await page.locator('#board #calbar').textContent();
  check('step 3 shows the result and 重來', /完成/.test(bar3) && /儲存/.test(bar3) && (await page.locator('#board [data-act="calstop"]').textContent()).includes('重來'), bar3);
  await page.click('#board [data-act="setsave"]');
  await page.waitForTimeout(200);
  const sc = await lastCall(page, 'save_stick_cal');
  check('儲存 sends save_stick_cal with the captured centre', sc?.args?.pad === 'dualsense' && Math.abs(sc.args.cal.left.x.center - 0.05) < 0.01 &&
    Math.abs(sc.args.cal.left.x.pos - 0.9) < 0.05, JSON.stringify(sc?.args?.cal?.left));
  await page.click('#board [data-act="setopen"]');
  await page.click('#board [data-stab="stick"]');
  await page.click('#board [data-act="calfac"]');
  await page.waitForTimeout(150);
  check('恢復出廠校正 → reset_stick_cal', (await lastCall(page, 'reset_stick_cal'))?.args?.pad === 'dualsense');
  await page.click('#board [data-act="calgo"]');
  await page.evaluate(() => { window.__mock.sticks = { pad: 'ns2_pro', raw: [0, 0, 0, 0], cal: [0, 0, 0, 0] }; });
  await page.waitForTimeout(200);
  check('capture cancels when another pad takes over', /手把換了/.test(await page.locator('#board #calbar').textContent()));
  await page.keyboard.press('Escape');
  await page.evaluate(() => { window.__mock.sticks = null; });

  // @@NEXT-TASKS

  const inPageErrors = await page.evaluate(() => window.__mockErrors);
  const otherMisses = httpMisses.filter(p => p !== '/favicon.ico');
  check('zero pageerror', pageErrors.length === 0, pageErrors.join(' | '));
  check('served under the app CSP', csp.includes("script-src 'self'"), csp);
  check('zero console errors', consoleErrors.length === 0, consoleErrors.join(' | '));
  check('zero window error / unhandledrejection', inPageErrors.length === 0, inPageErrors.join(' | '));
  check('no unexpected dialogs (alert/confirm/prompt)', dialogs.length === 0, dialogs.join(' | '));
  check('no 404 except favicon', otherMisses.length === 0, otherMisses.join(' '));
  const seen = [...new Set((await calls(page)).map(c => c.cmd))].sort();
  console.log(`info: IPC calls seen: ${seen.join(', ')}`);
} catch (e) {
  check('click-through ran to completion', false, String(e && (e.stack || e)));
} finally {
  await browser.close();
  server.close();
}

const failed = checks.filter(c => !c.ok).length;
console.log(`web-smoke: ${checks.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
