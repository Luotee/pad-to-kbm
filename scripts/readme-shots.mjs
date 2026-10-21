// readme-shots.mjs — regenerate the README screenshots in docs/images/readme/.
//
// Same approach as scripts/verify/web-smoke.mjs: serve rust/web/, inject a
// mock window.__TAURI__ before the module scripts run, and drive the board
// with Playwright + the cached Chromium (WebView2 is Chromium too). The mock
// serves a demo config (rust/config.example.json plus a few bindings, quick
// pages and two macros), so the pictures never show anyone's real setup.
//
//   NODE_PATH=rust/target/verify/node/node_modules node scripts/readme-shots.mjs \
//     --chrome ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome
//
// (scripts/verify.sh --only web installs playwright-core into
// rust/target/verify/node the first time.)

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright-core');

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const webDir = path.join(repo, 'rust/web');
const outDir = path.join(repo, 'docs/images/readme');
const argv = process.argv.slice(2);
const ci = argv.indexOf('--chrome');
const chrome = ci >= 0 ? argv[ci + 1] : null;
if (!chrome) throw new Error('missing --chrome <path>');
const version = /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(repo, 'rust/Cargo.toml'), 'utf8'))[1];
fs.mkdirSync(outDir, { recursive: true });

// ─── Demo config ─────────────────────────────────────────────────────────────

const config = JSON.parse(fs.readFileSync(path.join(repo, 'rust/config.example.json'), 'utf8'));
for (const pad of ['dualsense', 'ns_pro', 'ns2_pro']) {
  const p = config.pads[pad], b = p.buttons;
  const key = (id, value) => { b[id] = { label: b[id].label, type: 'key', value }; };
  key('0', 'LAlt'); key('1', 'z'); key('2', 'LShift'); key('3', 'a');
  key('9', 'q'); key('10', 'e'); key('6', 'Esc');
  b['8'] = { label: b['8'].label, type: 'macro', value: '連發' };
  p.quick_l = {
    '11': { label: 'D-pad ↑', type: 'key', value: 'F1' }, '12': { label: 'D-pad ↓', type: 'key', value: 'F2' },
    '13': { label: 'D-pad ←', type: 'key', value: 'F3' }, '14': { label: 'D-pad →', type: 'key', value: 'F4' },
    '3': { label: 'Triangle', type: 'macro', value: '跳躍攻擊' },
  };
  p.quick_r = { '0': { label: 'Cross', type: 'key', value: '1' }, '1': { label: 'Circle', type: 'key', value: '2' }, '2': { label: 'Square', type: 'key', value: '3' }, '3': { label: 'Triangle', type: 'key', value: '4' } };
}
config.mapping_enabled = { ...config.mapping_enabled, ns2_pro: true };
// The light bar shots: 鍵鼠時 彩虹 (the default), 非鍵鼠時 呼吸 in 天藍 so both sections show their rows.
config.lightbar = { ...config.lightbar, other: { ...config.lightbar.other, effect: 'breathe', color: '#2EA8FF' } };
config.macros = {
  連發: { loop: true, steps: [{ key: 'j', action: 'down', delay_ms: [30, 50] }, { key: 'j', action: 'up', delay_ms: [15, 25] }] },
  跳躍攻擊: {
    loop: false,
    steps: [
      { key: 'LAlt', action: 'down', delay_ms: [40, 60] }, { key: 'LAlt', action: 'up', delay_ms: [15, 25] },
      { key: 'z', action: 'down', delay_ms: [40, 60] }, { key: 'z', action: 'up', delay_ms: [120, 180] },
    ],
  },
};

// ─── Static server ───────────────────────────────────────────────────────────

