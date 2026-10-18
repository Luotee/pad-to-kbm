import test from 'node:test';
import assert from 'node:assert/strict';
import { chromeHTML, BTN, FRAME, fitWindow } from '../chrome.js';

// Reskin Q19/Q20: the app draws the MapleStory window: steel frame, white title bar "PAD TO KBM",
// minimise + close in the reference's 12x12 checker style.
test('title_bar_has_title_minimise_close', () => {
  const h = chromeHTML();
  assert.match(h, /<div id="wtbar"><span class="wt">PAD TO KBM<\/span>/);
  assert.match(h, /<button type="button" class="wb" data-win="min" aria-label="縮小" title="縮小"><svg width="12" height="12"/);
  assert.match(h, /<button type="button" class="wb" data-win="close" aria-label="關閉" title="關閉"><svg width="12" height="12"/);
  assert.ok(h.indexOf('data-win="min"') < h.indexOf('data-win="close"'));
});

test('buttons_are_reference_pixels', () => {
  // black edge with cut corners, ten blue bands, 2x2 glyph blocks on a 3x3 grid from (3,3)
  assert.match(BTN.close, /<path d="M1 0h10v1h-10zM1 11h10v1h-10zM0 1h1v10h-1zM11 1h1v10h-1z" fill="#000714"\/>/);
  assert.equal((BTN.close.match(/height="1" fill=/g) || []).length, 10);
  assert.equal((BTN.close.match(/fill="#c4fcff"/g) || []).length, 5);   // pale corners + centre = the X
  assert.equal((BTN.close.match(/fill="#3a7fa8"/g) || []).length, 4);   // dark edges
  assert.equal((BTN.min.match(/fill="#c4fcff"/g) || []).length, 3);     // bottom bar
});

test('frame_cuts_the_corners', () => {
  assert.match(FRAME, /<polygon points="4.5,0.5 989.5,0.5 993.5,4.5 993.5,664.5 989.5,668.5 4.5,668.5 0.5,664.5 0.5,4.5" fill="none" stroke="#f7ffff"\/>/);
});

// Reskin Q23: the whole window is drawn at a whole number of device pixels per art pixel.
// Rust sizes the window to W*n x H*n physical pixels; fitWindow finds n back from the CSS size.
test('fit_uses_whole_device_pixels', () => {
  assert.deepEqual(fitWindow(994, 669, 1), { n: 1, k: 1, x: 0, y: 0, margin: false });
  assert.deepEqual(fitWindow(2982 / 2.5, 2007 / 2.5, 2.5), { n: 3, k: 1.2, x: 0, y: 0, margin: false });
  assert.deepEqual(fitWindow(994 / 1.25, 669 / 1.25, 1.25), { n: 1, k: 0.8, x: 0, y: 0, margin: false });
  assert.deepEqual(fitWindow(1988 / 1.5, 1338 / 1.5, 1.5), { n: 2, k: 2 / 1.5, x: 0, y: 0, margin: false });
});

test('fit_centres_on_device_pixels_when_bigger', () => {
  assert.deepEqual(fitWindow(1500, 900, 1), { n: 1, k: 1, x: 253, y: 116, margin: true });
  const f = fitWindow(1000, 700, 2.5);   // 2500x1750 device: 2x fits (1988x1338)
  assert.equal(f.n, 2);
  assert.equal(Math.round(f.x * 2.5), f.x * 2.5);
  assert.equal(Math.round(f.y * 2.5), f.y * 2.5);
});

test('fit_never_scales_to_zero', () => {
  const f = fitWindow(497, 334.5, 1);
  assert.equal(f.n, 0);
  assert.equal(f.k, 0.5);
  assert.ok(f.k > 0);
});

// Reskin Q24 B: the settings ✕ is the title-bar close key, drawn 2x.
test('win_key_scales_the_reference_key', async () => {
  const { winKey } = await import('../chrome.js');
  assert.equal(winKey('close'), BTN.close);
  assert.match(winKey('close', 24), /^<svg width="24" height="24" viewBox="0 0 12 12"/);
});
