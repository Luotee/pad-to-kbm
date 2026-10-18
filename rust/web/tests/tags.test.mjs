import test from 'node:test';
import assert from 'node:assert/strict';
import { tagSvg, switchHTML } from '../board/tags.js';

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

// Reskin Q17: every on/off switch is a 關｜開 pair (two real spans, so the glyphs are font-checked).
test('switch_is_off_on_pair', () => {
  assert.equal(switchHTML('rumble', '連線震動', true),
    '<button type="button" class="sw on" role="switch" aria-checked="true" data-act="rumble"><span>連線震動</span><span class="track"><span>關</span><span>開</span></span></button>');
  assert.match(switchHTML('mloop', '按住時重複', false, { cls: 'lt', title: 't', icon: '<svg></svg>', disabled: true }),
    /^<button type="button" class="sw lt" role="switch" aria-checked="false" data-act="mloop" title="t" disabled><svg><\/svg><span>按住時重複<\/span>/);
});
