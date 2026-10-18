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

