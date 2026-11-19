import test from 'node:test';
import assert from 'node:assert/strict';
import { powerOffFrom } from '../board/model.js';

// v3.10.0: controller-status.power_off lists the connected pads whose ⏻ works (the Xbox Nano only while its
// companion channel is ready).
test('power_off_lists_the_pads', () => {
  assert.deepEqual([...powerOffFrom({ power_off: ['dualsense', 'xbox_nano'] })], ['dualsense', 'xbox_nano']);
});

test('power_off_missing_is_empty', () => {
  assert.equal(powerOffFrom({}).size, 0);
  assert.equal(powerOffFrom(null).size, 0);
  assert.equal(powerOffFrom({ power_off: 'xbox_nano' }).size, 0, 'not a list');
});
