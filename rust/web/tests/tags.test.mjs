import test from 'node:test';
import assert from 'node:assert/strict';
import { tagSvg } from '../board/tags.js';

// Reskin Q11: the d-pad slot tags are solid 8px pixel triangles, not the keyboard's arrow glyphs.
test('dpad_tags_are_pixel_triangles', () => {
  const rot = { '↑': 0, '→': 90, '↓': 180, '←': 270 };
  for (const [t, r] of Object.entries(rot)) {
    const s = tagSvg(t);
    assert.match(s, /^<svg width="8" height="8" viewBox="0 0 8 8"/, t);
    assert.match(s, new RegExp('rotate\\(' + r + ' 4 4\\)'), t);
    assert.doesNotMatch(s, /<text/, t);
  }
});
