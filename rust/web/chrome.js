// The window chrome (reskin v3.3.0): Tauri draws no title bar (decorations: false), so the page draws
// the MapleStory KEY BINDINGS window around the board — steel frame with 45-degree cut corners, white
// title bar, minimise + close keys copied pixel by pixel from the reference. Pure markup; app.js wires
// the keys to the window (ipc.js appWindow).

export const W = 994, H = 669;          // the whole window at 1x: board 980x640 at (7, 22)
export const TITLE = 'PAD TO KBM';

// Reference close key, 12x12: black edge with cut corners, ten blue bands, and 2x2 blocks on a 3x3 grid
// (pale corners + centre read as an X, dark edges between). Minimise = a pale bottom bar.
const BANDS = ['#69c3f2', '#1f88bd', '#2a8fc6', '#3196c8', '#3c9fd0', '#45a8d6', '#47acd6', '#4bb2dc', '#5ac2f2', '#6bc2eb'];
const GRID = { close: ['P.P', '.P.', 'P.P'], min: ['...', '...', 'PPP'] };
function key(kind) {
  let r = '<path d="M1 0h10v1h-10zM1 11h10v1h-10zM0 1h1v10h-1zM11 1h1v10h-1z" fill="#000714"/>';
  BANDS.forEach((c, i) => { r += '<rect x="1" y="' + (i + 1) + '" width="10" height="1" fill="' + c + '"/>'; });
  GRID[kind].forEach((row, j) => [...row].forEach((ch, i) => {
    const fill = ch === 'P' ? '#c4fcff' : (kind === 'close' ? '#3a7fa8' : null);
    if (fill) r += '<rect x="' + (3 + 2 * i) + '" y="' + (3 + 2 * j) + '" width="2" height="2" fill="' + fill + '"/>';
  }));
  return '<svg width="12" height="12" viewBox="0 0 12 12" shape-rendering="crispEdges" aria-hidden="true">' + r + '</svg>';
}
export const BTN = { min: key('min'), close: key('close') };

// White outer line and dark steel line, both following the 4px cut corners.
function poly(d) {
  const c = 4;
  return [[c + d, d], [W - c - d, d], [W - d, c + d], [W - d, H - c - d], [W - c - d, H - d], [c + d, H - d], [d, H - c - d], [d, c + d]].map(q => q.join(',')).join(' ');
}
export const FRAME = '<svg width="' + W + '" height="' + H + '" shape-rendering="crispEdges" aria-hidden="true"><polygon points="' + poly(0.5) +
  '" fill="none" stroke="#f7ffff"/><polygon points="' + poly(1.5) + '" fill="none" stroke="#64768b"/></svg>';

export function chromeHTML() {
  return '<div id="wtbar"><span class="wt">' + TITLE + '</span><span class="wsp"></span>' +
    '<button type="button" class="wb" data-win="min" aria-label="縮小" title="縮小">' + BTN.min + '</button>' +
    '<button type="button" class="wb" data-win="close" aria-label="關閉" title="關閉">' + BTN.close + '</button></div>' +
    '<div id="wframe">' + FRAME + '</div>';
}

// How the W x H window is drawn into a viewport of iw x ih CSS pixels at devicePixelRatio dpr
// (reskin Q23): n device pixels per art pixel, n a whole number, so every pixel edge is sharp.
// Rust sizes the window to W*n x H*n physical pixels, so normally it fits exactly; a bigger
// viewport (maximised, snapped) centres it on whole device pixels with a steel margin; a viewport
// smaller than 1x falls back to a fractional scale (n = 0) rather than vanish.
export function fitWindow(iw, ih, dpr) {
  const n = Math.floor(Math.min(iw * dpr / W, ih * dpr / H) + 0.01);
  const k = n >= 1 ? n / dpr : Math.min(iw / W, ih / H);
  const snap = v => Math.round(Math.max(0, v) * dpr) / dpr;
  const x = snap((iw - W * k) / 2), y = snap((ih - H * k) / 2);
  return { n, k, x, y, margin: x > 0 || y > 0 };
}
