import test from 'node:test';
import assert from 'node:assert/strict';
import { hsvHex, hexHsv, pickerIndex, hueCells, satCells, normHex, SPEEDS, SPEED_TXT, PRESETS } from '../board/lightcolor.js';

test('hsv_hex_primaries_and_white', () => {
  assert.equal(hsvHex(0, 1, 1), '#FF0000');
  assert.equal(hsvHex(120, 1, 1), '#00FF00');
  assert.equal(hsvHex(0, 0, 1), '#FFFFFF');
});

test('hex_hsv_reads_back_hue_and_saturation', () => {
  assert.deepEqual(hexHsv('#FF0000'), { h: 0, s: 1, v: 1 });
  assert.deepEqual(hexHsv('#FFFFFF'), { h: 0, s: 0, v: 1 });
});

test('picker_index_finds_the_cells', () => {
  assert.deepEqual(pickerIndex('#FFFFFF'), { hi: 0, si: 0 });
  assert.deepEqual(pickerIndex(hsvHex(225, 12 / 24, 1)), { hi: 15, si: 11 });
});

test('picker_strips_are_24_cells', () => {
  assert.equal(hueCells(23).length, 24);
  assert.equal(satCells(0).length, 24);
  assert.equal(hueCells(23)[0], '#FF0000');
  assert.equal(satCells(0)[23], '#FF0000');
});

test('norm_hex_takes_six_digits_with_or_without_hash', () => {
  assert.equal(normHex('2ea8ff'), '#2EA8FF');
  assert.equal(normHex('#2EA8FF'), '#2EA8FF');
  assert.equal(normHex('#2EA8F'), null);
  assert.equal(normHex('red'), null);
});

test('speeds_and_presets_follow_the_spec', () => {
  assert.deepEqual(SPEEDS, ['slow', 'medium', 'fast', 'hyper']);
  assert.deepEqual(SPEED_TXT, ['慢', '中', '快', '極快']);
  assert.deepEqual(PRESETS.map(p => p[1]), ['#FFFFFF', '#FF2A2A', '#FF7A00', '#2BD94A', '#2EA8FF', '#8A3CFF']);
});