const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.woff2': 'font/woff2',
};
const server = http.createServer((req, res) => {
  const p = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = path.resolve(webDir, p === '/' ? 'index.html' : p.replace(/^\/+/, ''));
  if (!file.startsWith(webDir + path.sep) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
  fs.createReadStream(file).pipe(res);
});
await new Promise(r => server.listen(0, '127.0.0.1', r));

// ─── Tauri mock (runs in the page) ───────────────────────────────────────────

function tauriMock({ config, version }) {
  const listeners = {};
  let nextId = 1;
  const status = { name: 'DualSense', transport: 'BT', kind: 'dualsense', warnings: [], connected: ['dualsense', 'ns2_pro'], off_pads: [] };
  window.__emit = (name, payload) => {
    for (const l of (listeners[name] || []).slice()) l.cb({ event: name, id: l.id, payload });
  };
  const clone = v => JSON.parse(JSON.stringify(v));
  async function invoke(cmd) {
    switch (cmd) {
      case 'get_controller_status': return clone(status);
      case 'get_config': return clone(config);
      case 'get_ui_prefs': return { last_pad: 'dualsense' };
      case 'get_app_version': return version;
      case 'get_config_path': return 'D:\\Games\\PadToKBM\\padtokbm.json';
      case 'set_pad_off': return [];
      case 'get_vigem_status': return 'installed';
      case 'install_vigem': return 'installed';
      default: return null;
    }
  }
  window.__TAURI__ = {
    core: { invoke },
    event: {
      listen: async (name, cb) => {
        const id = nextId++;
        (listeners[name] = listeners[name] || []).push({ id, cb });
        return () => { listeners[name] = listeners[name].filter(l => l.id !== id); };
      },
    },
  };
}

// ─── Shots ───────────────────────────────────────────────────────────────────

const browser = await chromium.launch({ executablePath: chrome, headless: true });
const errors = [];
try {
  const context = await browser.newContext({ viewport: { width: 994, height: 669 }, deviceScaleFactor: 2 });   // the drawn window at 2x
  await context.addInitScript(tauriMock, { config, version });
  const page = await context.newPage();
  page.on('pageerror', e => errors.push(String(e)));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.waitForFunction(() => document.querySelectorAll('#board .slot').length > 20);
  await page.evaluate(() => document.fonts.ready);
  const shot = async (name, clip) => {
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(outDir, name), ...(clip ? { clip } : {}) });
    console.log(`wrote docs/images/readme/${name}`);
  };
  const emit = (name, payload) => page.evaluate(([n, p]) => window.__emit(n, p), [name, payload]);
  // The pad-art light strips animate (彩虹 by default): hold them at their first frame so reruns give the same pictures.
  await page.addStyleTag({ content: '#board .lbar{animation-play-state:paused!important;animation-delay:0s!important}' });

  // 1. The board, DualSense shown, Cross held: slot, line and tile light up.
  await emit('button-down', { id: 0 });
  await emit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'LAlt', action: 'down', source: { button: 0 } });
  await shot('board.png');
  await emit('button-up', { id: 0 });
  await emit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'LAlt', action: 'up' });

  // 2. Light bar (DualSense): the 燈條 dropdown with its 自訂 panel, then the colour picker on 非鍵鼠時.
  //    Cropped to the pad and the panel.
  const lbClip = { x: 430, y: 84, width: 545, height: 330 };
  await page.click('#board .lbdd');
  await shot('lightbar.png', lbClip);
  await page.click('#board .lbpop .lbadd[data-which="other"]');
  await shot('lightbar-picker.png', lbClip);
  await page.click('#board .lbcpop .lbxk');
  await page.click('#board .lbdd');

  // 3. Quick-slot page: ZL held on the Switch 2 Pro lights its page slot.
  await page.click('#board [data-pad="ns2_pro"]');
  await emit('button-down', { id: 23 });
  await emit('button-down', { id: 13 });
  await emit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'F3', action: 'down' });
  await shot('quickslots.png');
  await emit('button-up', { id: 13 });
  await emit('button-up', { id: 23 });
  await emit('activity', { ts_ms: Date.now(), kind: 'emit', key: 'F3', action: 'up' });

  // 4. Key capture: a slot waits for a key.
  await page.click('#board .slot[data-slot="4"]');
  await shot('capture.png');
  await page.keyboard.press('Escape');

  // 5. Settings → 一般, with recent events.
  await page.click('#board [data-pad="dualsense"]');
  await page.click('#board [data-act="setopen"]');
  // Each row names what sent it (v3.2.0): Cross → LAlt, Triangle → its macro.
  for (const [kind, extra] of [['emit', { key: 'LAlt', action: 'down', source: { button: 0 } }], ['emit', { key: 'LAlt', action: 'up', source: { button: 0 } }],
    ['macro-start', { name: '跳躍攻擊', button: 3 }], ['emit', { key: 'LAlt', action: 'down', source: { macro: '跳躍攻擊' } }],
    ['emit', { key: 'LAlt', action: 'up', source: { macro: '跳躍攻擊' } }], ['emit', { key: 'Space', action: 'down', source: { button: 9 } }]]) {
    await emit('activity', { ts_ms: Date.now(), kind, ...extra });
    await page.waitForTimeout(40);
  }
  await shot('settings.png');
  await page.click('#board [data-act="setclose"]');

  // 6. Macro editor.
  await page.click('#board [data-medit="跳躍攻擊"]');
  await shot('macro-editor.png');
  await page.click('#board .med [data-act="mdone"]');


} finally {
  await browser.close();
  server.close();
}
if (errors.length) { console.error(errors.join('\n')); process.exit(1); }
