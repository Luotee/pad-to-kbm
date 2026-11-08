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
import { pathToFileURL } from 'node:url';

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
    // `battery` as the engine sends it (v3.8.0): empty here so the first-load lamp / header checks see the green
    // lamp; the battery checks below send readings.
    status: { name: 'DualSense', transport: 'BT', kind: 'dualsense', warnings: [], connected: ['dualsense'], off_pads: [], battery: {} },
    sticks: null,
    // get_xbox_status reply (drivers + whether the selected pad is hidden yet) and what each
    // installer leaves behind (install_vigem / install_hidhide reply).
    xbox: { vigem: 'installed', hidhide: 'installed', hidden: true, auto_off: false, replug_usb: false },
    after: { vigem: 'installed', hidhide: 'installed' },
    // v3.6.0 ui prefs kept across a page.reload() (sessionStorage), as the config file would be.
    dismissed: false,
  };
  try {
    const keep = JSON.parse(sessionStorage.getItem('__keep') || 'null');
    if (keep) {
      Object.assign(state.xbox, keep.xbox || {});
      state.dismissed = !!keep.dismissed;
      if (Array.isArray(keep.connected)) state.status.connected = keep.connected;   // [] = no pad connected at load
    }
  } catch (e) { /* no storage */ }
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
      case 'get_ui_prefs': return { last_pad: c.last_pad ?? null, hidhide_prompt_dismissed: state.dismissed };
      case 'set_ui_prefs':   // v3.4.0: last_pad is a config field; a change emits config-changed marked ui
        if (a.prefs.hidhide_prompt_dismissed !== undefined) state.dismissed = !!a.prefs.hidhide_prompt_dismissed;
        if (a.prefs.last_pad && a.prefs.last_pad !== c.last_pad) {
          c.last_pad = a.prefs.last_pad;
          setTimeout(() => emit('config-changed', { source: 'ipc', ui: true }), 0);
        }
        return null;
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
      case 'set_output_mode':   // as ConfigDoc::set_output_mode: the slug's legacy mapping_enabled goes
        c.output_mode = { ...(c.output_mode || {}), [a.kind]: a.mode };
        if (c.mapping_enabled) { delete c.mapping_enabled[a.kind]; if (!Object.keys(c.mapping_enabled).length) delete c.mapping_enabled; }
        changed(); return null;
      case 'get_xbox_status': if (state.slowXbox) await new Promise(r => setTimeout(r, 200)); return clone(state.xbox);
      case 'get_vigem_status': return state.xbox.vigem;
      case 'install_vigem': case 'install_hidhide': {
        const id = cmd === 'install_vigem' ? 'vigem' : 'hidhide';
        await new Promise(r => setTimeout(r, 150));
        state.xbox[id] = state.after[id];
        return state.xbox[id];
      }
      case 'set_pad_off': {
        // Runtime only (no config-changed); the engine re-sends the status.
        const off = new Set(state.status.off_pads || []);
        if (a.off) off.add(a.kind); else off.delete(a.kind);
        state.status.off_pads = ['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano'].filter(k => off.has(k));
        setTimeout(() => emit('controller-status', { connected: true, ...clone(state.status), connected_pads: state.status.connected }), 0);
        return clone(state.status.off_pads);
      }
      case 'set_lightbar': c.lightbar = clone(a.lightbar); changed(); return null;
      case 'set_rumble_on_connect': c.rumble_on_connect = a.enabled; changed(); return null;
      case 'save_stick_cal': c.stick_cal = c.stick_cal || {}; c.stick_cal[a.pad] = clone(a.cal); changed(); return null;
      case 'reset_stick_cal': if (c.stick_cal) delete c.stick_cal[a.pad]; changed(); return null;
      // v3.6.0 remembered HidHide pads, as the Rust side: a present pad is
      // kept (刪除 refused, 全部刪除 skips it); config-changed only when something went.
      case 'get_present_pad_ids': return clone(state.presentIds || []);
      case 'forget_hidden_pad': case 'forget_all_hidden_pads': {
        const lc = s => String(s).toLowerCase(), here = new Set((state.presentIds || []).map(lc));
        if (cmd === 'forget_hidden_pad' && here.has(lc(a.id))) throw '連線中的手把要先關掉才能刪除';
        const list = c.hidhide_known || [];
        const next = cmd === 'forget_hidden_pad' ? list.filter(k => lc(k.id) !== lc(a.id)) : list.filter(k => here.has(lc(k.id)));
        if (next.length !== list.length) { if (next.length) c.hidhide_known = next; else delete c.hidhide_known; changed(); }
        return null;
      }
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

