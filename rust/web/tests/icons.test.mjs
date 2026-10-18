import test from 'node:test';
import assert from 'node:assert/strict';
import { ICON } from '../board/icons.js';

// Reskin Q24 B: every icon is pixel art in the text colour; only the icon-only blue keys carry a ring.
test('every_icon_is_crisp_pixel_art', () => {
  for (const [name, s] of Object.entries(ICON)) {
    assert.match(s, /shape-rendering="crispEdges"/, name);
    assert.doesNotMatch(s, /stroke|<circle|<rect [^>]*rx|[MLHVhvl]-?\d*\.\d/, name);
    const [, w, h, vw, vh] = /width="(\d+)" height="(\d+)" viewBox="0 0 (\d+) (\d+)"/.exec(s);
    assert.ok(w === vw && h === vh, name + ' drawn at 1 css px per art pixel');
  }
});

test('ring_only_on_blue_key_icons', () => {
  const ringed = Object.keys(ICON).filter(k => /class="iol"/.test(ICON[k])).sort();
  assert.deepEqual(ringed, ['gear', 'next', 'pen', 'prev']);
});

test('settings_close_uses_the_window_key', () => {
  assert.equal(ICON.close, undefined);
});
