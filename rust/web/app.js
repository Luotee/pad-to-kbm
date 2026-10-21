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
import { startLightbar } from './board/lightbar.js';
import { fitKeyLabels } from './board/palette.js';
import { chromeHTML, fitWindow } from './chrome.js';
import { appWindow } from './ipc.js';

const board = document.getElementById('board');
const win = document.getElementById('win');

// The drawn window (reskin v3.3.0): frame + title bar around the board. The title bar drags the window
// (no double-click maximise: the window keeps whole-pixel sizes, see chrome.js fitWindow).
win.insertAdjacentHTML('beforeend', chromeHTML());
const tbar = document.getElementById('wtbar');
tbar.addEventListener('mousedown', ev => {
  if (ev.button !== 0 || ev.target.closest('.wb')) return;
  appWindow()?.startDragging();
});
tbar.addEventListener('click', ev => {
  const b = ev.target.closest('[data-win]');
  if (!b) return;
  const w = appWindow();
  if (b.dataset.win === 'min') w?.minimize();
  else w?.close();   // CloseRequested ends the app and releases held keys (runtime.rs)
});

// Scale before the first frame (and before any await): the board layer is rastered at its first scale.
fit();
addEventListener('resize', fit);

function fit() {
  const f = fitWindow(innerWidth, innerHeight, devicePixelRatio || 1);
  win.style.transform = `translate(${f.x}px, ${f.y}px) scale(${f.k})`;
  document.getElementById('app').classList.toggle('margin', f.margin);
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
startLightbar(board);
paint();
// Tag ink boxes are measured with the pixel font; re-measure once it loaded.
if (document.fonts) document.fonts.ready.then(() => { resetInk(); paint(); });
