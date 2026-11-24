import test from 'node:test';
import assert from 'node:assert/strict';
import { nativeFrom } from '../board/model.js';

// v3.11.0: controller-status.native_xinput lists the connected pads Microsoft's Xbox driver owns (the Nano under
// its driver). The board shows no ViGEmBus / HidHide prompt for those.
test('native_lists_the_pads', () => {
  assert.deepEqual([...nativeFrom({ native_xinput: ['xbox_nano'] })], ['xbox_nano']);
});

test('native_missing_is_empty', () => {
  assert.equal(nativeFrom({}).size, 0);
  assert.equal(nativeFrom(null).size, 0);
  assert.equal(nativeFrom({ native_xinput: 'xbox_nano' }).size, 0, 'not a list');
});
