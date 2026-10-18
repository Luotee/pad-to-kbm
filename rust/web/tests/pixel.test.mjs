import test from 'node:test';
import assert from 'node:assert/strict';
import { pixelSvg, withRing } from '../board/pixel.js';

// Reskin Q24: icons are pixel grids, one path per colour, each run of a row one rect.
test('pixel_svg_merges_runs_per_colour', () => {
  const s = pixelSvg(['#.#', '###'], { '#': 'currentColor' });
  assert.equal(s, '<svg width="3" height="2" viewBox="0 0 3 2" shape-rendering="crispEdges" aria-hidden="true">' +
    '<path d="M0 0h1v1h-1zM2 0h1v1h-1zM0 1h3v1h-3z" fill="currentColor"/></svg>');
});

test('pixel_svg_scales_by_whole_pixels_only', () => {
  assert.match(pixelSvg(['#'], { '#': '#000' }, 3), /^<svg width="3" height="3" viewBox="0 0 1 1"/);
  assert.throws(() => pixelSvg(['#'], { '#': '#000' }, 1.5));
});

test('pixel_svg_ring_takes_the_iol_class', () => {
  const s = pixelSvg(withRing(['#']), { '#': 'currentColor', r: 'iol' });
  assert.match(s, /<path class="iol" d="M0 0h3v1h-3zM0 1h1v1h-1zM2 1h1v1h-1zM0 2h3v1h-3z"\/>/);
});

test('with_ring_pads_and_marks_the_8_neighbours', () => {
  assert.deepEqual(withRing(['#.', '..']), ['rrr.', 'r#r.', 'rrr.', '....']);
});
