import test from 'node:test';
import assert from 'node:assert/strict';
import { padIconSvg, padSvg } from '../board/padart.js';

const full = { glyphs: true, small: true, strokeScale: 1, grey: false };
const viewBox = s => /viewBox="([^"]+)"/.exec(s)[1].split(/\s+/).map(Number);
const bodyStroke = s => /fill="#ffffff" stroke="#2d3142" stroke-width="([\d.]+)"/.exec(s)[1];

test('icon art is a standalone square svg', () => {
  const s = padIconSvg('dualsense', full);
  assert.match(s, /^<svg xmlns="http:\/\/www.w3.org\/2000\/svg"/);
  const [, , w, h] = viewBox(s);
  assert.equal(w, h);
  assert.doesNotMatch(s, /opacity="\.28"/, 'no drop shadow');
});

test('glyphs:false drops every label and symbol', () => {
  assert.match(padIconSvg('dualsense', full), /stroke-linecap="round"/);
  const s = padIconSvg('dualsense', { ...full, glyphs: false });
  assert.doesNotMatch(s, /<text/);
  assert.doesNotMatch(s, /stroke-linecap="round"/);
});

test('small:false drops the small buttons', () => {
  assert.match(padIconSvg('dualsense', full), /#ffcf2e/);
  assert.doesNotMatch(padIconSvg('dualsense', { ...full, small: false }), /#ffcf2e/);
});

test('strokeScale multiplies the outline', () => {
  assert.equal(bodyStroke(padIconSvg('dualsense', full)), '5');
  assert.equal(bodyStroke(padIconSvg('dualsense', { ...full, strokeScale: 2 })), '10');
});

test('grey wraps the art in a desaturating, faded group', () => {
  assert.doesNotMatch(padIconSvg('dualsense', full), /saturate/);
  const s = padIconSvg('dualsense', { ...full, grey: true });
  assert.match(s, /<feColorMatrix type="saturate" values="0"\/>/);
  assert.match(s, /<g filter="url\(#grey\)" opacity="\.55">/);
});

test('board drawing is unchanged by the icon export', () => {
  assert.match(padSvg('dualsense'), /^<svg class="pad"/);
});

// Reskin Q5: the board pad is MapleStory line art (navy ink, slate keys, no drop shadow, no colour);
// the app / tray icons keep their colours.
test('board pad is line art, icons keep colour', () => {
  const b = padSvg('dualsense');
  assert.doesNotMatch(b, /opacity="\.28"/);
  assert.match(b, /stroke="#2d4359"/);
  assert.doesNotMatch(b, /#a8587a|#b0405f|#ffcf2e/);
  assert.match(padIconSvg('dualsense', { glyphs: true, small: true }), /#a8587a/);
  assert.doesNotMatch(padIconSvg('dualsense', { glyphs: true, small: true }), /#2d4359/);
});

test('minimal drops face buttons, d-pad keys and stick tops', () => {
  const s = padIconSvg('dualsense', { ...full, glyphs: false, small: false, minimal: true });
  assert.doesNotMatch(s, /#6c7ba0/, 'stick tops dropped');
  assert.equal((s.match(/fill="#a8587a"/g) || []).length, 2, 'only the two bumpers keep the face colour');
  assert.equal((s.match(/fill="#4b5a7c"/g) || []).length, 2, 'only the two stick caps stay');
});
