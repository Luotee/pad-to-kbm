import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  PADS, QUICK_IDS, slotRef, slotKey, boardBinds, entryForTile, keyToTile, tileToKey,
  MOUSE_TILE, slotLabel,
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
  assert.deepEqual(PADS, ['dualsense', 'ns_pro', 'ns2_pro']);
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
