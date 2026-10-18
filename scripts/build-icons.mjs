// build-icons.mjs — render the app and tray icons from the pixel art in
// scripts/icon-art.mjs (reskin Q25). Each size is a whole multiple of a
// hand-placed grid, rendered by Chromium at its real pixel size and packed
// into rust/icons/*.ico:
//   icon.ico               16 / 24 / 32 / 48 / 256, full colour
//   tray-connected.ico     32, full colour
//   tray-disconnected.ico  32, desaturated + faded
// The tray ICOs hold one 32 px entry: tauri's Image::from_bytes keeps only the
// largest ICO entry. Previews (every PNG + an 8x pixelated sheet.png on light
// and dark backgrounds) go to rust/target/icons-preview/.
//
//   NODE_PATH=rust/target/verify/node/node_modules node scripts/build-icons.mjs \
//     --chrome ~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome
//
// (scripts/verify.sh --only web installs playwright-core into
// rust/target/verify/node the first time.)

import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { packIco } from './ico.mjs';
import { appIconSvg } from './icon-art.mjs';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright-core');

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const iconDir = path.join(repo, 'rust/icons');
const previewDir = path.join(repo, 'rust/target/icons-preview');
const argv = process.argv.slice(2);
const ci = argv.indexOf('--chrome');
const chrome = ci >= 0 ? argv[ci + 1] : null;
if (!chrome) throw new Error('missing --chrome <path>');

const ICONS = [
  { name: 'icon', sizes: [16, 24, 32, 48, 256], grey: false },
  { name: 'tray-connected', sizes: [32], grey: false },
  { name: 'tray-disconnected', sizes: [32], grey: true },
];

const css = 'html,body{margin:0;background:transparent}svg{display:block}';

const browser = await chromium.launch({ executablePath: chrome });
const page = await browser.newPage({ deviceScaleFactor: 1 });
fs.mkdirSync(previewDir, { recursive: true });

async function render(size, grey) {
  const svg = appIconSvg(size, { grey });
  await page.setViewportSize({ width: size, height: size });
  await page.setContent(`<!doctype html><style>${css}</style>${svg}`);
  return page.locator('svg').screenshot({ omitBackground: true });
}

const previews = [];
for (const icon of ICONS) {
  const entries = [];
  for (const size of icon.sizes) {
    const png = await render(size, icon.grey);
    fs.writeFileSync(path.join(previewDir, `${icon.name}-${size}.png`), png);
    entries.push({ size, png });
    previews.push({ label: `${icon.name} ${size}`, size, png: png.toString('base64') });
  }
  const file = path.join(iconDir, `${icon.name}.ico`);
  fs.writeFileSync(file, packIco(entries));
  console.log(`${path.relative(repo, file)}: ${icon.sizes.join('/')} px`);
}

// Contact sheet: every PNG at 8x (256 at 1x), nearest-neighbour, on light and dark.
const cell = p => {
  const z = p.size >= 128 ? p.size : p.size * 8;
  return `<figure><img src="data:image/png;base64,${p.png}" width="${z}" height="${z}"><figcaption>${p.label}</figcaption></figure>`;
};
const strip = bg => `<div class="row" style="background:${bg}">${previews.map(cell).join('')}</div>`;
await page.setViewportSize({ width: 1800, height: 800 });
await page.setContent(`<!doctype html><style>body{margin:0;font:14px sans-serif}
.row{display:flex;flex-wrap:wrap;align-items:flex-end;gap:20px;padding:20px}
img{image-rendering:pixelated;display:block}figure{margin:0}
figcaption{color:#888;text-align:center;padding-top:4px}</style>
${strip('#f3f3f3')}${strip('#202020')}`);
await page.screenshot({ path: path.join(previewDir, 'sheet.png'), fullPage: true });
console.log(`previews: ${path.relative(repo, previewDir)}/`);
await browser.close();