// v3.7.0 pad-art measurements, run in the page on the board's own <svg class="pad"> (paths carry
// data-role, labels sit in <g data-glyph>). Ports of the assets' tools/labels.html analyse() and
// tools/lineup4.html metrics(), in pad units:
// - labels: each L／R label's ink to the nearest edge of its visible key face, on an 8x raster with a
//   chamfer distance transform. Shoulder face = fill minus its own outline; trigger face = fill minus its
//   outline, the body and the shoulder (fill + outline). The right side is mirrored onto the left.
// - gap: the shoulder band (bump / bumpLite / trig, fill + outline) to the nearest key (face, dkey,
//   dcross, dwell, small, well, cap; fill + outline) on a 4x raster. `skip` lists centres of small keys left out
//   of `keys` (DualSense Create / Options, which the user accepted at their measured distance); the
//   touchpad is not a key here (the DualSense shoulder is drawn to meet it). `skipped` = those keys alone.
function padArtMetrics(opt) {
  const root = document.querySelector('#board svg.pad');
  const paths = [...root.querySelectorAll('path[data-role]')];
  const matOf = el => { const m = root.getScreenCTM().inverse().multiply(el.getScreenCTM()); return new DOMMatrix([m.a, m.b, m.c, m.d, m.e, m.f]); };
  const MIR = new DOMMatrix([-1, 0, 0, 1, 300, 0]);
  const centre = el => { const r = el.getBBox(); return matOf(el).transformPoint(new DOMPoint(r.x + r.width / 2, r.y + r.height / 2)); };
  function raster(K, X0, Y0, W, H) {
    const mk = () => { const c = document.createElement('canvas'); c.width = W; c.height = H; return c.getContext('2d', { willReadFrequently: true }); };
    const draw = (c, el, mode, pre) => {
      let m = matOf(el); if (pre) m = pre.multiply(m);
      c.setTransform(K * m.a, K * m.b, K * m.c, K * m.d, K * (m.e - X0), K * (m.f - Y0));
      const p = new Path2D(el.getAttribute('d')), sw = +(el.getAttribute('stroke-width') || el.parentNode.getAttribute('stroke-width') || 0);
      c.fillStyle = '#000'; c.strokeStyle = '#000'; c.lineJoin = 'round'; c.lineCap = 'round';
      if (mode !== 'stroke') c.fill(p);
      if (mode !== 'fill' && sw) { c.lineWidth = sw; c.stroke(p); }
    };
    const bits = c => { const d = c.getImageData(0, 0, W, H).data, a = new Uint8Array(W * H); for (let i = 0; i < a.length; i++) a[i] = d[i * 4 + 3] > 100 ? 1 : 0; return a; };
    // Chamfer distance (px) to the nearest pixel whose value is `to` (0: inside distance of a set region).
    const dist = (m, to) => {
      const INF = 1e9, d = new Float32Array(W * H), b = Math.SQRT2;
      for (let i = 0; i < d.length; i++) d[i] = m[i] === to ? 0 : INF;
      for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) { const i = y * W + x; if (!d[i]) continue; let v = d[i];
        if (x > 0) v = Math.min(v, d[i - 1] + 1); if (y > 0) { v = Math.min(v, d[i - W] + 1); if (x > 0) v = Math.min(v, d[i - W - 1] + b); if (x < W - 1) v = Math.min(v, d[i - W + 1] + b); } d[i] = v; }
      for (let y = H - 1; y >= 0; y--) for (let x = W - 1; x >= 0; x--) { const i = y * W + x; if (!d[i]) continue; let v = d[i];
        if (x < W - 1) v = Math.min(v, d[i + 1] + 1); if (y < H - 1) { v = Math.min(v, d[i + W] + 1); if (x < W - 1) v = Math.min(v, d[i + W + 1] + b); if (x > 0) v = Math.min(v, d[i + W - 1] + b); } d[i] = v; }
      return d;
    };
    return { mk, draw, bits, dist };
  }
  const labels = [];
  {
    const K = 8, R = raster(K, -30, -30, 190 * K, 160 * K);
    for (const right of [false, true]) {
      const pre = right ? MIR : null, mine = (...roles) => paths.filter(e => roles.includes(e.dataset.role) && (centre(e).x < 150) !== right);
      const trig = R.mk(), trigS = R.mk(), cover = R.mk(), front = R.mk(), frontS = R.mk();
      for (const e of mine('trig')) { R.draw(trig, e, 'fill', pre); R.draw(trigS, e, 'stroke', pre); }
      for (const e of [...paths.filter(e => e.dataset.role === 'body'), ...mine('bump', 'bumpLite')]) R.draw(cover, e, 'both', pre);
      for (const e of mine('bump', 'bumpLite')) { R.draw(front, e, 'fill', pre); R.draw(frontS, e, 'stroke', pre); }
      const T = R.bits(trig), TS = R.bits(trigS), C = R.bits(cover), F = R.bits(front), FS = R.bits(frontS);
      const visT = new Uint8Array(T.length), visF = new Uint8Array(T.length);
      for (let i = 0; i < T.length; i++) { visT[i] = T[i] && !TS[i] && !C[i] ? 1 : 0; visF[i] = F[i] && !FS[i] ? 1 : 0; }
      const dT = R.dist(visT, 0), dF = R.dist(visF, 0);
      for (const g of root.querySelectorAll('g[data-glyph]')) {
        const role = g.dataset.glyph;
        if (!/^(trigT|bumpT|bumpLiteT)$/.test(role) || (+g.dataset.x >= 150) !== right) continue;
        const ink = R.mk(); for (const p of g.querySelectorAll('path')) R.draw(ink, p, 'stroke', pre);
        const I = R.bits(ink), D = role === 'trigT' ? dT : dF;
        let clr = 1e9, n = 0; for (let i = 0; i < I.length; i++) if (I[i]) { n++; clr = Math.min(clr, D[i]); }
        labels.push({ at: (right ? 'R ' : 'L ') + role, clr: n ? +(clr / K).toFixed(2) : -1 });
      }
    }
  }
  let gap = null;
  {
    const K = 4, R = raster(K, -40, -40, 380 * K, 300 * K), KEY = ['face', 'dkey', 'dcross', 'dwell', 'small', 'well', 'cap'], BAND = ['bump', 'bumpLite', 'trig'];
    const skipped = e => e.dataset.role === 'small' && (opt.skip || []).some(q => { const c = centre(e); return Math.abs(c.x - q[0]) <= 1 && Math.abs(c.y - q[1]) <= 1; });
    const keys = R.mk(), skip = R.mk(), band = R.mk();
    for (const e of paths) {
      if (BAND.includes(e.dataset.role)) R.draw(band, e, 'both');
      if (KEY.includes(e.dataset.role)) R.draw(skipped(e) ? skip : keys, e, 'both');
    }
    const B = R.bits(band), near = d => { let v = -K; for (let i = 0; i < B.length; i++) if (B[i]) v = v < 0 ? d[i] : Math.min(v, d[i]); return +(v / K).toFixed(2); };   // -1: no shoulder drawn
    const S = R.bits(skip);
    gap = { keys: near(R.dist(R.bits(keys), 1)), skipped: S.some(x => x) ? near(R.dist(S, 1)) : null };
  }
  return { labels, gap };
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
  check('four pad buttons', JSON.stringify(padNames.map(s => s.trim())) === JSON.stringify(['DualSense', 'Switch Pro', 'Switch 2 Pro', 'Xbox Nano']), padNames.join('|'));
  const lights = await page.locator('#board .hdr [data-pad] .live').evaluateAll(els => els.map(e => e.classList.contains('idle') ? 'idle' : 'live'));
  check('DualSense light live, others idle', JSON.stringify(lights) === '["live","idle","idle","idle"]', lights.join(','));
  check('one ⏻ (connected pad only)', (await page.locator('#board .hdr .pwr').count()) === 1);
  check('DualSense ⏻ title is 斷開手把', (await page.locator('#board .hdr .pwr').getAttribute('title')) === '斷開手把');

  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  const ns2Slots = await page.locator('#board .slot').evaluateAll(els => els.map(e => e.dataset.slot));
  check('Switch 2 Pro shows 30/31/32 and no touchpad slots', ['30', '31', '32'].every(k => ns2Slots.includes(k)) && !ns2Slots.includes('25'));
  check('pad choice saved as last_pad', (await lastCall(page, 'set_ui_prefs'))?.args?.prefs?.last_pad === 'ns2_pro');
  check('Switch 2 Pro maps to keys in the example config (v3.2.0 default)', !(await page.locator('#board').getAttribute('class')).split(' ').includes('off'));
  check('example config carries output_mode, no mapping_enabled', !!exampleConfig.output_mode && exampleConfig.mapping_enabled === undefined);
  await page.evaluate(() => { delete window.__mock.config.output_mode.ns2_pro; });
  await page.evaluate(() => window.__mockEmit('config-changed', { source: 'external' }));
  await settle(page);
  check('missing output_mode.ns2_pro → keys (not off)', !(await page.locator('#board').getAttribute('class')).split(' ').includes('off'));
  await page.evaluate(() => { window.__mock.config.output_mode.ns2_pro = 'kbm'; });

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
  const nPt = await countCalls(page, 'set_pad_tables');
  await page.keyboard.press('Control+z');
  await settle(page);
  check('Ctrl+Z is no shortcut (v3.4.0)', (await countCalls(page, 'set_pad_tables')) === nPt);
  await page.click('#board [data-act="undo"]');
  await settle(page);
  const ux = await lastCall(page, 'set_pad_tables');
  const shownPad = await page.locator('#board .hdr [data-pad][aria-pressed="true"]').getAttribute('data-pad');
  check('undo switches to the pad it changes (復原)', ux?.args?.pad === 'dualsense' && shownPad === 'dualsense', `${ux?.args?.pad} ${shownPad}`);

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
  // Output capsule 關｜鍵鼠｜手把 (v3.2.0); Xbox for every pad (v3.6.0).
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
  check('capsule shows 鍵鼠 for a fresh config', (await modeOf()) === 'kbm' && (await page.locator('#board .omode button').count()) === 4);
  check('no 鍵鼠對應 switch any more', (await page.locator('#board [data-act="map"]').count()) === 0);
  // v3.6.0: 關｜鍵鼠｜Xbox｜手把, left to right, on every pad.
  const segs = () => page.evaluate(() => [...document.querySelectorAll('#board .omode button')].map(b => {
    const r = b.getBoundingClientRect();
    return { mode: b.dataset.mode, text: b.textContent.trim(), dis: b.getAttribute('aria-disabled'), title: b.title, l: r.left, r: r.right };
  }));
  {
    const sg = await segs();
    check('capsule segments 關 / 鍵鼠 / Xbox / 手把 in order', JSON.stringify(sg.map(x => x.mode + ':' + x.text)) === '["off:關","kbm:鍵鼠","xbox:Xbox","pad:手把"]' &&
      sg.every((x, i) => i === 0 || x.l >= sg[i - 1].r - 0.5), JSON.stringify(sg.map(x => [x.mode, x.l, x.r])));
    check('DualSense: no segment disabled; Xbox title names both drivers', sg.every(x => x.dis === null) && sg[2].title === '遊戲看到一支 Xbox 手把（需要 ViGEmBus、HidHide）', JSON.stringify(sg));
    const fits = await page.evaluate(() => {
      const o = document.querySelector('#board .omode').getBoundingClientRect(), h = document.querySelector('#board .hdr').getBoundingClientRect();
      const kids = [...document.querySelectorAll('#board .omode button')].map(b => b.getBoundingClientRect());
      return o.left >= h.left && o.right <= h.right && kids.every(k => k.left >= o.left - 0.5 && k.right <= o.right + 0.5);
    });
    check('capsule fits inside the header, segments inside the capsule', fits);
  }
  await page.click('#board .omode [data-mode="off"]');
  await settle(page);
  const po = await lastCall(page, 'set_pad_off');
  check('關 sends set_pad_off(pad,true)', po?.args?.kind === 'dualsense' && po.args.off === true, JSON.stringify(po?.args));
  check('關 shows the gold banner', (await banner()) === '這支手把已關閉' && (await page.locator('#board .obanner.gold').count()) === 1 && (await modeOf()) === 'off', await banner());
  check('關 dims the board', await dimmed());
  await page.screenshot({ path: path.join(outDir, 'web-mode-off.png') });
  check('關 does not write the config', (await countCalls(page, 'set_output_mode')) === 0);
  // Not 鍵鼠 → the board is locked (v3.2.1): nothing below the header edits.
  check('關 banner sits at the stage centre (490, 228 on the board)', await bannerCentred(), JSON.stringify(await bannerBox()));
  await expectLocked('關');
  const xs0 = await countCalls(page, 'get_xbox_status');   // 鍵鼠 on a native pad already looked once at load (v3.6.0)
  await page.click('#board .omode [data-mode="pad"]');
  await settle(page);
  const offBack = await lastCall(page, 'set_pad_off'), me = await lastCall(page, 'set_output_mode');
  check('手把 clears 關 (set_pad_off false)', offBack?.args?.kind === 'dualsense' && offBack.args.off === false, JSON.stringify(offBack?.args));
  check('手把 sends set_output_mode(dualsense, pad)', me?.args?.kind === 'dualsense' && me.args.mode === 'pad', JSON.stringify(me?.args));
  check('手把 shows the white banner', (await banner()) === '程式不送東西，遊戲直接用手把；下面的鍵鼠綁定這時不作用' && (await page.locator('#board .obanner.white').count()) === 1, await banner());
  check('手把 dims the board', (await dimmed()) && (await modeOf()) === 'pad');
  check('手把 banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
  // (the transient 鍵鼠 paint between 關 and 手把 may refresh the status quietly; it must not prompt or install)
  check('手把 asks nothing about Xbox drivers', (await page.locator('#board .vgpop, #board .vgbang').count()) === 0 && (await calls(page)).every(c => !/^install_/.test(c.cmd)), `${xs0} → ${await countCalls(page, 'get_xbox_status')}`);
  await expectLocked('手把');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  const mk = await lastCall(page, 'set_output_mode');
  check('鍵鼠 sends set_output_mode(dualsense, kbm)', mk?.args?.kind === 'dualsense' && mk.args.mode === 'kbm', JSON.stringify(mk?.args));
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
  // Pre-3.6.0 configs: a pad missing from output_mode falls back to mapping_enabled (false = 手把).
  await page.evaluate(() => { const c = window.__mock.config; delete c.output_mode.dualsense; c.mapping_enabled = { dualsense: false }; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  check('legacy mapping_enabled.dualsense=false shows 手把', (await modeOf()) === 'pad');
  await page.evaluate(() => { const c = window.__mock.config; delete c.mapping_enabled; c.output_mode.dualsense = 'kbm'; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  // Switch 2 Pro: Windows has no driver for it, so 手把 stays in place but greyed and inert.
  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  {
    const sg = await segs();
    check('NS2: same four segments, 手把 aria-disabled with the reason as its title',
      JSON.stringify(sg.map(x => x.mode + ':' + x.text + ':' + x.dis)) === '["off:關:null","kbm:鍵鼠:null","xbox:Xbox:null","pad:手把:true"]' &&
      sg[3].title === 'Windows 認不得 Switch 2 Pro，遊戲讀不到它；要用手把玩請選 Xbox' && sg[2].title === '遊戲看到一支 Xbox 手把（需要 ViGEmBus）', JSON.stringify(sg));
    check('NS2 手把 is drawn grey', await page.evaluate(() => getComputedStyle(document.querySelector('#board .omode [data-mode="pad"]')).color === 'rgb(169, 180, 192)'));
    const n0 = (await calls(page)).length;
    await page.click('#board .omode [data-mode="pad"]', { force: true });
    await settle(page);
    check('NS2 手把 click sends no IPC and keeps 鍵鼠', (await calls(page)).length === n0 && (await modeOf()) === 'kbm');
  }
  await page.evaluate(() => { const c = window.__mock.config; delete c.output_mode.ns2_pro; c.mapping_enabled = { ns2_pro: false }; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  check('legacy mapping_enabled.ns2_pro=false shows Xbox', (await modeOf()) === 'xbox');
  await page.evaluate(() => { const c = window.__mock.config; delete c.mapping_enabled; c.output_mode.ns2_pro = 'xbox'; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  await page.screenshot({ path: path.join(outDir, 'web-mode-xbox.png') });
  check('NS2 in Xbox mode shows the Xbox banner', (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
  check('Xbox banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
  check('a pad in Xbox mode asks get_xbox_status for itself', (await lastCall(page, 'get_xbox_status'))?.args?.kind === 'ns2_pro');
  await expectLocked('Xbox');
  await page.evaluate(() => { window.__mock.config.output_mode.ns2_pro = 'kbm'; window.__mockEmit('config-changed', { source: 'external' }); });
  await settle(page);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: ['ns2_pro'] }); });
  await settle(page);
  check('off_pads from controller-status selects 關', (await modeOf()) === 'off');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);

  // ── v3.6.0: Xbox output prompt (ViGEmBus for every pad, HidHide for DualSense / Switch Pro) ──
  const pop = () => page.locator('#board .vgpop');
  const popBtns = () => page.locator('#board .vgpop button').allTextContents();
  const popRows = () => page.locator('#board .vgpop .vgl li').evaluateAll(els => els.map(e => e.querySelector('.vgn').textContent + ':' + e.querySelector('.vgs').textContent));
  const bang = () => page.locator('#board .hdr .vgbang');
  const setXbox = (x, after) => page.evaluate(([x, after]) => { Object.assign(window.__mock.xbox, x); if (after) Object.assign(window.__mock.after, after); }, [x, after || null]);
  const installs = async () => (await calls(page)).filter(c => /^install_/.test(c.cmd)).map(c => c.cmd);
  // The pointer's centre (the rotated square's centre) against the Xbox segment's centre, in board px.
  const pointer = () => page.evaluate(() => {
    const p = document.querySelector('#board .vgpop'), x = document.querySelector('#board .omode [data-mode="xbox"]');
    const pr = p.getBoundingClientRect(), xr = x.getBoundingClientRect(), b = document.getElementById('board').getBoundingClientRect();
    const cs = getComputedStyle(p), bs = getComputedStyle(p, '::before');
    const w = parseFloat(bs.width) + parseFloat(bs.borderLeftWidth) + parseFloat(bs.borderRightWidth);
    const apex = pr.right - parseFloat(cs.borderRightWidth) - parseFloat(bs.right) - w / 2;
    return { apex: apex - b.left, seg: (xr.left + xr.right) / 2 - b.left, l: xr.left - b.left, r: xr.right - b.left, popL: pr.left - b.left, popR: pr.right - b.left };
  });
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);
  await setXbox({ vigem: 'missing', hidhide: 'missing', hidden: null }, { vigem: 'installed', hidhide: 'installed' });
  await page.click('#board .omode [data-mode="xbox"]');
  await settle(page);
  {
    const xm = await lastCall(page, 'set_output_mode');
    check('Xbox on DualSense sends set_output_mode(dualsense, xbox)', xm?.args?.kind === 'dualsense' && xm.args.mode === 'xbox', JSON.stringify(xm?.args));
    check('Xbox with both drivers missing opens the popover', (await pop().count()) === 1 && (await lastCall(page, 'get_xbox_status'))?.args?.kind === 'dualsense');
    const t = ((await pop().textContent()) || '');
    check('DualSense popover: one row per driver, both 未安裝', JSON.stringify(await popRows()) === '["ViGEmBus:未安裝","HidHide:未安裝"]' &&
      t.includes('要讓遊戲把 DualSense 當成 Xbox 手把，需要這兩個：') && t.includes('把原本的 DualSense 對遊戲藏起來') && t.includes('都是官方安裝程式，各約 1 分鐘'), t);
    check('popover has 稍後 / 改回鍵鼠 / 安裝這兩個', JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '改回鍵鼠', '安裝這兩個']), JSON.stringify(await popBtns()));
    const pt = await pointer();
    check('popover pointer aims at the Xbox segment (apex within 0.5 px of its centre, inside it)', Math.abs(pt.apex - pt.seg) <= 0.5 && pt.apex > pt.l && pt.apex < pt.r, JSON.stringify(pt));
    check('popover lies inside the board', pt.popL >= 0 && pt.popR <= 980, JSON.stringify(pt));
    check('no banner under the popover while drivers are missing', (await page.locator('#board .obanner').count()) === 0);
    await page.screenshot({ path: path.join(outDir, 'web-xbox-both.png') });
  }
  await page.click('#board [data-act="vglater"]');
  await settle(page);
  check('稍後 closes it and leaves the !', (await pop().count()) === 0 && (await bang().count()) === 1);
  {
    // A config-changed / controller-status refresh keeps a popover the user closed closed.
    const n = await countCalls(page, 'get_xbox_status');
    await page.evaluate(() => window.__mockEmit('config-changed', { source: 'external' }));
    await settle(page);
    check('a refresh after 稍後 re-probes once and does not reopen', (await countCalls(page, 'get_xbox_status')) === n + 1 && (await pop().count()) === 0 && (await bang().count()) === 1,
      `${n} → ${await countCalls(page, 'get_xbox_status')}`);
  }
  await bang().click();
  await settle(page);
  check('the ! reopens it', (await pop().count()) === 1);
  // A clean run: ViGEmBus, then HidHide, then one status probe; the popover closes.
  {
    const n0 = (await calls(page)).length;
    await page.click('#board [data-act="vginstall"]');
    await page.waitForTimeout(40);
    check('安裝 shows 安裝中… with every button disabled', ((await page.locator('#board [data-act="vginstall"]').textContent()) || '').trim() === '安裝中…' &&
      (await page.locator('#board .vgpop button:disabled').count()) === 3);
    await page.waitForTimeout(450);
    const seq = (await calls(page)).slice(n0).map(c => c.cmd).filter(c => /^install_|^get_xbox_status$/.test(c));
    check('安裝這兩個 runs install_vigem, install_hidhide, then get_xbox_status', JSON.stringify(seq.slice(0, 3)) === '["install_vigem","install_hidhide","get_xbox_status"]', JSON.stringify(seq));
    check('installed → popover and ! gone, Xbox banner shows', (await pop().count()) === 0 && (await bang().count()) === 0 &&
      (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
  }
  // The pad was present before it was hidden: the reconnect hint, until a reconnect.
  await setXbox({ hidden: false });
  await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
  await settle(page);
  {
    const t = await banner();
    check('hidden:false shows the reconnect banner (with the ⏻)', t === '再連一次 DualSense，遊戲才看不到原本的手把：按分頁上的  斷開，再按 PS 鍵連回來' &&
      (await page.locator('#board .obanner.rc svg').count()) === 1, t);
    check('reconnect banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
    const fits = await page.evaluate(() => { const b = document.querySelector('#board .obanner.rc').getBoundingClientRect(), s = document.getElementById('board').getBoundingClientRect(); return b.left >= s.left && b.right <= s.right; });
    check('reconnect banner fits the board', fits);
    check('Xbox reconnect banner has no 這次不再提醒', (await page.locator('#board .obanner.rc [data-act="rcdismiss"]').count()) === 0);
    await page.screenshot({ path: path.join(outDir, 'web-xbox-reconnect.png') });
    // A USB pad (`replug_usb`): only unplugging makes a game drop it, and the ⏻ would drop a
    // Bluetooth pad of the same type: the banner says unplug, no ⏻.
    await setXbox({ replug_usb: true });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await settle(page);
    const u = await banner();
    check('replug_usb: the reconnect banner says unplug the USB cable, no ⏻', u === '再連一次 DualSense，遊戲才看不到原本的手把：拔掉 USB 線再插回去' &&
      (await page.locator('#board .obanner.rc svg').count()) === 0, u);
    check('replug_usb: banner sits at the stage centre and fits', await bannerCentred(), JSON.stringify(await bannerBox()));
    await page.screenshot({ path: path.join(outDir, 'web-xbox-reconnect-usb.png') });
    await setXbox({ replug_usb: false });
  }
  {
    await setXbox({ hidden: true });
    const n = await countCalls(page, 'get_xbox_status');
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await settle(page);
    check('hidden:true after a reconnect: one re-probe, the reconnect banner clears', (await countCalls(page, 'get_xbox_status')) === n + 1 &&
      (await page.locator('#board .obanner.rc').count()) === 0 && (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', `${n} → ${await countCalls(page, 'get_xbox_status')} ${await banner()}`);
  }
  {
    // Events while a probe is in flight: no second probe at once, one follow-up after it.
    await page.evaluate(() => { const s = window.__mock; s.slowXbox = true; });
    const n = await countCalls(page, 'get_xbox_status');
    await page.evaluate(() => { for (let i = 0; i < 5; i++) window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }); });
    await page.waitForTimeout(500);
    const m = await countCalls(page, 'get_xbox_status');
    check('five events during a slow probe → at most two probes', m - n >= 1 && m - n <= 2, `${n} → ${m}`);
    await page.evaluate(() => { window.__mock.slowXbox = false; });
  }
  // The app powered the pad off itself (it needed a replug): a white banner while it is away,
  // no 這次不再提醒; gone once it is back. Xbox and 鍵鼠 alike.
  {
    const aoText = name => '已替你把 ' + name + ' 關機，好讓遊戲看不到它。按 ' + (name === 'DualSense' ? 'PS' : 'HOME') + ' 鍵打開就能用了';
    const away = () => page.evaluate(() => window.__mockEmit('controller-status', { connected: false, kind: null, connected_pads: [], warnings: [], off_pads: [] }));
    const back = kind => page.evaluate(k => window.__mockEmit('controller-status', { connected: true, kind: k, connected_pads: [k], warnings: [], off_pads: [] }), kind);
    await setXbox({ hidden: true, auto_off: true });
    await away();
    await settle(page);
    const t = await banner();
    check('auto-off: DualSense away → the power-off banner', t === aoText('DualSense') && (await page.locator('#board .obanner.ao').count()) === 1, t);
    check('auto-off: no 這次不再提醒, no ⏻ icon', (await page.locator('#board .obanner [data-act="rcdismiss"]').count()) === 0 && (await page.locator('#board .obanner svg').count()) === 0);
    check('auto-off: banner sits at the stage centre', await bannerCentred(), JSON.stringify(await bannerBox()));
    const m = await page.evaluate(() => {
      const b = document.querySelector('#board .obanner.ao'), sp = b.querySelector('span'), br = b.getBoundingClientRect(), sr = sp.getBoundingClientRect();
      const s = document.getElementById('board').getBoundingClientRect(), cs = getComputedStyle(b);
      return { bl: br.left, br: br.right, bt: br.top, bb: br.bottom, sl: sr.left, sr: sr.right, st: sr.top, sb: sr.bottom, kl: s.left, kr: s.right,
        inner: [br.top + parseFloat(cs.borderTopWidth), br.bottom - parseFloat(cs.borderBottomWidth)], fs: parseFloat(getComputedStyle(sp).fontSize) };
    });
    const J = JSON.stringify(m);
    check('auto-off: text inside the banner, one line', m.sl >= m.bl && m.sr <= m.br && m.st >= m.bt && m.sb <= m.bb && m.sb - m.st < 2 * m.fs, J);
    check('auto-off: text vertically centred', Math.abs((m.st - m.inner[0]) - (m.inner[1] - m.sb)) <= 1, J);
    check('auto-off: banner on whole pixels, inside the board', [m.bl, m.br, m.bt, m.bb].every(v => Math.abs(v - Math.round(v)) < 0.01) && m.bl >= m.kl && m.br <= m.kr, J);
    await page.screenshot({ path: path.join(outDir, 'web-auto-off.png') });
    // A DualSense connected (the reply still says auto_off): no banner. This is the pad back on, or a
    // second DualSense in use while the one the app powered off is still away (auto_off is per kind).
    await back('dualsense');
    await settle(page);
    check('auto-off: a DualSense connected (that pad back, or another one in use) keeps it quiet', (await page.locator('#board .obanner.ao').count()) === 0 && (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
    // The power-off dropped no link (USB): the old reconnect banner, as before.
    await setXbox({ hidden: false, auto_off: false });
    await back('dualsense');
    await settle(page);
    check('auto-off: no link dropped → the reconnect banner as before', ((await banner()) || '').startsWith('再連一次 DualSense') && (await page.locator('#board .obanner.ao').count()) === 0, await banner());
    // Switch Pro in 鍵鼠: HOME.
    await setXbox({ hidden: true, auto_off: true });
    await page.click('#board [data-pad="ns_pro"]');
    await settle(page);
    if ((await modeOf()) !== 'kbm') { await page.click('#board .omode [data-mode="kbm"]'); await settle(page); }
    if ((await pop().count()) === 1) { await page.click('#board [data-act="vglater"]'); await settle(page); }
    await away();
    await settle(page);
    check('auto-off: Switch Pro in 鍵鼠 says HOME', (await banner()) === aoText('Switch Pro') && (await page.locator('#board .obanner [data-act="rcdismiss"]').count()) === 0, await banner());
    await setXbox({ hidden: true, auto_off: false });
    await back('dualsense');
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
  }
  // Failures keep the popover open with the reason.
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  await setXbox({ vigem: 'missing', hidhide: 'missing', hidden: null }, { vigem: 'missing', hidhide: 'installed' });
  await page.click('#board .omode [data-mode="xbox"]');
  await settle(page);
  {
    const i0 = (await installs()).length;
    await page.click('#board [data-act="vginstall"]');
    await page.waitForTimeout(350);
    const err = ((await page.locator('#board .vgpop .vgerr').textContent().catch(() => '')) || '').trim();
    check('ViGEmBus still missing after its installer: error line, HidHide not tried', (await pop().count()) === 1 &&
      err === 'ViGEmBus 還沒裝好，可以再試一次（狀態：missing）' && JSON.stringify((await installs()).slice(i0)) === '["install_vigem"]', err);
  }
  await setXbox({}, { vigem: 'installed' });
  await page.evaluate(() => { window.__mockFail.install_hidhide = '下載失敗（0x80072EE7）'; });
  {
    const i0 = (await installs()).length;
    await page.click('#board [data-act="vginstall"]');
    await page.waitForTimeout(350);
    const err = ((await page.locator('#board .vgpop .vgerr').textContent().catch(() => '')) || '').trim();
    check('a rejected install_hidhide shows its message, popover stays', (await pop().count()) === 1 && err === '安裝失敗：下載失敗（0x80072EE7）' &&
      JSON.stringify((await installs()).slice(i0)) === '["install_vigem","install_hidhide"]', err);
    check('after ViGEmBus went in: its row 已安裝, the button 安裝 HidHide', JSON.stringify(await popRows()) === '["ViGEmBus:已安裝","HidHide:未安裝"]' &&
      JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '改回鍵鼠', '安裝 HidHide']), JSON.stringify(await popBtns()));
    await page.screenshot({ path: path.join(outDir, 'web-xbox-error.png') });
  }
  {
    const i0 = (await installs()).length;
    await page.click('#board [data-act="vginstall"]');
    await page.waitForTimeout(350);
    check('安裝 HidHide runs only install_hidhide and closes on installed', JSON.stringify((await installs()).slice(i0)) === '["install_hidhide"]' && (await pop().count()) === 0);
  }
  // 改回鍵鼠 from the popover; only HidHide missing (Switch Pro).
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  await page.click('#board [data-pad="ns_pro"]');
  await settle(page);
  await setXbox({ vigem: 'installed', hidhide: 'missing', hidden: null });
  await page.click('#board .omode [data-mode="xbox"]');
  await settle(page);
  {
    const t = ((await pop().textContent()) || '');
    check('only HidHide missing: ViGEmBus 已安裝, button 安裝 HidHide', (await pop().count()) === 1 && JSON.stringify(await popRows()) === '["ViGEmBus:已安裝","HidHide:未安裝"]' &&
      JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '改回鍵鼠', '安裝 HidHide']) && t.includes('把原本的 Switch Pro 對遊戲藏起來') && t.includes('官方安裝程式，約 1 分鐘') && !t.includes('各約'), t);
    await page.screenshot({ path: path.join(outDir, 'web-xbox-hidhide.png') });
  }
  await page.click('#board [data-act="vgkbm"]');
  await settle(page);
  const back = await lastCall(page, 'set_output_mode');
  check('改回鍵鼠 sends set_output_mode(ns_pro, kbm)', back?.args?.kind === 'ns_pro' && back.args.mode === 'kbm' && (await pop().count()) === 0 && (await modeOf()) === 'kbm', JSON.stringify(back?.args));
  // Switch 2 Pro: ViGEmBus only, and never the reconnect hint.
  await page.click('#board [data-pad="ns2_pro"]');
  await settle(page);
  await setXbox({ vigem: 'missing', hidhide: 'missing', hidden: null }, { vigem: 'installed', hidhide: 'missing' });
  await page.click('#board .omode [data-mode="xbox"]');
  await settle(page);
  {
    const t = ((await pop().textContent()) || '');
    check('NS2 popover: only the ViGEmBus row, button 安裝 ViGEmBus', (await pop().count()) === 1 && JSON.stringify(await popRows()) === '["ViGEmBus:未安裝"]' &&
      JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '改回鍵鼠', '安裝 ViGEmBus']) && t.includes('要讓遊戲把 Switch 2 Pro 當成 Xbox 手把，需要：') && !t.includes('HidHide'), t);
    const pt = await pointer();
    check('NS2 popover pointer on the Xbox segment too', Math.abs(pt.apex - pt.seg) <= 0.5 && pt.apex > pt.l && pt.apex < pt.r, JSON.stringify(pt));
    await page.screenshot({ path: path.join(outDir, 'web-vigem.png') });
    const i0 = (await installs()).length;
    await page.click('#board [data-act="vginstall"]');
    await page.waitForTimeout(350);
    check('NS2 安裝 ViGEmBus runs install_vigem only and closes', JSON.stringify((await installs()).slice(i0)) === '["install_vigem"]' && (await pop().count()) === 0 && (await bang().count()) === 0);
  }
  await setXbox({ hidden: false });
  await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
  await settle(page);
  check('NS2 never shows the reconnect banner', (await page.locator('#board .obanner.rc').count()) === 0 && (await banner()) === '遊戲看到的是 Xbox 手把；下面的鍵鼠綁定這時不作用', await banner());
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);
  await setXbox({ vigem: 'installed', hidhide: 'installed', hidden: true });
  await page.click('#board .omode [data-mode="xbox"]');
  await settle(page);
  check('installed → no popover', (await pop().count()) === 0 && (await bang().count()) === 0);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'ns2_pro', connected_pads: ['dualsense', 'ns2_pro'], warnings: ['vigem_missing'], off_pads: [] }); });
  await settle(page);
  check('live vigem_missing warning shows the !', (await bang().count()) === 1);
  await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }); });
  await page.click('#board [data-pad="ns2_pro"]');
  await page.click('#board .omode [data-mode="kbm"]');
  await settle(page);

  // ── v3.6.0: 鍵鼠 hides a native pad too (HidHide prompt once, !, reconnect banner) ──
  {
    const reloadWith = async (xbox, dismissed, connected = null) => {
      await page.evaluate(([x, d, c]) => sessionStorage.setItem('__keep', JSON.stringify({ xbox: x, dismissed: d, connected: c })), [xbox, dismissed, connected]);
      await page.reload({ waitUntil: 'load' });
      await page.locator('#board .hdr').waitFor({ timeout: 10000 });
      await page.waitForFunction(() => document.querySelectorAll('#board .slot').length > 20, null, { timeout: 10000 });
      await settle(page);
    };
    const MISS = { vigem: 'missing', hidhide: 'missing', hidden: null };
    await reloadWith(MISS, false);
    check('kbm-prompt-once: DualSense in 鍵鼠 → mode is 鍵鼠', (await modeOf()) === 'kbm');
    {
      const t = ((await pop().textContent().catch(() => '')) || '');
      check('kbm-prompt-once: popover opens by itself with the 鍵鼠 title', (await pop().count()) === 1 && t.includes('要讓遊戲看不到 DualSense、只收到鍵鼠，需要：') &&
        t.includes('官方安裝程式，約 1 分鐘，會要求系統管理員權限。沒裝也能用鍵鼠，只是遊戲會同時讀到手把。'), t);
      check('kbm-prompt-once: one row, HidHide only', JSON.stringify(await popRows()) === '["HidHide:未安裝"]', JSON.stringify(await popRows()));
      check('kbm-prompt-once: buttons 稍後 / 安裝 HidHide', JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '安裝 HidHide']), JSON.stringify(await popBtns()));
      check('kbm-hidhide-missing-still-sends: board not locked, no 不送東西 banner', (await page.locator('#board .obanner').count()) === 0 &&
        (await page.locator('#board [data-act="reset"]:disabled').count()) === 0);
    }
    {
      const pt = await page.evaluate(() => {
        const p = document.querySelector('#board .vgpop'), x = document.querySelector('#board .omode [data-mode="kbm"]');
        const pr = p.getBoundingClientRect(), xr = x.getBoundingClientRect(), b = document.getElementById('board').getBoundingClientRect();
        const cs = getComputedStyle(p), bs = getComputedStyle(p, '::before');
        const w = parseFloat(bs.width) + parseFloat(bs.borderLeftWidth) + parseFloat(bs.borderRightWidth);
        const apex = pr.right - parseFloat(cs.borderRightWidth) - parseFloat(bs.right) - w / 2;
        return { apex: apex - b.left, seg: (xr.left + xr.right) / 2 - b.left, l: xr.left - b.left, r: xr.right - b.left };
      });
      check('kbm-prompt-once: popover pointer aims at the 鍵鼠 segment', Math.abs(pt.apex - pt.seg) <= 0.5 && pt.apex > pt.l && pt.apex < pt.r, JSON.stringify(pt));
      await page.screenshot({ path: path.join(outDir, 'web-kbm-hidhide.png') });
    }
    await page.click('#board [data-act="vglater"]');
    await settle(page);
    {
      const sp = (await calls(page)).filter(c => c.cmd === 'set_ui_prefs').pop();
      check('kbm-prompt-once: 稍後 stores hidhide_prompt_dismissed=true', sp?.args?.prefs?.hidhide_prompt_dismissed === true, JSON.stringify(sp?.args));
      check('kbm-prompt-once: popover gone, ! stays', (await pop().count()) === 0 && (await bang().count()) === 1);
    }
    // No hider to install (dry run, not Windows): no prompt, no !, no reconnect banner in 鍵鼠.
    await reloadWith({ vigem: 'unsupported', hidhide: 'unsupported', hidden: null }, false);
    check('kbm-unsupported: no popover, no !', (await modeOf()) === 'kbm' && (await pop().count()) === 0 && (await bang().count()) === 0);
    await setXbox({ hidden: false });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await settle(page);
    check('kbm-unsupported: no reconnect banner either', (await page.locator('#board .obanner').count()) === 0 && (await pop().count()) === 0 && (await bang().count()) === 0);
    // The prompt waits for the pad: nothing connected → no popover; the pad connecting opens it once.
    const connectEv = () => page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await reloadWith(MISS, false, []);
    check('kbm-await-pad: nothing connected → no popover on load', (await modeOf()) === 'kbm' && (await pop().count()) === 0);
    await connectEv();
    await settle(page);
    check('kbm-await-pad: the pad connecting opens it', (await pop().count()) === 1 && JSON.stringify(await popRows()) === '["HidHide:未安裝"]');
    await page.click('#board [data-act="vglater"]');
    await settle(page);
    await page.evaluate(() => { window.__mock.status.connected = []; window.__mockEmit('controller-status', { connected: false, kind: null, connected_pads: [], warnings: [], off_pads: [] }); });
    await settle(page);
    await connectEv();
    await settle(page);
    check('kbm-await-pad: opened once this session, a reconnect does not reopen it', (await pop().count()) === 0);
    await reloadWith(MISS, true, []);
    await connectEv();
    await settle(page);
    check('kbm-await-pad: dismissed → no auto-open on connect either, the ! is there', (await pop().count()) === 0 && (await bang().count()) === 1);
    {
      await page.evaluate(() => { window.__mockCalls.length = 0; });
      await page.click('#board [data-pad="ns_pro"]');
      await settle(page);
      const sp = (await calls(page)).filter(c => c.cmd === 'set_ui_prefs').pop();
      check('tab switch sends only last_pad (no hidhide_prompt_dismissed)', !!sp && sp.args.prefs.last_pad === 'ns_pro' && !('hidhide_prompt_dismissed' in sp.args.prefs), JSON.stringify(sp?.args));
      await page.click('#board [data-pad="dualsense"]');
      await settle(page);
    }
    await reloadWith(MISS, true);
    check('kbm-dismissed: no popover, the ! is there', (await pop().count()) === 0 && (await bang().count()) === 1);
    await bang().click();
    await settle(page);
    check('kbm-dismissed: clicking the ! opens it', (await pop().count()) === 1 && JSON.stringify(await popBtns()) === JSON.stringify(['稍後', '安裝 HidHide']));
    await page.click('#board [data-act="vglater"]');
    await settle(page);
    await page.click('#board .omode [data-mode="off"]');
    await settle(page);
    check('kbm-manual-pick: 關 shows no popover', (await modeOf()) === 'off' && (await pop().count()) === 0);
    await page.click('#board .omode [data-mode="kbm"]');
    await settle(page);
    check('kbm-manual-pick: picking 鍵鼠 opens it although dismissed', (await modeOf()) === 'kbm' && (await pop().count()) === 1);
    await page.click('#board [data-act="vglater"]');
    await settle(page);
    // Reconnect banner: HidHide in, pad not yet hidden.
    await setXbox({ vigem: 'installed', hidhide: 'installed', hidden: false });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await settle(page);
    {
      const t = await banner();
      check('kbm-reconnect: DualSense banner with the ⏻', t === '再連一次 DualSense，遊戲才看不到原本的手把：按分頁上的  斷開，再按 PS 鍵連回來這次不再提醒' &&
        (await page.locator('#board .obanner.rc svg').count()) === 1 && (await bang().count()) === 0 && (await pop().count()) === 0, t);
      check('kbm-reconnect: banner sits at the stage centre and fits', await bannerCentred(), JSON.stringify(await bannerBox()));
    }
    // 這次不再提醒: per pad, front-end memory only, no IPC, back after a reload.
    {
      const btn = page.locator('#board .obanner.rc [data-act="rcdismiss"]');
      check('kbm-rcdismiss: the banner carries 這次不再提醒', (await btn.count()) === 1 && ((await btn.textContent()) || '').trim() === '這次不再提醒');
      const m = await page.evaluate(() => {
        const b = document.querySelector('#board .obanner.rc').getBoundingClientRect(), k = document.querySelector('#board .obanner.rc [data-act="rcdismiss"]').getBoundingClientRect();
        const sp = document.querySelector('#board .obanner.rc span').getBoundingClientRect();
        return { bt: b.top, bb: b.bottom, kt: k.top, kb: k.bottom, kl: k.left, kr: k.right, br: b.right, st: sp.top, sb: sp.bottom };
      });
      const gt = m.kt - m.bt, gb = m.bb - m.kb;
      check('kbm-rcdismiss: button vertically centred in the banner, whole pixels', Math.abs(gt - gb) <= 1 && Math.abs((m.kt + m.kb) / 2 - (m.st + m.sb) / 2) <= 1 &&
        [m.kt, m.kb, m.kl, m.kr].every(v => Math.abs(v - Math.round(v)) < 0.01) && m.kr <= m.br, JSON.stringify(m));
      await page.screenshot({ path: path.join(outDir, 'web-kbm-reconnect.png') });
      await page.evaluate(() => { window.__mockCalls.length = 0; });
      await btn.click();
      await settle(page);
      check('kbm-rcdismiss: click hides the banner, no IPC call', (await page.locator('#board .obanner.rc').count()) === 0 && (await calls(page)).length === 0, JSON.stringify(await calls(page)));
      await page.click('#board [data-pad="ns_pro"]');
      await page.evaluate(() => { window.__mock.xbox.hidden = false; window.__mockEmit('config-changed', { source: 'external' }); });
      await settle(page);
      check('kbm-rcdismiss: another pad still gets its banner', (await page.locator('#board .obanner.rc').count()) === 1 && ((await banner()) || '').includes('再按 HOME 鍵連回來'), await banner());
      await page.click('#board [data-pad="dualsense"]');
      await page.evaluate(() => { window.__mock.xbox.hidden = false; window.__mockEmit('config-changed', { source: 'external' }); });
      await settle(page);
      check('kbm-rcdismiss: back on DualSense it stays hidden', (await page.locator('#board .obanner.rc').count()) === 0);
      await reloadWith({ vigem: 'installed', hidhide: 'installed', hidden: false }, true);
      check('kbm-rcdismiss: a reload brings the DualSense banner back', (await page.locator('#board .obanner.rc [data-act="rcdismiss"]').count()) === 1, await banner());
    }
    await page.click('#board [data-pad="ns_pro"]');
    await settle(page);
    await page.evaluate(() => { window.__mock.xbox.hidden = false; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);
    check('kbm-reconnect: Switch Pro banner says HOME', ((await banner()) || '').endsWith('再按 HOME 鍵連回來這次不再提醒'), await banner());
    await setXbox({ hidden: true });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'ns_pro', connected_pads: ['ns_pro'], warnings: [], off_pads: [] }));
    await settle(page);
    check('kbm-reconnect: hidden:true clears it', (await page.locator('#board .obanner.rc').count()) === 0);
    // Switch 2 Pro: Windows cannot read it, nothing to hide.
    await setXbox(MISS);
    await page.click('#board [data-pad="ns2_pro"]');
    await settle(page);
    await page.evaluate(() => { window.__mock.config.output_mode.ns2_pro = 'kbm'; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);
    check('ns2-kbm: no popover, no !', (await modeOf()) === 'kbm' && (await pop().count()) === 0 && (await bang().count()) === 0);
    await page.click('#board .omode [data-mode="off"]');
    await settle(page);
    await page.click('#board [data-pad="ns2_pro"]');   // the mock's status event moves the tab to the owner pad
    await settle(page);
    await page.click('#board .omode [data-mode="kbm"]');
    await settle(page);
    check('ns2-kbm: picking 鍵鼠 again still shows neither', (await pop().count()) === 0 && (await bang().count()) === 0, `${await modeOf()} pop=${await pop().count()} bang=${await bang().count()} `);
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
    await page.evaluate(() => sessionStorage.removeItem('__keep'));
    // Leave the drivers installed: the ! moves the header controls (later layout checks assume none).
    await setXbox({ vigem: 'installed', hidhide: 'installed', hidden: true });
    await page.evaluate(() => window.__mockEmit('config-changed', { source: 'external' }));
    await settle(page);
    check('drivers installed again → no ! in 鍵鼠', (await bang().count()) === 0 && (await pop().count()) === 0);
  }

  // ── v3.6.0 header: fits with every pad connected + the !, power key on the selected tab only (four pads, v3.7.0) ──
  {
    await setXbox({ vigem: 'missing', hidhide: 'missing', hidden: null });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano'], warnings: [], off_pads: [] }));
    for (const pad of ['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano']) {
      await page.click(`#board [data-pad="${pad}"]`);
      await settle(page);
      const want = pad === 'ns2_pro' ? 'xbox' : 'kbm';
      if ((await modeOf()) !== want) { await page.click(`#board .omode [data-mode="${want}"]`); await settle(page); }
      if ((await pop().count()) === 1) { await page.click('#board [data-act="vglater"]'); await settle(page); }
      const m = await page.evaluate(() => {
        const R = e => e.getBoundingClientRect(), win = R(document.getElementById('win')), hdr = document.querySelector('#board .hdr');
        const o = {};
        o.fit = hdr.scrollWidth <= hdr.clientWidth;
        o.bang = document.querySelectorAll('#board .hdr .vgbang').length;
        o.gearW = R(hdr.querySelector('.gear')).width;
        o.gearRight = R(hdr.querySelector('.gear')).right - win.left;
        o.pwr = [...document.querySelectorAll('#board .pdw')].map(w => w.classList.contains('act') + ':' + w.querySelectorAll('.pwr').length);
        o.tabs = [...document.querySelectorAll('#board .pdw .pd')].map(pd => {
          const r = R(pd), ic = R(pd.querySelector('.live')), nm = R(pd.querySelector('span:not(.live)'));
          return { act: pd.classList.contains('act'), left: r.left - win.left, iconL: ic.left - r.left, gap: nm.left - ic.right, right: r.right - nm.right };
        });
        o.ints = [...document.querySelectorAll('#board .pd, #board .omode button, #board .gear, #board .live')].filter(e => !Number.isInteger(R(e).left) || !Number.isInteger(R(e).top)).map(e => e.className || e.dataset.mode);
        o.offW = R(document.querySelector('#board .omode .o-off')).width;
        o.kbmW = R(document.querySelector('#board .omode .o-kbm')).width;
        return o;
      });
      const J = JSON.stringify(m);
      const sel = m.tabs.find(t => t.act);
      check(`header[${pad}]: fits 980 px with four pads and the !`, m.fit && m.bang === 1, J);
      check(`header[${pad}]: gear 30 wide, right edge at 975`, m.gearW === 30 && m.gearRight === 975, J);
      check(`header[${pad}]: first tab at x=19`, m.tabs[0].left === 19, J);
      check(`header[${pad}]: icon 7 from the tab edge, name 6 after it`, m.tabs.every(t => t.iconL === 7 && t.gap === 6), J);
      check(`header[${pad}]: unselected tabs 7 px right of the name`, m.tabs.filter(t => !t.act).every(t => t.right === 7), J);
      // The Nano has one too, greyed out and disabled (Bluetooth LE: the app cannot turn it off).
      check(`header[${pad}]: power key only on the selected tab (greyed out on the Nano)`, JSON.stringify(m.pwr) === JSON.stringify(['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano'].map(q => (q === pad) + ':' + (q === pad ? 1 : 0))), J);
      check(`header[${pad}]: tabs, segments, gear, lamps on whole pixels; 關 39 / 鍵鼠 59 wide`, m.ints.length === 0 && m.offW === 39 && m.kbmW === 59 && !!sel, J);
      if (pad === 'xbox_nano') {
        // The Nano on 手把: the joined Xbox／手把 option lit, the header still fits and the gear stays put.
        await page.click('#board .omode .o-pad');
        await settle(page);
        const n = await page.evaluate(() => {
          const R = e => e.getBoundingClientRect(), win = R(document.getElementById('win')), hdr = document.querySelector('#board .hdr'), g = hdr.querySelector('.gear');
          return { fit: hdr.scrollWidth <= hdr.clientWidth, on: !!hdr.querySelector('.omode .xjoin.on'), gearW: R(g).width, gearRight: R(g).right - win.left,
            ints: [...document.querySelectorAll('#board .pd, #board .omode button, #board .omode .xjoin, #board .gear')].filter(e => !Number.isInteger(R(e).left) || !Number.isInteger(R(e).top)).length };
        });
        check('header[xbox_nano 手把]: .xjoin.on, fits 980 px, gear 30 wide at 975, whole pixels', n.on && n.fit && n.gearW === 30 && n.gearRight === 975 && n.ints === 0, JSON.stringify(n));
        await page.click('#board .omode .o-kbm');
        await settle(page);
      }
    }
    await setXbox({ vigem: 'installed', hidhide: 'installed', hidden: true });
    await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }); window.__mockEmit('config-changed', { source: 'external' }); });
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
  }

  // ── v3.8.0: the pad battery. controller-status.battery (readable connected pads only) puts a 13x7 pixel
  // battery in place of the lamp, a breathing bolt over it while charging, and a grey plate on hover. ──
  {
    const ALL = ['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano'];
    const BATT = { dualsense: { pct: 80, charging: false }, ns_pro: { pct: 75, charging: false }, xbox_nano: { pct: 60, charging: true } };
    const status = (pads, battery) => page.evaluate(([p, b]) => window.__mockEmit('controller-status',
      { connected: true, kind: p[0] || null, connected_pads: p, warnings: [], off_pads: [], battery: b }), [pads, battery]);
    const icons = () => page.evaluate(() => [...document.querySelectorAll('#board .pdw')].map(w =>
      (w.querySelector('.tbatt') ? 'batt' : '') + (w.querySelector('.live') ? (w.querySelector('.live.idle') ? 'idle' : 'live') : '') + (w.querySelector('.btip') ? '+tip' : '')));
    const bars = () => page.evaluate(() => [...document.querySelectorAll('#board .pdw')].map(w => {
      const s = w.querySelector('.tbatt svg');
      if (!s) return null;
      const n = c => s.innerHTML.split('fill="' + c + '"').length - 1;
      return { g: n('#7ee05a'), y: n('#ffd23f') - (s.querySelector('.bolt') ? 1 : 0), r: n('#ff5a4a'), bolt: s.querySelectorAll('.bolt').length };
    }));
    await status(ALL, BATT);
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
    const ic = await icons();
    check('battery: a tab with a battery shows it instead of the lamp, plus its plate; Switch 2 Pro keeps the green lamp',
      JSON.stringify(ic) === JSON.stringify(['batt+tip', 'batt+tip', 'live', 'batt+tip']), JSON.stringify(ic));
    const bs = await bars();
    check('battery: bars by level and colour (80 → 3 green, 75 → 3 green, 60 → 2 yellow), the bolt on the charging Nano only',
      JSON.stringify(bs) === JSON.stringify([{ g: 3, y: 0, r: 0, bolt: 0 }, { g: 3, y: 0, r: 0, bolt: 0 }, null, { g: 0, y: 2, r: 0, bolt: 1 }]), JSON.stringify(bs));
    for (const [pct, want] of [[50, { g: 0, y: 2, r: 0, bolt: 0 }], [20, { g: 0, y: 0, r: 1, bolt: 0 }], [5, { g: 0, y: 0, r: 0, bolt: 0 }]]) {
      await status(ALL, { ...BATT, dualsense: { pct, charging: false } });
      await settle(page);
      const b = (await bars())[0];
      check(`battery: ${pct}% → ${want.g + want.y + want.r} bar(s), ${want.y ? 'yellow' : want.r ? 'red' : 'none'}`, JSON.stringify(b) === JSON.stringify(want), JSON.stringify(b));
    }
    await status(ALL, BATT);
    await settle(page);
    const pos = await page.evaluate(() => {
      const R = e => e.getBoundingClientRect(), win = R(document.getElementById('win'));
      return [...document.querySelectorAll('#board .pdw')].filter(w => w.querySelector('.tbatt')).map(w => {
        const b = R(w.querySelector('.tbatt')), s = R(w.querySelector('.tbatt svg'));
        return { act: w.classList.contains('act'), bx: b.left - win.left, by: b.top - win.top, bw: b.width, bh: b.height, sx: s.left - win.left, sy: s.top - win.top, sw: s.width, sh: s.height };
      });
    });
    console.log(`  battery positions: ${JSON.stringify(pos)}`);
    check('battery: the 8 px box and the 13x9 svg on whole pixels (selected and unselected tabs)',
      pos.length === 3 && pos.every(p => [p.bx, p.by, p.sx, p.sy].every(Number.isInteger) && p.bw === 13 && p.bh === 8 && p.sw === 13 && p.sh === 9 && p.sx === p.bx && p.sy === p.by), JSON.stringify(pos));
    // The plate: hidden until the battery is hovered, then below the tab, inside the window, on whole pixels.
    const plate = pad => page.evaluate(p => {
      const R = e => e.getBoundingClientRect(), win = R(document.getElementById('win')), t = document.querySelector(`#board .pdw:has([data-pad="${p}"]) .btip`);
      if (!t) return null;
      const r = R(t), s = R(t.querySelector('svg')), x = R(t.querySelector('span')), tab = R(t.parentElement), cs = getComputedStyle(t);
      return { shown: cs.display !== 'none', text: t.textContent, left: r.left - win.left, top: r.top - win.top, right: r.right - win.left, bottom: r.bottom - win.top, w: r.width, h: r.height,
        winW: win.width, winH: win.height, tabBottom: tab.bottom - win.top, bg: cs.backgroundColor, clip: cs.clipPath !== 'none', color: cs.color,
        svg: { x: s.left - r.left, y: s.top - r.top, w: s.width, h: s.height }, txt: { x: x.left - r.left, y: x.top - r.top, r: r.right - x.right, b: r.bottom - x.bottom } };
    }, pad);
    await page.mouse.move(500, 400);
    await settle(page);
    const before = await plate('dualsense');
    check('battery plate hidden until hovered', before && !before.shown, JSON.stringify(before));
    await page.hover('#board [data-pad="dualsense"] .tbatt');
    await settle(page);
    const tip = await plate('dualsense');
    const inBox = t => t.txt.x >= 0 && t.txt.y >= 0 && t.txt.r >= 0 && t.txt.b >= 0 && t.svg.x >= 0 && t.svg.y >= 0 && t.svg.y + t.svg.h <= t.h;
    console.log(`  battery plate (DualSense): ${JSON.stringify(tip)}`);
    check('hover DualSense battery → grey notched plate, white 80%, doubled battery, inside the window on whole pixels',
      tip && tip.shown && tip.text === '80%' && tip.bg === 'rgb(119, 133, 147)' && tip.clip && tip.color === 'rgb(255, 255, 255)' &&
      tip.svg.w === 26 && tip.svg.h === 18 && inBox(tip) && tip.top >= tip.tabBottom &&
      tip.left >= 0 && tip.top >= 0 && tip.right <= tip.winW && tip.bottom <= tip.winH && [tip.left, tip.top, tip.svg.x, tip.svg.y, tip.txt.y].every(Number.isInteger), JSON.stringify(tip));
    await page.screenshot({ path: path.join(outDir, 'web-battery-hover.png'), animations: 'disabled' });
    await page.hover('#board [data-pad="xbox_nano"] .tbatt');
    await settle(page);
    const tipN = await plate('xbox_nano');
    console.log(`  battery plate (Nano, charging): ${JSON.stringify(tipN)}`);
    check('hover the charging Nano → 60% 充電中, inside the window on whole pixels', tipN && tipN.shown && tipN.text === '60% 充電中' && !tipN.text.includes('約') &&
      inBox(tipN) && tipN.right <= tipN.winW && [tipN.left, tipN.top, tipN.txt.y].every(Number.isInteger), JSON.stringify(tipN));
    await page.screenshot({ path: path.join(outDir, 'web-battery-charging.png'), animations: 'disabled' });
    await page.hover('#board [data-pad="ns_pro"] .tbatt');
    await settle(page);
    const tipP = await plate('ns_pro');
    check('Switch Pro plate says 75% (no 約)', tipP && tipP.shown && tipP.text === '75%', JSON.stringify(tipP));
    await page.mouse.move(500, 400);
    await settle(page);
    // The bolt breathes (1.6 s, opacity 1 → .15 → 1); with reduced motion it stands still at full opacity.
    const anim = () => page.evaluate(() => {
      const b = document.querySelector('#board .pdw:has([data-pad="xbox_nano"]) .tbatt .bolt'), cs = b && getComputedStyle(b);
      return cs && { name: cs.animationName, dur: cs.animationDuration, ease: cs.animationTimingFunction, iter: cs.animationIterationCount, op: cs.opacity };
    });
    const a1 = await anim();
    check('charging bolt breathes: 1.6 s ease-in-out, infinite', !!a1 && a1.name !== 'none' && a1.dur === '1.6s' && a1.ease === 'ease-in-out' && a1.iter === 'infinite', JSON.stringify(a1));
    const kf = await page.evaluate(name => {
      for (const sh of document.styleSheets) for (const r of sh.cssRules) if (r.type === CSSRule.KEYFRAMES_RULE && r.name === name) return [...r.cssRules].map(k => k.keyText + ':' + k.style.opacity);
      return null;
    }, a1 ? a1.name : '');
    check('charging bolt keyframes: opacity 1 → 0.15 → 1', JSON.stringify(kf) === JSON.stringify(['0%, 100%:1', '50%:0.15']), JSON.stringify(kf));
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await settle(page);
    const a2 = await anim();
    check('reduced motion: the bolt does not move and stays at opacity 1', !!a2 && a2.name === 'none' && a2.op === '1', JSON.stringify(a2));
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    // Disconnect: a status without ns_pro → its tab is back to the (idle) lamp, no plate; a battery entry for a pad
    // that is not connected is ignored.
    await status(['dualsense', 'ns2_pro', 'xbox_nano'], BATT);
    await settle(page);
    const ic2 = await icons();
    check('Switch Pro disconnects → its tab is back to the lamp, no battery, no plate', JSON.stringify(ic2) === JSON.stringify(['batt+tip', 'idle', 'live', 'batt+tip']), JSON.stringify(ic2));
    await status(['dualsense', 'ns2_pro', 'xbox_nano'], {});
    await settle(page);
    const ic3 = await icons();
    check('a status with no battery → every connected tab shows the green lamp', JSON.stringify(ic3) === JSON.stringify(['live', 'idle', 'live', 'live']), JSON.stringify(ic3));
    // Header: all four connected, every one with a battery, the ! shown — still fits, gear 30 at 975, whole pixels.
    await setXbox({ vigem: 'missing', hidhide: 'missing', hidden: null });
    const FULL = { dualsense: { pct: 100, charging: true }, ns_pro: { pct: 25, charging: false }, ns2_pro: { pct: 50, charging: true }, xbox_nano: { pct: 10, charging: false } };
    await status(ALL, FULL);
    for (const pad of ALL) {
      await page.click(`#board [data-pad="${pad}"]`);
      await settle(page);
      const want = pad === 'ns2_pro' ? 'xbox' : 'kbm';
      if ((await modeOf()) !== want) { await page.click(`#board .omode [data-mode="${want}"]`); await settle(page); }
      if ((await pop().count()) === 1) { await page.click('#board [data-act="vglater"]'); await settle(page); }
      const m = await page.evaluate(() => {
        const R = e => e.getBoundingClientRect(), win = R(document.getElementById('win')), hdr = document.querySelector('#board .hdr');
        return {
          fit: hdr.scrollWidth <= hdr.clientWidth, bang: document.querySelectorAll('#board .hdr .vgbang').length,
          gearW: R(hdr.querySelector('.gear')).width, gearRight: R(hdr.querySelector('.gear')).right - win.left,
          tabs: [...document.querySelectorAll('#board .pdw .pd')].map(pd => {
            const r = R(pd), ic = R(pd.querySelector('.tbatt')), nm = R(pd.querySelector(':scope > span:last-child'));
            return { act: pd.classList.contains('act'), left: r.left - win.left, w: r.width, iconL: ic.left - r.left, gap: nm.left - ic.right, right: r.right - nm.right };
          }),
          free: R(hdr.querySelector('.sp')).width,
          ints: [...document.querySelectorAll('#board .pd, #board .omode button, #board .gear, #board .tbatt, #board .tbatt svg')].filter(e => !Number.isInteger(R(e).left) || !Number.isInteger(R(e).top)).map(e => e.className.baseVal ?? e.className),
        };
      });
      const J = JSON.stringify(m);
      console.log(`  battery header[${pad}]: ${J}`);
      check(`battery header[${pad}]: four batteries and the ! fit 980 px, gear 30 wide at 975, first tab at 19`, m.fit && m.bang === 1 && m.gearW === 30 && m.gearRight === 975 && m.tabs[0].left === 19, J);
      check(`battery header[${pad}]: battery 7 from the tab edge, name 6 after it; unselected tabs 7 right of the name`,
        m.tabs.every(t => t.iconL === 7 && t.gap === 6) && m.tabs.filter(t => !t.act).every(t => t.right === 7), J);
      check(`battery header[${pad}]: tabs, segments, gear, batteries on whole pixels`, m.ints.length === 0, J);
      if (pad === 'dualsense') await page.screenshot({ path: path.join(outDir, 'web-battery-four.png'), animations: 'disabled' });
    }
    await setXbox({ vigem: 'installed', hidhide: 'installed', hidden: true });
    await page.evaluate(() => { window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }); window.__mockEmit('config-changed', { source: 'external' }); });
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
  }

  // ── v3.7.0: Xbox Nano tab. The Nano is an Xbox pad already: Xbox and 手把 are one option (stored as
  // `pad`), the pink spans both cells; LT／RT, LB／RB and View／Xbox／Menu／Share name its keys. ──
  {
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'xbox_nano', connected_pads: ['dualsense', 'xbox_nano'], warnings: [], off_pads: [] }));
    await page.click('#board [data-pad="xbox_nano"]');
    await settle(page);
    const segsN = () => page.locator('#board .omode button').evaluateAll(els => els.map(e => ({
      cls: e.className, mode: e.dataset.mode, text: e.textContent.trim(), svg: e.querySelectorAll('svg').length, on: e.getAttribute('aria-pressed') })));
    const joined = () => page.evaluate(() => { const j = document.querySelector('#board .omode .xjoin'); return j && { on: j.classList.contains('on'), w: j.getBoundingClientRect().width, n: j.querySelectorAll('button').length }; });
    // ⏻ on the Nano is shown but greyed out: the app cannot turn a Bluetooth LE pad off (it would only reconnect).
    check('Nano tab: connected and selected, one power key, disabled, exact title',
      (await page.locator('#board .hdr .pwr').count()) === 1 &&
      (await page.locator('#board .pdw.act [data-pad="xbox_nano"]').count()) === 1 &&
      await page.locator('#board .hdr .pwr').isDisabled() &&
      (await page.locator('#board .hdr .pwr').getAttribute('title')) === 'Xbox Nano 無法從電腦關機：請長按 Xbox 鍵約 6 秒');
    const dcBefore = await countCalls(page, 'disconnect_gamepad');
    await page.locator('#board .hdr .pwr').click({ force: true });
    check('Nano ⏻ click sends no disconnect_gamepad', (await countCalls(page, 'disconnect_gamepad')) === dcBefore);
    const sg = await segsN();
    check('Nano: four segments 關／鍵鼠／Xbox／手把, each with its icon',
      JSON.stringify(sg.map(x => x.text)) === '["關","鍵鼠","Xbox","手把"]' && sg.every(x => x.svg === 1), JSON.stringify(sg));
    check('Nano: Xbox and 手把 are both data-mode="pad" inside one .xjoin',
      JSON.stringify(sg.map(x => x.mode)) === '["off","kbm","pad","pad"]' && (await joined())?.n === 2 &&
      (await page.locator('#board .omode .xjoin .o-xbox + .o-pad').count()) === 1, JSON.stringify(sg));
    check('Nano 鍵鼠: only 鍵鼠 pressed, .xjoin not on',
      JSON.stringify(sg.map(x => x.on)) === '["false","true","false","false"]' && (await joined())?.on === false, JSON.stringify(sg));
    for (const which of ['o-xbox', 'o-pad']) {
      await page.click(`#board .omode .${which}`);
      await settle(page);
      const om = await lastCall(page, 'set_output_mode'), j = await joined(), s2 = await segsN();
      check(`Nano ${which} → set_output_mode(xbox_nano, pad)`, om?.args?.kind === 'xbox_nano' && om.args.mode === 'pad', JSON.stringify(om?.args));
      check(`Nano ${which}: .xjoin.on 124 wide, Xbox and 手把 both pressed`, !!j && j.on && j.w === 124 &&
        JSON.stringify(s2.map(x => x.on)) === '["false","false","true","true"]', JSON.stringify({ j, s2 }));
      check(`Nano ${which}: banner says the game reads it as an Xbox pad`, (await banner()) === '遊戲直接把它當 Xbox 手把用；下面的鍵鼠綁定這時不作用', await banner());
      check(`Nano ${which}: no ViGEmBus prompt or !`, (await pop().count()) === 0 && (await bang().count()) === 0);
      if (which === 'o-xbox') await page.screenshot({ path: path.join(outDir, 'web-nano-pad.png') });
      await page.click('#board .omode .o-kbm');
      await settle(page);
      const mk = await lastCall(page, 'set_output_mode');
      check(`Nano back to 鍵鼠 → set_output_mode(xbox_nano, kbm), only 鍵鼠 pressed`, mk?.args?.kind === 'xbox_nano' && mk.args.mode === 'kbm' &&
        JSON.stringify((await segsN()).map(x => x.on)) === '["false","true","false","false"]' && (await joined())?.on === false);
    }
    // A config that says xbox for the Nano (hand-edited) reads as the joined option, without a ViGEmBus prompt.
    await setXbox({ vigem: 'missing' });
    await page.evaluate(() => { window.__mock.config.output_mode.xbox_nano = 'xbox'; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);
    check('Nano output_mode xbox (hand-edited) shows the joined option, no ViGEmBus prompt',
      (await joined())?.on === true && (await pop().count()) === 0 && (await bang().count()) === 0 && (await banner()) === '遊戲直接把它當 Xbox 手把用；下面的鍵鼠綁定這時不作用', await banner());
    await setXbox({ vigem: 'installed' });
    await page.evaluate(() => { window.__mock.config.output_mode.xbox_nano = 'kbm'; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);
    // Its key names: trigger plates, shoulder slots, the three middle keys and Share below the pad.
    const qn = await page.locator('#board .qcap .qname').allTextContents();
    check('Nano trigger plates read LT／RT', JSON.stringify(qn) === '["LT","RT"]', JSON.stringify(qn));
    const tagOf = key => page.locator(`#board .slot[data-slot="${key}"] .tag`).evaluate(e => e.textContent.trim()).catch(() => null);
    const tags = {};
    for (const k of ['9', '10', '4', '5', '6', '29', '3', '2', '1', '0']) tags[k] = await tagOf(k);
    check('Nano shoulder slots read LB／RB', tags['9'] === 'LB' && tags['10'] === 'RB', JSON.stringify(tags));
    check('Nano middle slots read View／Xbox／Menu, Share below the pad', tags['4'] === 'View' && tags['5'] === 'Xbox' && tags['6'] === 'Menu' && tags['29'] === 'Share' &&
      await page.evaluate(() => { const y = k => document.querySelector(`#board .slot[data-slot="${k}"]`).getBoundingClientRect().top, pad = document.querySelector('#board svg.pad').getBoundingClientRect(); return y('29') > pad.top + 150 && [4, 5, 6].every(k => y(k) < pad.top); }), JSON.stringify(tags));
    check('Nano face slots read Y／X／B／A', tags['3'] === 'Y' && tags['2'] === 'X' && tags['1'] === 'B' && tags['0'] === 'A', JSON.stringify(tags));
    check('Nano has no touchpad / GL / GR / C slots', (await page.locator('#board .slot[data-slot="25"], #board .slot[data-slot="30"], #board .slot[data-slot="31"], #board .slot[data-slot="32"]').count()) === 0);
    // Share never reaches the computer (not in the XInput state, nor the HID report): its slot is greyed out and
    // cannot be bound — no click, double click, tile drop or swap sends anything.
    const share = slot(page, '29');
    check('Nano Share slot: disabled, exact title, not draggable',
      await share.isDisabled() && (await share.getAttribute('title')) === 'Xbox Nano 的 Share 鍵目前不會送給電腦' &&
      (await share.getAttribute('draggable')) === 'false', String(await share.getAttribute('title')));
    const shareCalls = async () => (await countCalls(page, 'set_binding')) + (await countCalls(page, 'swap_bindings'));
    const sb0 = await shareCalls(), cap0 = await countCalls(page, 'set_capture_active');
    await share.click({ force: true });
    await settle(page);
    check('Nano Share slot: a click never selects it for capture', await page.evaluate(() => !document.querySelector('#board .slot.sel')) &&
      (await countCalls(page, 'set_capture_active')) === cap0);
    await page.click('#board [data-tile="Q"]');
    await share.click({ force: true });
    await settle(page);
    await share.dblclick({ force: true });
    await settle(page);
    await page.evaluate(() => { const d = new DataTransfer(); const s = document.querySelector('#board .slot[data-slot="29"]');
      document.querySelector('#board [data-tile="W"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: d }));
      s.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: d }));
      s.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: d }));
      document.querySelector('#board [data-tile="W"]').dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: d })); });
    // A bound slot (the first one lit) dragged onto Share: no swap, and it is not cleared as dragged out.
    const boundKey = await page.evaluate(() => document.querySelector('#board .slot.on:not([data-slot="29"])')?.dataset.slot);
    const boundText = boundKey ? await slotText(page, boundKey) : '';
    await page.evaluate(k => { const d = new DataTransfer(); const s = document.querySelector('#board .slot[data-slot="29"]'), a = document.querySelector(`#board .slot[data-slot="${k}"]`);
      a.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: d }));
      s.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: d }));
      a.dispatchEvent(new DragEvent('dragend', { bubbles: true, dataTransfer: d })); }, boundKey);
    await settle(page);
    check('Nano Share slot: click, double click, tile drop and slot swap send no set_binding / swap_bindings', (await shareCalls()) === sb0,
      JSON.stringify((await calls(page)).slice(-4)));
    if (await page.locator('#board .tile.picked').count()) await page.click('#board .tile.picked');
    check('Nano: a bound slot dropped on Share keeps its own key', !!boundText && (await slotText(page, boundKey)) === boundText,
      boundKey + ' ' + boundText + ' → ' + (boundKey && await slotText(page, boundKey)));
    await settle(page);
    await page.click('#board .qcap [data-qs="L"][data-mode="single"]');
    await settle(page);
    check('Nano 單鍵 folds LT into one slot tagged LT', (await tagOf('23')) === 'LT', String(await tagOf('23')));
    await page.click('#board .qcap [data-qs="L"][data-mode="quick"]');
    await settle(page);
    // 鍵鼠 hides the Nano (HidHide): its reconnect hint says to press the Xbox key.
    await setXbox({ hidden: false });
    await page.evaluate(() => window.__mockEmit('config-changed', { source: 'external' }));
    await settle(page);
    const rc = ((await page.locator('#board .obanner.rc span').first().textContent().catch(() => '')) || '').trim();
    // Bluetooth LE: the app cannot turn the Nano off (no ⏻ in the hint): hold Xbox about 6 s, then press it.
    check('Nano 鍵鼠 reconnect hint: turn it off with the Xbox key, no ⏻', rc === '再連一次 Xbox Nano，遊戲才看不到原本的手把：長按 Xbox 鍵約 6 秒關機，再按 Xbox 鍵打開', rc);
    await setXbox({ hidden: false, auto_off: true });
    await page.evaluate(() => window.__mockEmit('controller-status', { connected: true, kind: 'dualsense', connected_pads: ['dualsense'], warnings: [], off_pads: [] }));
    await page.click('#board [data-pad="xbox_nano"]');
    await settle(page);
    const ao = ((await page.locator('#board .obanner.ao span').first().textContent().catch(() => '')) || '').trim();
    check('Nano powered off by the app: press the Xbox key to turn it on', ao === '已替你把 Xbox Nano 關機，好讓遊戲看不到它。按 Xbox 鍵打開就能用了', ao);
    await setXbox({ hidden: true, auto_off: false });
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
  }

  check('header fits 980 px (no overflow)', await page.evaluate(() => { const h = document.querySelector('#board .hdr'); return h.scrollWidth <= h.clientWidth; }));
  await page.click('#board [data-pad="dualsense"]');
  await settle(page);

  // ── v3.5.0: DualSense light bar (燈條 dropdown, 自訂 panel, K2 picker) ──────
  {
    const { hueCells, pickerIndex } = await import(pathToFileURL(path.join(webDir, 'board', 'lightcolor.js')).href);
    const canon = o => JSON.stringify(o, (k, v) => v && typeof v === 'object' && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).sort()) : v);
    const exp = JSON.parse(JSON.stringify(exampleConfig.lightbar));
    const sentLb = async () => (await lastCall(page, 'set_lightbar'))?.args?.lightbar;
    // Click `sel`, apply `edit` to the expected object, then the FULL object sent must equal it.
    async function act(name, how, edit) {
      const n = await countCalls(page, 'set_lightbar');
      if (typeof how === 'string') await page.click(how); else await how();
      await settle(page);
      edit(exp);
      const got = await sentLb();
      check(name, (await countCalls(page, 'set_lightbar')) === n + 1 && canon(got) === canon(exp), `${(await countCalls(page, 'set_lightbar')) - n} call(s) ${canon(got)}`);
    }
    const sec = (w, k, v) => `#board .lbpop [data-which="${w}"][data-k="${k}"][data-v="${v}"]`;
    const stepBtn = (w, k, d) => `#board .lbpop [data-act="lbstep"][data-which="${w}"][data-k="${k}"][data-d="${d}"]`;
    const dd = () => page.locator('#board .lbdd');
    check('DualSense board has the 燈條 field, list and panel closed', (await dd().count()) === 1 && (await page.locator('#board .lbdl, #board .lbpop').count()) === 0);
    await dd().click();
    await settle(page);
    check('click 燈條 → list and 自訂 panel appear', (await page.locator('#board .lbdl').count()) === 1 && (await page.locator('#board .lbpop').count()) === 1);
    await page.screenshot({ path: path.join(outDir, 'web-lightbar.png') });
    await act('關閉 → mode off', '#board .lbdl [data-v="off"]', e => { e.mode = 'off'; });
    check('mode 關閉: list only, no panel', (await page.locator('#board .lbdl').count()) === 1 && (await page.locator('#board .lbpop').count()) === 0);
    await act('原廠燈 → mode native', '#board .lbdl [data-v="native"]', e => { e.mode = 'native'; });
    check('mode 原廠燈: list only, no panel', (await page.locator('#board .lbpop').count()) === 0);
    await act('自訂 → mode custom', '#board .lbdl [data-v="custom"]', e => { e.mode = 'custom'; });
    check('mode 自訂: panel appears', (await page.locator('#board .lbpop').count()) === 1);

    await act('鍵鼠時 固定 → kbm.effect fixed', sec('kbm', 'effect', 'fixed'), e => { e.kbm.effect = 'fixed'; });
    await act('鍵鼠時 紅色格 → kbm.color #FF2A2A', `#board .lbpop .lbslot[data-which="kbm"][data-v="#FF2A2A"]`, e => { e.kbm.color = '#FF2A2A'; });
    await act('非鍵鼠時 呼吸 → other.effect breathe', sec('other', 'effect', 'breathe'), e => { e.other.effect = 'breathe'; });
    check('呼吸 row is labelled 呼吸速度', /呼吸速度/.test((await page.locator('#board .lbpop .lbsec').nth(1).textContent()) || ''));
    await act('呼吸速度 右鍵 → other.speed fast (from medium)', stepBtn('other', 'speed', 1), e => { e.other.speed = 'fast'; });
    await act('亮度 左鍵 → other.brightness 70', stepBtn('other', 'brightness', -1), e => { e.other.brightness = 70; });
    await act('非鍵鼠時 彩虹 → other.effect rainbow', sec('other', 'effect', 'rainbow'), e => { e.other.effect = 'rainbow'; });
    check('彩虹 (no 呼吸換色) row is labelled 換色速度', /換色速度/.test((await page.locator('#board .lbpop .lbsec').nth(1).textContent()) || ''));
    await act('呼吸換色 右鍵 → other.rainbow_breath true', stepBtn('other', 'rainbow_breath', 1), e => { e.other.rainbow_breath = true; });
    check('彩虹 with 呼吸換色 on: row relabelled 呼吸速度', /呼吸速度/.test((await page.locator('#board .lbpop .lbsec').nth(1).textContent()) || ''));

    // K2: opens from the 自訂 slot of a 固定 / 呼吸 state only.
    check('K2 closed before the 自訂 slot is clicked', (await page.locator('#board .lbcpop').count()) === 0);
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('自訂 slot opens K2', (await page.locator('#board .lbcpop').count()) === 1);
    const hue3 = hueCells(pickerIndex(exp.kbm.color).si)[3];
    await act('K2 色相第 3 格 → colour hueCells(si)[3]', '#board .lbcpop .lbhb >> nth=3', e => { e.kbm.color = hue3; });
    check('K2 stays open after picking', (await page.locator('#board .lbcpop').count()) === 1);
    // Drag along the strips: nothing is sent until release, then exactly one full set_lightbar.
    {
      const { hsvHex } = await import(pathToFileURL(path.join(webDir, 'board', 'lightcolor.js')).href);
      const cellBox = async (strip, k) => page.locator(`#board .lbcpop [data-strip="${strip}"] button >> nth=${k}`).boundingBox();
      const mid = async (strip, k) => { const b = await cellBox(strip, k); return { x: b.x + b.width / 2, y: b.y + b.height / 2 }; };
      const marked = async strip => page.evaluate(s => [...document.querySelectorAll(`#board .lbcpop [data-strip="${s}"] button`)].findIndex(b => b.getAttribute('aria-pressed') === 'true'), strip);
      const hexv = () => page.locator('#board .lbcpop .lbhexin').inputValue();
      const nCalls = () => countCalls(page, 'set_lightbar');
      for (const [strip, from, to] of [['hue', 2, 10], ['sat', 20, 5]]) {
        const { hi, si } = pickerIndex(exp.kbm.color);
        const want = k => strip === 'hue' ? hsvHex(k * 15, (si + 1) / 24, 1) : hsvHex(hi * 15, (k + 1) / 24, 1);
        const n = await nCalls();
        const p0 = await mid(strip, from), p1 = await mid(strip, to);
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.move((p0.x + p1.x) / 2, p0.y, { steps: 6 });
        await page.mouse.move(p1.x, p1.y, { steps: 6 });
        await settle(page);
        const mark = await marked(strip), field = await hexv();
        check(`${strip} drag ${from} → ${to}: no set_lightbar before release`, (await nCalls()) === n);
        check(`${strip} drag mid-drag: marker on cell ${to}, 色碼 shows the live colour`, mark === to && field === want(to), `marker ${mark}, field ${field}, want ${want(to)}`);
        const rgb = c => page.evaluate(c => { const d = document.createElement('i'); d.style.background = c; document.body.append(d); const v = getComputedStyle(d).backgroundColor; d.remove(); return v; }, c);
        const wantRgb = await rgb(want(to));
        const prev = await page.evaluate(() => getComputedStyle(document.querySelector('#board .lbcpop .lbprev')).backgroundColor);
        const slot = await page.evaluate(() => getComputedStyle(document.querySelector('#board .lbpop .lbadd[data-which="kbm"] i')).backgroundColor);
        check(`${strip} drag mid-drag: preview swatch and 自訂 slot show the live colour`, prev === wantRgb && slot === wantRgb, `${prev} / ${slot} want ${wantRgb}`);
        const padFill = await page.evaluate(() => { const r = document.querySelector('#board .lbarsvg .lbar [opacity]') || document.querySelector('#board .lbarsvg .lbar polygon, #board .lbarsvg .lbar rect'); return r ? getComputedStyle(r).fill : ''; });
        check(`${strip} drag mid-drag: pad-art strip follows`, padFill === wantRgb, `${padFill} want ${wantRgb}`);
        await page.mouse.up();
        await settle(page);
        exp.kbm.color = want(to);
        check(`${strip} drag release: exactly 1 full set_lightbar`, (await nCalls()) === n + 1 && canon(await sentLb()) === canon(exp), `${(await nCalls()) - n} call(s) ${canon(await sentLb())}`);
        check(`${strip} drag release: K2 and dropdown stay open`, (await page.locator('#board .lbcpop').count()) === 1 && (await page.locator('#board .lbpop').count()) === 1);
      }
      {
        // Past the right end clamps to cell 23; y is ignored.
        const n = await nCalls(), p0 = await mid('hue', 3), end = await cellBox('hue', 23);
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.move(end.x + end.width + 8, p0.y + 3, { steps: 8 });
        await page.mouse.up();
        await settle(page);
        const { si } = pickerIndex(exp.kbm.color);
        exp.kbm.color = hsvHex(23 * 15, (si + 1) / 24, 1);
        check('drag past the right end → cell 23', (await nCalls()) === n + 1 && canon(await sentLb()) === canon(exp), canon(await sentLb()));
      }
      {
        // Started on a strip, released outside the popover: nothing closes.
        const n = await nCalls(), p0 = await mid('sat', 4);
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.move(8, 8, { steps: 8 });
        await page.mouse.up();
        await settle(page);
        const { hi } = pickerIndex(exp.kbm.color);
        exp.kbm.color = hsvHex(hi * 15, 1 / 24 * 1, 1);
        check('drag released outside: dropdown and K2 stay open', (await page.locator('#board .lbcpop').count()) === 1 && (await page.locator('#board .lbpop').count()) === 1);
        check('drag released outside: 1 call, clamped to cell 0', (await nCalls()) === n + 1 && canon(await sentLb()) === canon(exp), canon(await sentLb()));
      }
      for (const [strip, k, side] of [['hue', 23, 'left'], ['hue', 0, 'right'], ['sat', 23, 'left'], ['sat', 0, 'right']]) {
        // Cell edges: a press 0.5 px inside the edge belongs to that cell (the strip's border is not part of the span).
        const b = await cellBox(strip, k);
        const x = side === 'left' ? b.x + 0.5 : b.x + b.width - 0.5, y = b.y + b.height / 2;
        await page.mouse.move(x, y);
        await page.mouse.down();
        await page.mouse.up();
        await settle(page);
        check(`${strip} cell ${k} ${side} edge ±0.5px → marker on ${k}`, (await marked(strip)) === k, `marker ${await marked(strip)}`);
      }
      {
        // Window blur mid-drag ends it like a release: one commit, drag cleared.
        const n = await nCalls(), { hi } = pickerIndex(exp.kbm.color);
        const k = hi === 12 ? 11 : 12, p0 = await mid('hue', 1), p1 = await mid('hue', k);
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.move(p1.x, p1.y, { steps: 5 });
        await settle(page);
        check('blur: nothing sent while dragging', (await nCalls()) === n);
        await page.evaluate(() => window.dispatchEvent(new Event('blur')));
        await settle(page);
        const { si } = pickerIndex(exp.kbm.color);
        exp.kbm.color = hsvHex(k * 15, (si + 1) / 24, 1);
        check('window blur mid-drag → exactly 1 set_lightbar, full object', (await nCalls()) === n + 1 && canon(await sentLb()) === canon(exp), `${(await nCalls()) - n} call(s)`);
        await page.mouse.move(p0.x, p0.y, { steps: 3 });
        await settle(page);
        check('blur: drag cleared (later moves change nothing)', (await nCalls()) === n + 1 && (await hexv()) === exp.kbm.color);
        await page.mouse.up();
        await settle(page);
        check('blur: release afterwards sends nothing more', (await nCalls()) === n + 1);
      }
      {
        // Press and release on one cell without moving: one call, no duplicate from the click.
        const n = await nCalls(), p0 = await mid('hue', 7);
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.up();
        await settle(page);
        const { si } = pickerIndex(exp.kbm.color);
        exp.kbm.color = hsvHex(7 * 15, (si + 1) / 24, 1);
        check('press + release on one cell → exactly 1 call', (await nCalls()) === n + 1 && canon(await sentLb()) === canon(exp), `${(await nCalls()) - n} call(s)`);
        // Same cell again: colour unchanged, nothing sent.
        await page.mouse.move(p0.x, p0.y);
        await page.mouse.down();
        await page.mouse.up();
        await settle(page);
        check('press + release on the already chosen cell → no call', (await nCalls()) === n + 1);
      }
    }
    const hex = () => page.locator('#board .lbcpop .lbhexin');
    await act('色碼 2ea8ff + Enter (change) → #2EA8FF', async () => { await hex().fill('2ea8ff'); await hex().press('Enter'); }, e => { e.kbm.color = '#2EA8FF'; });
    check('色碼 field shows the stored colour', (await hex().inputValue()) === '#2EA8FF');
    {
      const n = await countCalls(page, 'set_lightbar');
      await hex().fill('xyz');
      await hex().press('Enter');
      await settle(page);
      check('色碼 xyz → no set_lightbar, field back to #2EA8FF', (await countCalls(page, 'set_lightbar')) === n && (await hex().inputValue()) === '#2EA8FF');
    }
    await act('鍵鼠時 彩虹 → kbm.effect rainbow', sec('kbm', 'effect', 'rainbow'), e => { e.kbm.effect = 'rainbow'; });
    check('K2 gone when the state is 彩虹', (await page.locator('#board .lbcpop').count()) === 0);
    await act('鍵鼠時 呼吸 → kbm.effect breathe', sec('kbm', 'effect', 'breathe'), e => { e.kbm.effect = 'breathe'; });
    check('K2 stays closed after an effect change (opens only from the 自訂 slot)', (await page.locator('#board .lbcpop').count()) === 0);
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('自訂 slot opens K2 again for 呼吸', (await page.locator('#board .lbcpop').count()) === 1);
    await page.screenshot({ path: path.join(outDir, 'web-lightbar-k2.png') });

    // Closing: only a click on the field again or outside; Esc does nothing.
    const panelOpen = async () => (await page.locator('#board .lbpop').count()) === 1 || (await page.locator('#board .lbdl').count()) === 1;
    await page.keyboard.press('Escape');
    await settle(page);
    check('Esc does not close the dropdown', await panelOpen() && (await page.locator('#board .lbcpop').count()) === 1);
    // Layered dismissal (ruling R27): each outside click closes one layer and never swallows the click.
    const lbCalls = () => countCalls(page, 'set_lightbar');
    const k2 = () => page.locator('#board .lbcpop').count();
    const bothOpen = async () => (await page.locator('#board .lbpop').count()) === 1 && (await page.locator('#board .lbdl').count()) === 1;
    {
      const n = await lbCalls();
      await page.click('#board .lbcpop .lbxk');
      await settle(page);
      check('K2 close key → K2 gone, list and panel stay, 0 set_lightbar', (await k2()) === 0 && await bothOpen() && (await lbCalls()) === n);
    }
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('自訂 slot reopens K2 after the close key', (await k2()) === 1);
    await act('K2 open, click panel 鍵鼠時 紅色格 → K2 closes, control still acts', `#board .lbpop .lbslot[data-which="kbm"][data-v="#FF2A2A"]`, e => { e.kbm.color = '#FF2A2A'; });
    check('…and the panel stays open with K2 gone', (await k2()) === 0 && await bothOpen());
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('K2 open again', (await k2()) === 1);
    {
      const n = await lbCalls();
      await page.click('#board .hdr', { position: { x: 4, y: 4 } });
      await settle(page);
      check('click outside the menu with K2 open → K2 closes, dropdown stays, 0 calls', (await k2()) === 0 && await bothOpen() && (await lbCalls()) === n);
    }
    await page.click('#board .hdr', { position: { x: 4, y: 4 } });
    await settle(page);
    check('second click outside closes list and panel', !(await panelOpen()) && (await k2()) === 0);
    await dd().click();
    await settle(page);
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('自訂 slot opens K2 from a reopened dropdown', (await k2()) === 1);
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check('自訂 slot again closes K2 (kept), panel stays', (await k2()) === 0 && await bothOpen());
    await act('非鍵鼠時 固定 (so it has a 自訂 slot) → other.effect fixed', sec('other', 'effect', 'fixed'), e => { e.other.effect = 'fixed'; });
    await page.click('#board .lbpop .lbadd[data-which="other"]');
    await settle(page);
    check('K2 open on 非鍵鼠時 (top 265)', (await k2()) === 1 && (await page.locator('#board .lbcpop').evaluate(e => e.style.top)) === '265px');
    await page.click('#board .lbpop .lbadd[data-which="kbm"]');
    await settle(page);
    check("K2 open on other, 鍵鼠時's 自訂 slot → K2 switches to kbm (top 165)", (await k2()) === 1 && (await page.locator('#board .lbcpop').evaluate(e => e.style.top)) === '165px');
    await page.click('#board .hdr', { position: { x: 4, y: 4 } });
    await settle(page);
    await page.click('#board .hdr', { position: { x: 4, y: 4 } });
    await settle(page);
    check('both layers closed again', !(await panelOpen()));
    await dd().click();
    await settle(page);
    check('click the field opens it again', await panelOpen());
    await dd().click();
    await settle(page);
    check('click the field again closes it', !(await panelOpen()));
    await dd().click();
    await settle(page);
    await page.keyboard.press('Escape');
    await settle(page);
    check('Esc leaves a reopened dropdown open', await panelOpen());

    // The cap is not drawn over the macro editor, and comes back after it.
    // A scratch macro seeded in the mock only (removed again below), so the later macro checks see none.
    await page.evaluate(() => { window.__mock.config.macros['燈條測試'] = { loop: false, steps: [] }; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);
    await page.locator('#board [data-medit="燈條測試"]').click();
    await settle(page);
    check('macro editor open: no 燈條 cap, list, panel', (await page.locator('#board .med').count()) === 1 &&
      (await page.locator('#board .lbcap, #board .lbdl, #board .lbpop, #board .lbcpop').count()) === 0);
    await page.click('#board .med [data-act="mdone"]');
    await settle(page);
    check('macro editor closed: 燈條 cap is back, dropdown stayed closed', (await dd().count()) === 1 && !(await panelOpen()));
    await page.evaluate(() => { delete window.__mock.config.macros['燈條測試']; window.__mockEmit('config-changed', { source: 'external' }); });
    await settle(page);

    // Locked board (手把): the dropdown still opens and still saves.
    await page.click('#board .omode [data-mode="pad"]');
    await settle(page);
    check('手把 locks the board', await locked());
    await dd().click();
    await settle(page);
    check('locked board: dropdown still opens', await panelOpen());
    await act('locked board: 非鍵鼠時 固定 → other.effect fixed', sec('other', 'effect', 'fixed'), e => { e.other.effect = 'fixed'; });
    await act('locked board: 天藍 → other.color #2EA8FF', `#board .lbpop .lbslot[data-which="other"][data-v="#2EA8FF"]`, e => { e.other.color = '#2EA8FF'; });
    await dd().click();
    await settle(page);
    check('locked board: field closes it', !(await panelOpen()));
    await page.click('#board .omode [data-mode="kbm"]');
    await settle(page);

    // Not a DualSense: no cap.
    await page.click('#board [data-pad="ns_pro"]');
    await settle(page);
    check('Switch Pro board has no 燈條 cap', (await page.locator('#board .lbcap').count()) === 0 && (await page.locator('#board .lbdd').count()) === 0);
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
    check('back on DualSense the cap returns', (await dd().count()) === 1);
  }

  // ── The drawn window (v3.3.0): title bar, minimise / close, touchpad switch in the header ──
  check('title bar reads PAD TO KBM v + the app version', ((await page.locator('#wtbar .wt').textContent()) || '') === 'PAD TO KBM v' + appVersion,
    await page.locator('#wtbar .wt').textContent());
  {
    // Whole pixels, centred in the bar, clear of the minimise key; the OS title stays PadToKBM (index.html).
    const t = await page.evaluate(() => {
      const bar = document.getElementById('wtbar').getBoundingClientRect(), wt = document.querySelector('#wtbar .wt');
      const r = wt.getBoundingClientRect(), rg = document.createRange(); rg.selectNodeContents(wt); const tr = rg.getBoundingClientRect();
      const min = document.querySelector('#wtbar [data-win="min"]').getBoundingClientRect();
      return { ints: Number.isInteger(r.left) && Number.isInteger(r.top) && Number.isInteger(r.height), mid: (r.top + r.bottom) / 2 - (bar.top + bar.bottom) / 2,
        tmid: (tr.top + tr.bottom) / 2 - (bar.top + bar.bottom) / 2, clear: tr.right < min.left, title: document.title };
    });
    check('title text: whole pixels, centred in the bar, clear of the keys; document title PadToKBM', t.ints && Math.abs(t.mid) <= 0.5 && Math.abs(t.tmid) <= 1 && t.clear && t.title === 'PadToKBM', JSON.stringify(t));
  }
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
  // Reskin Q24 B: pixel icons; icon-only buttons are the title bar's blue key.
  check('header / footer / macro-box icons are crisp pixel art', await page.evaluate(() => {
    const svgs = [...document.querySelectorAll('#board .omode svg, #board .gear svg, #board .b-hist svg, #board .pen svg, #board .mpg svg, #board .pwr svg')];
    return svgs.length >= 8 && svgs.every(s => s.getAttribute('shape-rendering') === 'crispEdges' && !s.querySelector('[stroke], circle'));
  }));
  check('gear, pen and page keys are the blue title-bar key', await page.evaluate(() =>
    ['#board .hdr .gear', '#board .mpg button:not(.pdot)', '#board .mt .pen'].map(q => document.querySelector(q)).filter((e, i) => e || i < 2).every(e => {
      const c = e && getComputedStyle(e);
      return !!c && c.borderTopColor === 'rgb(0, 7, 20)' && c.color === 'rgb(196, 252, 255)' && /linear-gradient/.test(c.backgroundImage);
    })));

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
  check('capture bar text is exact', ((await page.locator('#board .capbar').textContent()) || '').trim() === '按鍵鼠上的鍵，或點下面的方塊（再點一次格子取消）');
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
  await slot(page, '0').click();
  await settle(page);
  check('clicking the waiting slot again cancels capture without binding', (await countCalls(page, 'set_binding')) === nBind &&
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
  await page.click('#board .med .mhd .mname');
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
  check('macro pen is the blue title-bar key (Q24 B)', await page.evaluate(() => {
    const c = getComputedStyle(document.querySelector('#board .mt .pen'));
    return c.borderTopColor === 'rgb(0, 7, 20)' && /linear-gradient/.test(c.backgroundImage);
  }));
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
  check('five tabs 一般/搖桿/觸控板/按鍵/記住的手把', JSON.stringify(tabs.map(t => t.trim())) === '["一般","搖桿","觸控板","按鍵","記住的手把"]', tabs.join('|'));
  {
    // The tab column holds all five on whole pixels; every label inside its button, vertically centred.
    const m = await page.evaluate(() => {
      const col = document.querySelector('#board .stabs'), out = [];
      for (const b of col.querySelectorAll('[data-stab]')) {
        const r = b.getBoundingClientRect(), rg = document.createRange();
        rg.selectNodeContents(b);
        const tr = rg.getBoundingClientRect();
        out.push({ t: b.textContent.trim(), ints: Number.isInteger(r.left) && Number.isInteger(r.top) && Number.isInteger(r.width) && Number.isInteger(r.height),
          inside: tr.left >= r.left + 2 && tr.right <= r.right - 2 && tr.top >= r.top && tr.bottom <= r.bottom,
          mid: Math.abs((tr.top + tr.bottom) / 2 - (r.top + r.bottom) / 2), bottom: r.bottom });
      }
      const cr = col.getBoundingClientRect();
      return { out, sh: col.scrollHeight, ch: col.clientHeight, sw: col.scrollWidth, cw: col.clientWidth, colBottom: cr.bottom };
    });
    check('settings tab column: five tabs fit (no overflow), whole pixels, labels inside and centred',
      m.out.length === 5 && m.sh === m.ch && m.sw === m.cw && m.out.every(o => o.ints && o.inside && o.mid <= 1 && o.bottom <= m.colBottom), JSON.stringify(m));
  }
  check('settings version line names PadToKBM 手把轉鍵鼠', /^PadToKBM 手把轉鍵鼠 v/.test(((await page.locator('#board .ver').textContent()) || '').trim()), await page.locator('#board .ver').textContent());
  {
    const about = (await page.locator('#board .ver').innerText()).split('\n').map(x => x.trim());
    check('settings about line names the four pads', about[1] === '支援 DualSense、Switch Pro、Switch 2 Pro、Xbox Nano', JSON.stringify(about));
  }
  check('一般 shows the config path and both open buttons', /padtokbm\.json/.test(await page.locator('#board .cfgp').textContent()) &&
    (await page.locator('#board [data-act="opencfg"]').count()) === 1 && (await page.locator('#board [data-act="opendir"]').count()) === 1);
  await page.click('#board [data-act="opencfg"]');
  await page.click('#board [data-act="opendir"]');
  await settle(page);
  check('settings close is the title-bar close key at 2x (Q24 B)', await page.evaluate(() => {
    const k = document.querySelector('#board .whd .x svg');
    return !!k && k.getAttribute('width') === '24' && k.getAttribute('viewBox') === '0 0 12 12' && !!k.querySelector('path[fill="#000714"]');
  }));
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
  check('Esc does not close settings (v3.4.0: no shortcuts)', (await page.locator('#board .setw').count()) === 1);
  if (await page.locator('#board .setw').count()) await page.click('#board [data-act="setclose"]');
  await settle(page);
  check('取消 closes without saving', (await page.locator('#board .setw').count()) === 0 && (await countCalls(page, 'set_settings')) === nS2);
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

  // ── v3.6.0: 設定 → 記住的手把 (remembered HidHide pads) ─────────────────────
  {
    const DS_USB = 'HID\\VID_054C&PID_0CE6&MI_03\\7&1a2b&0&0000';
    const NSP_BT = 'HID\\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002057e_PID&2009\\9&1&0&0000';
    const DS_BT = 'HID\\{00001124-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0ce6\\9&2d1c&0&0000';
    const HINT = '連線中的手把要先關掉才能刪除';
    // Bluetooth addresses (Task 10): the row shows the last two bytes, its title the whole address.
    const A_NSP = 'A0B1C2D3E4F5', A_DS = '7C66EF8223D2';
    // The list and the pads present now (the hider's last round); the tab re-reads them on config-changed.
    const setKnown = async (list, present = []) => {
      await page.evaluate(([l, p]) => {
        window.__mock.presentIds = p;
        if (l.length) window.__mock.config.hidhide_known = l; else delete window.__mock.config.hidhide_known;
        window.__mockEmit('config-changed', { source: 'watcher' });
      }, [list, present]);
      await settle(page);
    };
    // Rows, columns and text placement in the open pane.
    const measure = () => page.evaluate(() => {
      const pane = document.querySelector('#board .spane'), R = e => e.getBoundingClientRect();
      const txt = e => { const rg = document.createRange(); rg.selectNodeContents(e); return rg.getBoundingClientRect(); };
      const ints = r => Number.isInteger(r.left) && Number.isInteger(r.top) && Number.isInteger(r.width) && Number.isInteger(r.height);
      const pr = R(pane), bad = [];
      const rows = [...pane.querySelectorAll('.knr')].map(r => {
        const rr = R(r), b = r.querySelector('[data-act="forgetpad"]'), br = R(b), bt = txt(b), tag = r.querySelector('.knc');
        for (const s of r.querySelectorAll('.knn, .knb, .kns')) {
          const sr = txt(s);
          if (sr.left < rr.left || sr.right > (tag ? R(tag).left : br.left) || Math.abs((sr.top + sr.bottom) / 2 - (rr.top + rr.bottom) / 2) > 1) bad.push('span ' + s.textContent);
        }
        if (tag) {
          const tr = R(tag), tt = txt(tag);
          if (!ints(tr) || tr.right > br.left || tt.left < tr.left + 1 || tt.right > tr.right - 1 || Math.abs((tt.top + tt.bottom) / 2 - (tr.top + tr.bottom) / 2) > 1 ||
            Math.abs((tr.top + tr.bottom) / 2 - (rr.top + rr.bottom) / 2) > 1) bad.push('tag ' + b.dataset.id);
        }
        if (bt.left < br.left + 2 || bt.right > br.right - 2 || Math.abs((bt.top + bt.bottom) / 2 - (br.top + br.bottom) / 2) > 1) bad.push('button ' + b.dataset.id);
        if (br.right > rr.right || !Number.isInteger(rr.left) || !Number.isInteger(rr.top) || !ints(br)) bad.push('px ' + b.dataset.id);
        if (rr.left < pr.left || rr.right > pr.right || rr.bottom > pr.bottom) bad.push('out ' + b.dataset.id);
        const sn = r.querySelector('.kns');
        return { x: rr.left, y: rr.top, w: rr.width, text: r.textContent, title: r.getAttribute('title'), id: b.dataset.id, btn: b.textContent.trim(), serial: sn ? sn.textContent : null,
          used: Math.ceil(R(b).right - R(r.querySelector('.knn')).left),
          // Spare px in the row: the gap between the last text span and the tag / button, less the flex gap.
          free: Math.floor((tag ? R(tag).left : br.left) - R(sn || r.querySelector('.knb')).right - parseFloat(getComputedStyle(r).columnGap)),
          tag: tag ? tag.textContent.trim() : null, dis: b.disabled, btitle: b.getAttribute('title') };
      });
      const all = pane.querySelector('[data-act="forgetall"]');
      if (all) { const ar = R(all), at = txt(all); if (at.left < ar.left + 2 || at.right > ar.right - 2 || Math.abs((at.top + at.bottom) / 2 - (ar.top + ar.bottom) / 2) > 1 || !ints(ar) || ar.bottom > pr.bottom) bad.push('forgetall'); }
      const d = pane.querySelector('.knd');
      return { rows, bad, all: all ? all.textContent.trim() : null, allDis: all ? all.disabled : null, sh: pane.scrollHeight, ch: pane.clientHeight, sw: pane.scrollWidth, cw: pane.clientWidth,
        title: pane.querySelector('.knbox .cfgl')?.textContent.trim(), desc: d ? d.innerText.split('\n').map(x => x.trim()) : null, empty: pane.querySelector('.kne')?.textContent.trim() ?? null };
    });
    await setKnown([{ kind: 'dualsense', id: DS_USB }, { kind: 'ns_pro', id: NSP_BT, addr: A_NSP }], [DS_USB]);
    await page.click('#board [data-act="setopen"]');
    await settle(page);
    await page.click('#board [data-stab="known"]');
    await settle(page);
    let m = await measure();
    const J = () => JSON.stringify(m);
    check('記住的手把: title and the two description lines verbatim', m.title === '記住的手把' &&
      JSON.stringify(m.desc) === JSON.stringify(['連過的手把會被記住，開程式時就先藏起來，之後再打開手把不用重連。', '刪除會立刻生效；連線中的手把要先關掉才能刪除。']), J());
    check('記住的手把: one row per pad, name + 藍牙／USB + 刪除, row title = instance id', m.rows.length === 2 &&
      /^DualSense\s*USB/.test(m.rows[0].text) && m.rows[0].id === DS_USB && m.rows[0].title === DS_USB && m.rows[0].btn === '刪除' &&
      /^Switch Pro\s*藍牙/.test(m.rows[1].text) && m.rows[1].id === NSP_BT, J());
    check('記住的手把: a Bluetooth row shows its address\'s last two bytes; its title starts with the whole address', m.rows[1].serial === 'E4:F5' &&
      /^Switch Pro\s*藍牙\s*E4:F5\s*刪除$/.test(m.rows[1].text) && m.rows[1].title === '藍牙位址 A0:B1:C2:D3:E4:F5\n' + NSP_BT, J());
    check('記住的手把: a USB row has no serial', m.rows[0].serial === null && !/:/.test(m.rows[0].text), J());
    check('記住的手把: a present pad shows 連線中 and its 刪除 is disabled with the hint', m.rows[0].tag === '連線中' && m.rows[0].dis === true && m.rows[0].btitle === HINT &&
      m.rows[1].tag === null && m.rows[1].dis === false && m.rows[1].btitle === null, J());
    check('記住的手把: two rows fill the left column first', m.rows[0].x === m.rows[1].x && m.rows[1].y > m.rows[0].y, J());
    check('記住的手把: 全部刪除 shows with two or more, enabled while one is absent', m.all === '全部刪除' && m.allDis === false, J());
    check('記住的手把 (2): whole pixels, text inside its box and centred, pane does not scroll', m.bad.length === 0 && m.sh === m.ch && m.sw === m.cw, J());
    check('記住的手把 asks for the present pads', (await countCalls(page, 'get_present_pad_ids')) > 0);
    await page.screenshot({ path: path.join(outDir, 'web-known-2.png') });
    await page.click('#board [data-act="forgetpad"][data-id="' + NSP_BT.replace(/\\/g, '\\\\') + '"]');
    await settle(page);
    m = await measure();
    check('刪除 sends forget_hidden_pad with the id; the list redraws', (await lastCall(page, 'forget_hidden_pad'))?.args?.id === NSP_BT &&
      m.rows.length === 1 && m.rows[0].id === DS_USB && m.all === null, J());
    await setKnown([{ kind: 'dualsense', id: DS_USB }, { kind: 'dualsense', id: DS_BT }, { kind: 'ns_pro', id: NSP_BT }], [DS_USB]);
    const nAll = await countCalls(page, 'forget_all_hidden_pads');
    await page.click('#board [data-act="forgetall"]');
    await settle(page);
    m = await measure();
    check('全部刪除 sends forget_all_hidden_pads (no args): the absent ones go, the present one stays', (await countCalls(page, 'forget_all_hidden_pads')) === nAll + 1 &&
      (await lastCall(page, 'forget_all_hidden_pads'))?.args == null && m.rows.length === 1 && m.rows[0].id === DS_USB && m.rows[0].tag === '連線中', J());
    await setKnown([{ kind: 'dualsense', id: DS_USB }, { kind: 'ns_pro', id: NSP_BT }], [DS_USB, NSP_BT]);
    m = await measure();
    check('全部刪除 disabled when every remembered pad is present', m.all === '全部刪除' && m.allDis === true && m.rows.every(r => r.dis && r.tag === '連線中'), J());
    await setKnown([{ kind: 'dualsense', id: DS_BT }, { kind: 'ns_pro', id: NSP_BT }], []);
    await page.click('#board [data-act="forgetall"]');
    await settle(page);
    m = await measure();
    check('全部刪除 with none present: empty text shows', m.rows.length === 0 && m.empty === '還沒有記住的手把' && m.all === null, J());
    // v3.7.0: the Xbox Nano (Bluetooth LE, HID over GATT) shows as Xbox Nano 藍牙 with its address's last two bytes.
    const NANO = 'HID\\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&02045e_PID&0b4f_REV&0609_fb3a7c7df9a8&Col01\\8&113d3dd1&0&0000';
    await setKnown([{ kind: 'xbox_nano', id: NANO, addr: 'FB3A7C7DF9A8' }, { kind: 'ns_pro', id: NSP_BT, addr: A_NSP }], [NANO]);
    m = await measure();
    check('記住的手把: the Xbox Nano row reads Xbox Nano 藍牙 F9:A8 連線中; its title starts with the whole address', m.rows.length === 2 &&
      /^Xbox Nano\s*藍牙\s*F9:A8\s*連線中\s*刪除$/.test(m.rows[0].text) && m.rows[0].title === '藍牙位址 FB:3A:7C:7D:F9:A8\n' + NANO && m.rows[0].dis === true, J());
    check('記住的手把 (Nano): whole pixels, text inside its box, at least 2 px spare', m.bad.length === 0 && m.rows.every(r => r.free >= 2), J());
    const many = [], here = [];
    for (let i = 0; i < 16; i++) {
      // Bluetooth rows carry an address (the last one is an old entry without); its last two bytes are the row's i.
      // Worst cases for width (Cubic 11 is proportional: A / D are the widest hex glyphs): row 2,
      // Switch Pro 連線中, ends DD:DD; row 9, DualSense 連線中, ends AD:DA.
      const ad = 'A0B1C2D3' + (i === 2 ? 'DDDD' : i === 9 ? 'ADDA' : (0x2300 + i).toString(16).toUpperCase().padStart(4, '0'));
      const k = i % 3 === 2 ? { kind: 'ns_pro', id: NSP_BT.replace('9&1&0', '9&' + i + '&0'), addr: ad }
        : { kind: 'dualsense', id: (i % 2 ? DS_BT : DS_USB).replace(/0000$/, String(1000 + i)), ...(i % 2 && i !== 15 ? { addr: i === 1 ? A_DS : ad } : {}) };
      many.push(k);
      if ([1, 2, 9, 11].includes(i)) here.push(k.id);   // DualSense 藍牙 and Switch Pro 藍牙 連線中 in both columns
    }
    await setKnown(many, here);
    m = await measure();
    const left = m.rows.slice(0, 8), right = m.rows.slice(8);
    check('記住的手把 (16): two columns of eight, left first', m.rows.length === 16 && left.every(r => r.x === left[0].x) && right.every(r => r.x === right[0].x) &&
      right[0].x > left[0].x && left.every((r, i) => i === 0 || r.y > left[i - 1].y) && right.every((r, i) => r.y === left[i].y), J());
    check('記住的手把 (16): DualSense 藍牙 23:D2 / AD:DA 連線中, Switch Pro 藍牙 DD:DD 連線中 rows carry the serial and the tag', m.rows.filter(r => r.tag === '連線中').length === 4 &&
      /^DualSense\s*藍牙\s*23:D2\s*連線中\s*刪除$/.test(m.rows[1].text) && /^Switch Pro\s*藍牙\s*DD:DD\s*連線中\s*刪除$/.test(m.rows[2].text) &&
      /^DualSense\s*藍牙\s*AD:DA\s*連線中\s*刪除$/.test(m.rows[9].text) &&
      /^Switch Pro\s*藍牙\s*23:0B\s*連線中/.test(m.rows[11].text), J());
    check('記住的手把 (16): a Bluetooth row without an address (an old entry) and the USB rows show no serial',
      m.rows[15].serial === null && /^DualSense\s*藍牙\s*刪除$/.test(m.rows[15].text) && m.rows[15].title === many[15].id &&
      m.rows.filter((r, i) => many[i].id.includes('MI_03')).every(r => r.serial === null), J());
    check('記住的手把 (16): pane does not scroll, whole pixels, text inside its box and centred', m.sh === m.ch && m.sw === m.cw && m.bad.length === 0, J());
    const tight = m.rows.reduce((a, r) => (r.free < a.free ? r : a));
    check('記住的手把 (16): every row keeps at least 2 px spare (the widest serials included)', m.rows.every(r => r.free >= 2),
      'row ' + tight.w + 'px; tightest "' + tight.text + '": ' + tight.free + 'px spare');
    console.log('  記住的手把 (16): row ' + tight.w + 'px; the tightest row "' + tight.text + '" has ' + tight.free + 'px spare');
    await page.screenshot({ path: path.join(outDir, 'web-known-16.png') });
    await setKnown([]);
    await page.click('#board [data-act="setclose"]');
    await settle(page);
    // The 一般 pane is unchanged and still fits.
    await page.click('#board [data-act="setopen"]');
    await page.click('#board [data-stab="gen"]');
    await settle(page);
    const g = await page.evaluate(() => { const p = document.querySelector('#board .spane'); return [p.scrollHeight, p.clientHeight, p.querySelectorAll('.knr, .knbox').length]; });
    check('一般 pane unchanged: no 記住的手把 section, no scroll', g[0] === g[1] && g[2] === 0, JSON.stringify(g));
    await page.click('#board [data-act="setclose"]');
    await settle(page);
  }

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
  if (await page.locator('#board .setw').count()) await page.click('#board [data-act="setclose"]');
  await settle(page);
  await page.evaluate(() => { window.__mock.sticks = null; });

  // ── v3.4.0: the keyboard only lights and binds keys ───────────────────────
  // No shortcuts and no browser key behaviour on the board: every real key
  // lights its palette tile while held (bound or not, in every mode, except
  // while typing in a text field), and binds a waiting slot. Disabled tiles
  // (Caps / Ins / Menu) never light.
  {
    const { MAIN, NAV } = await import(path.join(webDir, 'board', 'keys.js'));
    const CODE = { LShift: 'ShiftLeft', RShift: 'ShiftRight', LCtrl: 'ControlLeft', RCtrl: 'ControlRight', LAlt: 'AltLeft', RAlt: 'AltRight',
      Win: 'MetaLeft', '↑': 'ArrowUp', '↓': 'ArrowDown', '←': 'ArrowLeft', '→': 'ArrowRight', Esc: 'Escape', Space: 'Space', Enter: 'Enter',
      Tab: 'Tab', Backspace: 'Backspace', Del: 'Delete', Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown', '-': 'Minus',
      '=': 'Equal', '[': 'BracketLeft', ']': 'BracketRight', '\\': 'Backslash', ';': 'Semicolon', "'": 'Quote', ',': 'Comma', '.': 'Period', '/': 'Slash', '`': 'Backquote' };
    const codeOf = t => CODE[t] || (/^[A-Z]$/.test(t) ? 'Key' + t : /^\d$/.test(t) ? 'Digit' + t : /^F\d+$/.test(t) ? t : null);
    const all = [...MAIN, ...NAV].flat().filter(x => x && x[0]);
    const usable = all.filter(x => x[2] !== 'dis').map(x => x[0]);
    const tileEsc = t => page.locator(`#board [data-tile="${t.replace(/["\\]/g, '\\$&')}"]`);
    const hit = async t => ((await tileEsc(t).getAttribute('class')) || '').split(' ').includes('hit');
    const lit = async (code, t) => {
      await page.keyboard.down(code);
      await page.waitForTimeout(60);   // past the 30 ms synthesized-key window
      const on = await hit(t);
      await page.keyboard.up(code);
      await page.waitForTimeout(20);
      return on && !(await hit(t));
    };
    check('palette has 80 bindable keys', usable.length === 80, String(usable.length));
    check('undo / redo titles name no shortcut', (await page.locator('#board [data-act="undo"]').getAttribute('title')) === '復原' &&
      (await page.locator('#board [data-act="redo"]').getAttribute('title')) === '重做');

    // The activity check above left the engine holding Space; release it.
    await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'Space', action: 'up' }));
    await settle(page);
    // A focused button: Enter / Space must not press it, Tab must not move focus.
    await page.evaluate(() => {
      window.__alive = true;
      window.__focusMoves = [];
      document.addEventListener('focusin', e => window.__focusMoves.push(e.target.dataset?.act || e.target.tagName));
    });
    const mut0 = (await calls(page)).length;
    for (const k of ['Enter', 'Space', 'Tab']) {
      await page.evaluate(() => document.querySelector('#board [data-act="setopen"]').focus());
      await page.keyboard.press(k);
      await settle(page);
    }
    const moves = await page.evaluate(() => window.__focusMoves.filter(a => a !== 'setopen'));
    check('Enter / Space do not press the focused button, Tab does not move focus', moves.length === 0 &&
      (await page.locator('#board .setw').count()) === 0, moves.join(' '));
    const dark = [];
    for (const t of usable) if (!(await lit(codeOf(t), t))) dark.push(t);
    check('every key lights its tile while held (nothing waiting)', dark.length === 0, dark.join(' '));
    const st = await page.evaluate(() => ({ alive: window.__alive === true, moves: window.__focusMoves.filter(a => a !== 'setopen') }));
    check('no key reloads the page or moves focus', st.alive && st.moves.length === 0, JSON.stringify(st));
    check('no key opens a window or sends a command', (await page.locator('#board .setw, #board .med').count()) === 0 &&
      (await calls(page)).slice(mut0).every(c => c.cmd.startsWith('get_')), JSON.stringify((await calls(page)).slice(mut0).map(c => c.cmd)));
    const disLit = [];
    for (const [t, code] of [['Caps', 'CapsLock'], ['Ins', 'Insert'], ['Menu', 'ContextMenu']]) {
      await page.keyboard.down(code);
      await page.waitForTimeout(60);
      const cls = (await page.locator(`#board .pal button[disabled][aria-label^="${t}（"]`).getAttribute('class')) || '';
      if (cls.split(' ').includes('hit')) disLit.push(t);
      await page.keyboard.up(code);
    }
    check('Caps / Ins / Menu never light', disLit.length === 0, disLit.join(' '));

    // Every key binds a waiting slot, Ctrl / Win / Esc included.
    const unbound = [];
    for (const t of usable) {
      const n0 = await countCalls(page, 'set_binding');
      await slot(page, '3').click();
      await settle(page);
      await page.keyboard.press(codeOf(t));
      await settle(page);
      const b = (await countCalls(page, 'set_binding')) > n0 ? (await lastCall(page, 'set_binding')).args.entry : null;
      const back = b ? await page.evaluate(v => import('/board/model.js').then(m => m.keyToTile(v)), b.value) : null;
      if (back !== t) unbound.push(`${t}→${back}`);
      if (await page.locator('#board .slot.sel').count()) { await slot(page, '3').click(); await settle(page); }
    }
    check('every key binds a waiting slot (Ctrl, Win, Esc too)', unbound.length === 0, unbound.join(' '));

    // Lights in the macro editor and in settings too, but not while typing.
    await page.locator('#board [data-medit]').first().click();
    await settle(page);
    const inEditor = (await page.locator('#board .med').count()) === 1 && await lit('KeyQ', 'Q');
    await page.locator('#board .med .mname').focus();
    await page.keyboard.down('KeyQ');
    await page.waitForTimeout(60);
    const typingLit = await hit('Q');
    await page.keyboard.up('KeyQ');
    check('keys light in the macro editor, not while typing its name', inEditor && !typingLit, `${inEditor} ${typingLit}`);
    if (await page.locator('#board .med').count()) { await page.click('#board .med [data-act="mdone"]'); await settle(page); }
    await page.click('#board [data-act="setopen"]');
    await settle(page);
    check('keys light while settings are open', await lit('KeyQ', 'Q'));
    await page.click('#board [data-act="setclose"]');
    await settle(page);
  }

  // ── v3.4.0: settings open over the macro editor ──────────────────────────
  // The gear works while a macro is being edited; the draft is untouched by
  // 儲存 / 取消, and both changes land (they write different config fields).
  {
    for (let i = 0; i < 5 && !(await page.locator('#board .macros .mnew').count()); i++) { await page.click('#board [data-act="mnext"]'); await settle(page); }
    await page.click('#board .macros .mnew');
    await settle(page);
    await tile(page, 'A').click();
    await page.locator('#board .med .mname').fill('設定中');
    await page.locator('#board .med .mname').dispatchEvent('input');
    await page.click('#board .med [data-act="sdelay"][data-i="0"]');
    await settle(page);
    const draft = async () => ({
      med: await page.locator('#board .med').count(),
      steps: await page.locator('#board .med .st[data-step]:not([data-step="end"])').count(),
      name: await page.locator('#board .med .mname').inputValue().catch(() => null),
    });
    const same = d => d.med === 1 && d.steps === 2 && d.name === '設定中';
    await page.click('#board [data-act="setopen"]');
    await settle(page);
    const d1 = await draft();
    check('the gear opens settings over the macro editor', (await page.locator('#board .setw').count()) === 1 && same(d1), JSON.stringify(d1));
    await page.click('#board [data-stab="stick"]');
    await page.locator('#board [data-f="deadzone"]').fill('0.35');
    await page.locator('#board [data-f="deadzone"]').dispatchEvent('input');
    await page.click('#board [data-act="setsave"]');
    await settle(page);
    const d2 = await draft();
    check('儲存 closes settings back to the untouched draft', (await page.locator('#board .setw').count()) === 0 && same(d2) &&
      Math.abs((await lastCall(page, 'set_settings'))?.args?.settings?.deadzone - 0.35) < 1e-6, JSON.stringify(d2));
    await page.click('#board [data-act="setopen"]');
    await settle(page);
    await page.click('#board [data-act="setclose"]');
    await settle(page);
    const d3 = await draft();
    check('取消 closes settings back to the untouched draft', (await page.locator('#board .setw').count()) === 0 && same(d3), JSON.stringify(d3));
    await page.click('#board .med [data-act="mdone"]');
    await settle(page);
    const sv = await lastCall(page, 'set_macro');
    const kept = await page.evaluate(() => ({ dz: window.__mock.config.deadzone, m: window.__mock.config.macros['設定中'] }));
    check('完成 saves the macro and keeps the saved settings', sv?.args?.name === '設定中' && sv.args.def.steps.length === 2 &&
      kept.m?.steps?.length === 2 && Math.abs(kept.dz - 0.35) < 1e-6, JSON.stringify(kept));
  }

  // ── v3.4.0: one file beside the exe ───────────────────────────────────────
  // The last pad is a config field; remembering it is not a settings change
  // in the activity log.
  {
    await page.evaluate(() => window.__mockEmit('activity', { ts_ms: Date.now(), kind: 'macro-start', name: '探針' }));
    await page.click('#board [data-pad="ns_pro"]');
    await settle(page);
    check('the last pad is written into the config', await page.evaluate(() => window.__mock.config.last_pad) === 'ns_pro');
    await page.click('#board [data-act="setopen"]');
    await settle(page);
    await page.click('#board [data-stab="gen"]');
    await settle(page);
    const top = ((await page.locator('#board #logrows .lr').first().textContent()) || '').trim();
    check('a pad switch logs no 設定已變更', /巨集「探針」開始/.test(top), top);
    await page.click('#board [data-act="setclose"]');
    await settle(page);
    await page.click('#board [data-pad="dualsense"]');
    await settle(page);
  }

  // ── v3.7.0: photo-traced pad art ──────────────────────────────────────────
  // DualSense = photo art + shoulder F, Switch Pro / Switch 2 Pro = photo art + shoulder A (user Q13 / Q14),
  // Xbox Nano = traced art variant 1 (short band):
  // every L／R label sits inside its visible key face, and no shoulder sticks to a key.
  {
    const SHOULDER_GAP = { dualsense: { min: 5, skip: [[82, 50.5], [218, 50.5]] }, ns_pro: { min: 4 }, ns2_pro: { min: 4 }, xbox_nano: { min: 4 } };
    const pads = await page.locator('#board .hdr [data-pad]').evaluateAll(els => els.map(e => e.dataset.pad));
    for (const pad of pads) {
      await page.click(`#board .hdr [data-pad="${pad}"]`);
      await settle(page);
      await page.screenshot({ path: path.join(outDir, `web-pad-${pad}.png`) });
      const m = await page.evaluate(padArtMetrics, { skip: SHOULDER_GAP[pad]?.skip });
      const lab = m.labels.map(l => `${l.at} ${l.clr}`).join(', ');
      check(`${pad}: every L／R label ≥ 1.45 inside its visible key face (8x raster)`, m.labels.length === 4 && m.labels.every(l => l.clr >= 1.45), lab || 'no labels');
      if (SHOULDER_GAP[pad]) {
        const g = SHOULDER_GAP[pad];
        check(`${pad}: L／R shoulders ≥ ${g.min} from every key${g.skip ? ' but Create / Options' : ''}`, m.gap.keys >= g.min,
          `keys ${m.gap.keys}${g.skip ? `, Create / Options ${m.gap.skipped}` : ''}`);
      }
      if (pad === 'dualsense') {
        // The light strips run along the touchpad's slanted sides: left edge 88.6 -> 96.4 over y 38 -> 84, 3 wide, mirrored.
        const polys = await page.evaluate(() => [...document.querySelectorAll('#board .lbarsvg .lbar polygon:not([opacity])')].map(p => p.getAttribute('points').split(/[ ,]/).map(Number)));
        const want = [[88.6, 38, 91.6, 38, 99.4, 84, 96.4, 84], [211.4, 38, 208.4, 38, 200.6, 84, 203.6, 84]];
        check('DualSense light strips follow the touchpad sides', polys.length === 2 && polys.every((p, i) => p.length === 8 && p.every((v, k) => Math.abs(v - want[i][k]) < 1e-6)), JSON.stringify(polys));
      }
    }
    await page.click('#board .hdr [data-pad="dualsense"]');
    await settle(page);
  }

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
