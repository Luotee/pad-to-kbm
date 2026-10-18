// Pixel art as SVG (reskin Q24): a grid of rows, one character per art pixel ('.' = empty), one path
// per colour, each horizontal run one rect. Drawn at whole-pixel scales only, so it stays as sharp as
// the pixel fonts once the window is scaled by a whole number (chrome.js fitWindow).

// fills: grid character -> fill colour, or 'iol' for the ring path that CSS colours (var(--iol)).
export function pixelSvg(rows, fills, scale = 1) {
  if (!Number.isInteger(scale) || scale < 1) throw new Error('pixelSvg: whole scales only');
  const w = rows[0].length, h = rows.length;
  let paths = '';
  for (const [ch, fill] of Object.entries(fills)) {
    let d = '';
    rows.forEach((row, y) => {
      for (let x = 0; x < w; x++) {
        if (row[x] !== ch) continue;
        let e = x;
        while (row[e + 1] === ch) e++;
        const n = e - x + 1;
        d += 'M' + x + ' ' + y + 'h' + n + 'v1h-' + n + 'z';
        x = e;
      }
    });
    if (d) paths += fill === 'iol' ? '<path class="iol" d="' + d + '"/>' : '<path d="' + d + '" fill="' + fill + '"/>';
  }
  return '<svg width="' + w * scale + '" height="' + h * scale + '" viewBox="0 0 ' + w + ' ' + h +
    '" shape-rendering="crispEdges" aria-hidden="true">' + paths + '</svg>';
}

// Pads the grid by one pixel and marks every empty 8-neighbour of '#' as 'r' (the ring).
export function withRing(rows) {
  const w = rows[0].length + 2, h = rows.length + 2;
  const ink = (x, y) => (rows[y - 1] || '')[x - 1] === '#';
  const out = [];
  for (let y = 0; y < h; y++) {
    let r = '';
    for (let x = 0; x < w; x++) {
      if (ink(x, y)) r += '#';
      else r += [-1, 0, 1].some(dx => [-1, 0, 1].some(dy => ink(x + dx, y + dy))) ? 'r' : '.';
    }
    out.push(r);
  }
  return out;
}
