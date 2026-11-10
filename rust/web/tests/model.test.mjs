import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  PADS, PAD_NAMES, QUICK_IDS, slotRef, slotKey, boardBinds, entryForTile, keyToTile, tileToKey, tileOfEntry, cursorGroup,
  MOUSE_TILE, slotLabel, slotOff,
} from '../board/model.js';
import { MAIN, NAV } from '../board/keys.js';

const example = JSON.parse(fs.readFileSync(fileURLToPath(new URL('../../config.example.json', import.meta.url)), 'utf8'));
const clone = v => JSON.parse(JSON.stringify(v));

test('slot_keys_round_trip', () => {
  for (let id = 0; id <= 32; id++) {
    assert.deepEqual(slotRef(String(id)), { page: 'base', id });
    assert.equal(slotKey('base', id), String(id));
  }
  for (const id of QUICK_IDS) {
    assert.deepEqual(slotRef('L' + id), { page: 'quick_l', id });
    assert.deepEqual(slotRef('R' + id), { page: 'quick_r', id });
    assert.equal(slotKey('quick_l', id), 'L' + id);
    assert.equal(slotKey('quick_r', id), 'R' + id);
  }
  assert.deepEqual(PADS, ['dualsense', 'ns_pro', 'ns2_pro', 'xbox_nano']);
  assert.equal(PAD_NAMES.xbox_nano, 'Xbox Nano');
});

test('board_binds_reads_all_three_tables', () => {
  const cfg = clone(example);
  cfg.pads.dualsense.quick_l['11'] = { label: 'x', type: 'key', value: 'F1' };
  cfg.pads.ns_pro.buttons['0'] = { label: 'x', type: 'macro', value: 'm' };
  const ds = boardBinds(cfg, 'dualsense');
  assert.deepEqual(ds, {
    '11': '↑', '12': '↓', '13': '←', '14': '→', '15': '↑', '16': '↓', '17': '←', '18': '→',
    '25': '左鍵', '26': '左鍵', '27': '左鍵', '28': '左鍵', 'L11': 'F1',
  });
  assert.equal(boardBinds(cfg, 'ns_pro')['0'], 'M:m');
  assert.equal(boardBinds(cfg, 'ns_pro')['25'], undefined);
  assert.deepEqual(boardBinds({ pads: {} }, 'ns2_pro'), {});
});

test('entry_for_tile_covers_key_mouse_macro_unbound', () => {
  assert.deepEqual(entryForTile('Q', 'Cross'), { label: 'Cross', type: 'key', value: 'q' });
  assert.deepEqual(entryForTile('↑', 'x'), { label: 'x', type: 'key', value: 'Up' });
  assert.deepEqual(entryForTile('下滾', 'x'), { label: 'x', type: 'mouse', value: 'wheel-down' });
  assert.deepEqual(entryForTile('M:連發', 'x'), { label: 'x', type: 'macro', value: '連發' });
  assert.deepEqual(entryForTile('', 'x'), { label: 'x', type: 'unbound' });
  assert.deepEqual(entryForTile(null, 'x'), { label: 'x', type: 'unbound' });
});

test('key_to_tile_normalizes_aliases', () => {
  const cases = {
    Return: 'Enter', enter: 'Enter', lctrl: 'LCtrl', Control: 'LCtrl', RCONTROL: 'RCtrl', shift: 'LShift',
    RShift: 'RShift', alt: 'LAlt', ralt: 'RAlt', cmd: 'Win', meta: 'Win', x: 'X', '7': '7', f11: 'F11',
    up: '↑', Left: '←', escape: 'Esc', del: 'Del', Delete: 'Del', pageup: 'PageUp', PageDown: 'PageDown',
    space: 'Space', tab: 'Tab', home: 'Home', END: 'End', backspace: 'Backspace', '/': '/', weird: 'weird',
  };
  for (const [k, v] of Object.entries(cases)) assert.equal(keyToTile(k), v, k);
});

// The key names config::parse_key accepts (rust/src/config.rs), lowercased.
const PARSE_KEY = new Set(['shift', 'lshift', 'rshift', 'control', 'ctrl', 'lcontrol', 'lctrl', 'rcontrol', 'rctrl',
  'alt', 'lalt', 'ralt', 'meta', 'win', 'cmd', 'left', 'right', 'up', 'down', 'space', 'enter', 'return', 'tab',
  'escape', 'esc', 'backspace', 'delete', 'del', 'home', 'end', 'pageup', 'pagedown',
  ...Array.from({ length: 12 }, (_, i) => 'f' + (i + 1))]);

test('tile_to_key_is_accepted_by_backend_names', () => {
  const tiles = [...MAIN, ...NAV].flat().filter(k => k[0] && k[2] !== 'dis').map(k => k[0]);
  assert.ok(tiles.length >= 75, String(tiles.length));
  for (const t of tiles) {
    const k = tileToKey(t);
    assert.ok([...k].length === 1 || PARSE_KEY.has(k.toLowerCase()), `${t} → ${k}`);
    assert.equal(keyToTile(k), t, `round trip ${t}`);
  }
});

test('mouse_tiles_and_labels', () => {
  assert.deepEqual(MOUSE_TILE, { left: '左鍵', right: '右鍵', middle: '中鍵', 'wheel-up': '上滾', 'wheel-down': '下滾' });
  assert.equal(slotLabel(example, 'dualsense', 'L11'), 'D-pad ↑');
  assert.equal(slotLabel(example, 'ns2_pro', '0'), 'Cross（叉叉）');
});

test('slotOff: only the Xbox Nano Share slot (29) is unavailable, with its title', () => {
  assert.equal(slotOff('xbox_nano', '29'), 'Xbox Nano 的 Share 鍵目前不會送給電腦');
  // Its other slots, and 29 on the Switch pads (Capture), stay usable.
  for (const k of ['0', '4', '5', '6', '23', '24', 'L11', 'R0']) assert.equal(slotOff('xbox_nano', k), '', k);
  for (const p of ['dualsense', 'ns_pro', 'ns2_pro']) assert.equal(slotOff(p, '29'), '', p);
});

test('游標 tile ↔ cursor entry (v3.9.0)', () => {
  assert.deepEqual(entryForTile('游標', 'L-stick ↑'), { label: 'L-stick ↑', type: 'cursor' });
  assert.equal(tileOfEntry({ label: 'x', type: 'cursor' }), '游標');
  const cfg = { pads: { dualsense: { buttons: { 19: { type: 'cursor' }, 20: { type: 'cursor' } }, quick_l: { 11: { type: 'cursor' } } } } };
  assert.deepEqual(boardBinds(cfg, 'dualsense'), { 19: '游標', 20: '游標', L11: '游標' });
});

test('cursorGroup: the four slots a 游標 covers, ↑ ↓ ← →', () => {
  assert.deepEqual(cursorGroup('17'), ['15', '16', '17', '18']);
  assert.deepEqual(cursorGroup(22), ['19', '20', '21', '22']);
  assert.deepEqual(cursorGroup('13'), ['11', '12', '13', '14']);
  assert.deepEqual(cursorGroup('R12'), ['R11', 'R12', 'R13', 'R14']);
  assert.equal(cursorGroup('3'), null);
  assert.equal(cursorGroup('L0'), null, "a quick page's face buttons are not a group");
  assert.equal(cursorGroup('7'), null, 'the stick press is not part of its stick');
});
