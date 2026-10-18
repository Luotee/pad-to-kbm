// The key-binding board: one 980×640 screen scaled to the window (spec 3).

import { state, init, onRender } from './board/store.js';
import { render, outputMode, boardLocked } from './board/view.js';
import { wire, selectSlot } from './board/events.js';
import { resetInk } from './board/tags.js';
import { startLive } from './board/live.js';
import { startMacros } from './board/macros.js';
import { startSettings } from './board/settings.js';
import { startCalibration } from './board/calibration.js';
import { startVigem, maybeProbe } from './board/vigem.js';
import { fitKeyLabels } from './board/palette.js';

const board = document.getElementById('board');

const band = document.getElementById('hband');

function fit() {
  // Wider than 980:640 → centred with side margins; taller → pinned to the top
  // (an empty header-coloured block above the header looks broken).
  const k = Math.min(innerWidth / 980, innerHeight / 640), ty = 0;
  board.style.transform = `translate(${(innerWidth - 980 * k) / 2}px, ${ty}px) scale(${k})`;
  // The header colour runs the full window width, flush with the board's header.
  band.style.height = (ty + 52 * k) + 'px';
}

function paint() {
  // 關 and 手把 / Xbox dim and lock the board (v3.2.1); the header stays live.
  const lock = boardLocked(state);
  board.className = 'mock' + (outputMode(state, state.pad) !== 'kbm' ? ' off' : '') + (lock ? ' lock' : '');
  board.innerHTML = render(state);
  fitKeyLabels(board);
  maybeProbe();
  // A slot waiting for a key, or a picked tile, ends when the board locks.
  if (lock && (state.sel !== null || state.picked)) queueMicrotask(() => selectSlot(null));
}

onRender(paint);
await init();
wire(board);
await startLive();
startMacros(board);
await startSettings(board);
startCalibration();
startVigem();
paint();
fit();
addEventListener('resize', fit);
// Tag ink boxes are measured with the pixel font; re-measure once it loaded.
if (document.fonts) document.fonts.ready.then(() => { resetInk(); paint(); });
