import test from 'node:test';
import assert from 'node:assert/strict';
import { battLevel, battSvg, boltSvg, tipHTML, batteryFrom } from '../board/battery.js';

const count = (s, needle) => s.split(needle).length - 1;

// v3.8.0: the same bars as the Rust `battery::level` (> 66 three, 34..=66 two, 11..=33 one, <= 10 none).
test('batt_level_matches_the_rust_boundaries', () => {
  for (const [pct, lv] of [[100, 3], [67, 3], [66, 2], [34, 2], [33, 1], [11, 1], [10, 0], [0, 0]]) assert.equal(battLevel(pct), lv, 'pct ' + pct);
});

test('batt_svg_bar_colour_is_the_level', () => {
  assert.equal(count(battSvg({ pct: 80, charging: false }, '#ffffff'), 'fill="#7ee05a"'), 3);
  assert.equal(count(battSvg({ pct: 50, charging: false }, '#ffffff'), 'fill="#ffd23f"'), 2);
  assert.equal(count(battSvg({ pct: 20, charging: false }, '#ffffff'), 'fill="#ff5a4a"'), 1);
  const empty = battSvg({ pct: 5, charging: false }, '#ffffff');
  for (const c of ['#7ee05a', '#ffd23f', '#ff5a4a']) assert.equal(count(empty, c), 0, c);
});

test('batt_svg_is_a_13x9_pixel_box_with_the_given_outline', () => {
  const s = battSvg({ pct: 80, charging: false }, '#ffffff');
  assert.match(s, /^<svg width="13" height="9" viewBox="0 0 13 9" shape-rendering="crispEdges" aria-hidden="true">/);
  assert.match(s, /fill="#ffffff"/);
  assert.match(s, /fill="#2d4359"/);
  assert.match(battSvg({ pct: 80 }, '#ffffff', 2), /^<svg width="26" height="18" viewBox="0 0 13 9"/);
  assert.throws(() => battSvg({ pct: 80 }, '#ffffff', 1.5));
});

test('batt_svg_has_the_bolt_only_while_charging', () => {
  assert.equal(count(battSvg({ pct: 60, charging: true }, '#ffffff'), 'class="bolt"'), 1);
  assert.equal(count(battSvg({ pct: 60, charging: false }, '#ffffff'), 'class="bolt"'), 0);
});

// charge_demo.py: the 4x7 bolt at (4, 1) over its 1px dark ring (every empty 8-neighbour, so the ring
// reaches rows 0 and 8 of the 13x9 box), ring first so the bolt paints on top.
test('bolt_is_the_charge_demo_pixels', () => {
  assert.equal(boltSvg(), '<g class="bolt">' +
    '<path d="M6 0h3v1h-3zM5 1h2v1h-2zM8 1h1v1h-1zM4 2h2v1h-2zM8 2h1v1h-1zM3 3h2v1h-2zM7 3h2v1h-2zM3 4h1v1h-1zM8 4h1v1h-1zM3 5h2v1h-2zM7 5h2v1h-2zM3 6h1v1h-1zM6 6h2v1h-2zM3 7h1v1h-1zM5 7h2v1h-2zM3 8h3v1h-3z" fill="#2d4359"/>' +
    '<path d="M7 1h1v1h-1zM6 2h2v1h-2zM5 3h2v1h-2zM4 4h4v1h-4zM5 5h2v1h-2zM4 6h2v1h-2zM4 7h1v1h-1z" fill="#ffd23f"/></g>');
});

test('tip_is_the_doubled_battery_and_the_percent', () => {
  const t = tipHTML({ pct: 75, charging: false });
  assert.match(t, /^<span class="btip">/);
  assert.match(t, /<svg width="26" height="18" viewBox="0 0 13 9"/);
  assert.equal(t.replace(/<[^>]*>/g, ''), '75%');
  assert.ok(!t.includes('約'));
  const c = tipHTML({ pct: 60, charging: true });
  assert.ok(c.includes('充電中'));
  assert.equal(c.replace(/<[^>]*>/g, ''), '60% 充電中');
});

test('battery_from_keeps_readable_entries_only', () => {
  assert.deepEqual(batteryFrom(null), {});
  assert.deepEqual(batteryFrom({ connected: true }), {});
  assert.deepEqual(batteryFrom({ battery: { dualsense: { pct: 80, charging: false }, ns_pro: { pct: 'x' }, ns2_pro: null } }),
    { dualsense: { pct: 80, charging: false } });
});
