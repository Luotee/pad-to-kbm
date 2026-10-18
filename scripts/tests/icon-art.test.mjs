import test from 'node:test';
import assert from 'node:assert/strict';
import { appIconSvg, ART } from '../icon-art.mjs';

const box = s => /width="(\d+)" height="(\d+)" viewBox="0 0 (\d+) (\d+)"/.exec(s).slice(1).map(Number);

// Reskin Q25 4 甲: the app / tray icon is the NES-style pad in the reference's emoticon cream.
test('art_grids_are_square', () => {
  for (const [n, rows] of Object.entries(ART)) {
    assert.equal(rows.length, Number(n), n);
    for (const r of rows) assert.equal(r.length, Number(n), n);
  }
});

test('every_ico_size_is_a_whole_scale_of_a_hand_drawn_grid', () => {
  for (const [size, grid] of [[16, 16], [24, 24], [32, 32], [48, 16], [256, 32]]) {
    const [w, h, vw, vh] = box(appIconSvg(size));
    assert.deepEqual([w, h, vw, vh], [size, size, grid, grid], String(size));
  }
  assert.throws(() => appIconSvg(20));
});

test('cream_palette', () => {
  const s = appIconSvg(32);
  for (const c of ['#100804', '#ffefdc', '#e6cdb0', '#e0424f']) assert.match(s, new RegExp('fill="' + c + '"'), c);
});

test('grey_is_desaturated_and_faded', () => {
  const s = appIconSvg(32, { grey: true });
  assert.match(s, /<g opacity="\.55">/);
  for (const [, c] of s.matchAll(/fill="#(\w{6})"/g)) assert.ok(c.slice(0, 2) === c.slice(2, 4) && c.slice(2, 4) === c.slice(4, 6), c);
});
