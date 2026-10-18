import test from 'node:test';
import assert from 'node:assert/strict';
import { keyboardHTML, mouseHTML, macroBoxHTML, dragGhostHTML } from '../board/palette.js';

const st = { picked: null, hot: {}, config: { macros: { 連招: {} } }, mpage: 0 };
const cls = (html, tile) => (html.match(new RegExp('class="([^"]*)"[^>]*data-tile="' + tile + '"')) || [])[1] || '';

// Reskin Q10: keys already bound on the shown pad are drawn pale (display only, still draggable).
test('bound_tiles_are_marked_used', () => {
  const used = new Set(['Q', '上滾', 'M:連招']);
  assert.match(cls(keyboardHTML(st, used), 'Q'), /\bused\b/);
  assert.doesNotMatch(cls(keyboardHTML(st, used), 'W'), /\bused\b/);
  assert.match(cls(mouseHTML(st, used), '上滾'), /\bused\b/);
  assert.match(cls(macroBoxHTML(st, '', used), 'M:連招'), /\bused\b/);
  assert.match(keyboardHTML(st, used), /data-tile="Q"[^>]*draggable|draggable="true" data-tile="Q"/);
});

test('mouse_wheel_tiles_read_short_names', () => {
  const h = mouseHTML(st, new Set());
  assert.match(h, /data-tile="上滾"[^>]*>上滾</);
  assert.match(h, /data-tile="下滾"[^>]*>下滾</);
});

// Reskin: every key label at the font's native 12px / Silkscreen 8px — no shrunken "sm" keys.
test('no_small_key_labels', () => {
  assert.doesNotMatch(keyboardHTML(st, new Set()), /class="tile[^"]*\bsm\b/);
});

// Silkscreen has no arrows: the keyboard arrow keys draw a 7x7 pixel arrow (white, 1px dark shadow).
test('keyboard_arrows_are_pixel_svgs', () => {
  const h = keyboardHTML(st, new Set());
  for (const k of ['↑', '↓', '←', '→']) {
    const m = h.match(new RegExp('data-tile="' + k + '"[^>]*>(.*?)</button>'));
    assert.ok(m, k);
    assert.match(m[1], /^<svg width="8" height="8"/, k);
    assert.ok(!m[1].includes(k), k + ' glyph not typed');
  }
});

// Reskin Q7 C: a dragged key shows as the 38px slot it will become, not as the keyboard tile.
test('drag_ghost_is_a_bound_slot', () => {
  assert.equal(dragGhostHTML('LShift'), '<div class="slot on dragimg"><span class="val">Shift</span></div>');
  assert.equal(dragGhostHTML('M:連招'), '<div class="slot on mac dragimg"><span class="val">連招</span></div>');
  assert.equal(dragGhostHTML('a<b'), '<div class="slot on dragimg"><span class="val">a&lt;b</span></div>');
});
