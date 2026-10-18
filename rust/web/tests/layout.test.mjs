import test from 'node:test';
import assert from 'node:assert/strict';
import { layoutFor, slotsOf, lint, routeHTML, groupSlots } from '../board/layout.js';

const MODES = [{ l: 'page', r: 'page' }, { l: 'key', r: 'key' }, { l: 'page', r: 'key' }];
const PADS = ['dualsense', 'ns_pro', 'ns2_pro'];

test('layouts_have_no_crossings_or_overlaps', () => {
  for (const pad of PADS) for (const modes of MODES) {
    const L = layoutFor(pad, modes);
    assert.deepEqual(lint(L), { cross: 0, through: 0, overlap: 0 }, `${pad} ${JSON.stringify(modes)}`);
  }
});

test('routes_are_orthogonal_with_long_enough_segments', () => {
  for (const pad of PADS) for (const modes of MODES) {
    for (const rt of layoutFor(pad, modes).routes) {
      for (let i = 0; i + 1 < rt.pts.length; i++) {
        const [a, b] = [rt.pts[i], rt.pts[i + 1]];
        assert.ok(a[0] === b[0] || a[1] === b[1], `${pad} ${rt.id} diagonal`);
        assert.ok(Math.abs(a[0] - b[0]) + Math.abs(a[1] - b[1]) >= 12, `${pad} ${rt.id} short segment`);
      }
    }
  }
});

test('slots_stay_on_the_stage_and_match_each_pad', () => {
  const keys = pad => slotsOf(layoutFor(pad, MODES[0])).map(s => s.key);
  for (const pad of PADS) for (const s of slotsOf(layoutFor(pad, MODES[0]))) {
    assert.ok(s.x >= 0 && s.x + 38 <= 980 && s.y >= 60 && s.y + 38 <= 440, `${pad} ${s.key} at ${s.x},${s.y}`);
  }
  const ds = keys('dualsense'), ns = keys('ns_pro'), ns2 = keys('ns2_pro');
  for (const k of ['25', '26', '27', '28', '4', '5', '6']) assert.ok(ds.includes(k), 'ds ' + k);
  for (const k of ['30', '31', '32']) assert.ok(ns2.includes(k) && !ns.includes(k) && !ds.includes(k), k);
  assert.ok(!ns.includes('25') && ns.includes('29'));
  for (const k of ['L11', 'L0', 'R11', 'R3']) assert.ok(ds.includes(k), 'page slot ' + k);
});

test('key_mode_folds_a_page_into_the_trigger_slot', () => {
  const k = slotsOf(layoutFor('ns_pro', { l: 'key', r: 'page' })).map(s => s.key);
  assert.ok(k.includes('23') && !k.includes('L11') && k.includes('R11') && !k.includes('24'));
  const tag = slotsOf(layoutFor('dualsense', { l: 'key', r: 'key' })).find(s => s.key === '24').tag;
  assert.equal(tag, 'R2');
});

test('every_route_is_one_svg_path', () => {
  const L = layoutFor('ns2_pro', MODES[0]);
  for (const rt of L.routes) {
    for (const hit of [false, true]) {
      const h = routeHTML(rt.pts, rt.r, rt.m, hit);
      // Reskin Q12: a lit line keeps its shape (one path, one end dot) and only changes colour (CSS).
      assert.equal((h.match(/<path/g) || []).length, 1, `${rt.id} hit=${hit}`);
      assert.equal((h.match(/<circle/g) || []).length, 1, `${rt.id} hit=${hit}`);
      assert.equal(h.startsWith('<g class="hit">'), hit, `${rt.id} hit=${hit}`);
      assert.ok(!h.includes('class="ln'), 'no div lines');
    }
  }
});

// Reskin (v3.3.0): a 4-slot group with no middle slot packs like the MapleStory quick slot —
// up and down meet in the centre column, every seam 2px.
test('four_slot_groups_pack_tight', () => {
  const g = layoutFor('dualsense', MODES[0]).groups.find(x => x.kind === 'diamond');
  const at = Object.fromEntries(groupSlots(g).map(s => [s.tag, [s.x - g.x, s.y - g.y]]));
  const pos = Object.values(at);
  assert.deepEqual(pos, [[-19, -39], [-59, -19], [21, -19], [-19, 1]]);
});

test('stick_cross_keeps_its_middle_slot', () => {
  const g = layoutFor('dualsense', MODES[0]).groups.find(x => x.kind === 'cross');
  const ys = groupSlots(g).map(s => s.y - g.y).sort((a, b) => a - b);
  assert.deepEqual([ys[0], ys[ys.length - 1]], [-59, 21]);
});

// Reskin Q18: the touchpad-cursor switch lives in the header, not on the pad art.
test('pad_art_has_no_switch_group', () => {
  for (const pad of PADS) assert.ok(!layoutFor(pad, MODES[0]).groups.some(g => g.kind === 'tpsw'), pad);
});
