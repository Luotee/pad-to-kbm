import test from 'node:test';
import assert from 'node:assert/strict';
import { padSvg } from '../board/padart.js';

test('board drawing is positioned on the board', () => {
  assert.match(padSvg('dualsense'), /^<svg class="pad"/);
});

// Reskin Q5: the board pad is MapleStory line art (navy ink, slate keys, no drop shadow, no colour).
test('board pad is line art', () => {
  const b = padSvg('dualsense');
  assert.doesNotMatch(b, /opacity="\.28"/);
  assert.match(b, /stroke="#2d4359"/);
  assert.doesNotMatch(b, /#a8587a|#b0405f|#ffcf2e/);
});


// v3.7.0: the Xbox Nano is drawn (variant 1): body, face plate, LT／RT behind it, LB／RB in front, and its
// labels as stroke letters (LT／RT／LB／RB, Y／B／A／X).
test('nano pad art', () => {
  const b = padSvg('xbox_nano'), roles = [...b.matchAll(/data-role="([a-zA-Z]+)"/g)].map(m => m[1]);
  for (const [r, n] of [['body', 1], ['plate', 1], ['trig', 2], ['bump', 2], ['face', 4], ['well', 2], ['dwell', 1], ['dcross', 1], ['small', 4]]) {
    assert.equal(roles.filter(x => x === r).length, n, r);
  }
  assert.ok(roles.indexOf('trig') < roles.indexOf('body') && roles.indexOf('body') < roles.indexOf('bump'), 'trig behind, bump in front');
  const glyphs = [...b.matchAll(/data-glyph="(\w+)"/g)].map(m => m[1]);
  assert.equal(glyphs.filter(g => g === 'trigT').length, 2);
  assert.equal(glyphs.filter(g => g === 'bumpT').length, 2);
  assert.equal(glyphs.filter(g => g === 'faceT').length, 4);
  assert.doesNotMatch(b, /<text/, 'every label is stroke letters');
});
