// The pad battery (v3.8.0): a 13x7 pixel battery in the pad tab instead of the green lamp, a breathing
// bolt over it while charging, and the grey caption plate on hover. Pixel art from the plan's
// charge_demo.py (BODY, BOLT, ring()). Pure: payload in, markup out.

import { withRing } from './pixel.js';

const OUTER_IN = '#2d4359';                 // the battery's dark inside and the bolt's ring
const BAR = { 3: '#7ee05a', 2: '#ffd23f', 1: '#ff5a4a', 0: '#ff5a4a' };
const BARX = [2, 5, 8];
const BOLT = ['...#', '..##', '.##.', '####', '.##.', '##..', '#...'];
const BOLT_FILL = '#ffd23f';

// Same bars as the Rust `battery::level`: > 66 three, 34..=66 two, 11..=33 one, <= 10 none.
export function battLevel(pct) {
  return pct > 66 ? 3 : pct >= 34 ? 2 : pct >= 11 ? 1 : 0;
}

// The readable batteries of a controller-status payload (`battery: { slug: { pct, charging } }`); a payload
// without the field, or an entry without a number, has none.
export function batteryFrom(s) {
  const out = {};
  const b = s && s.battery;
  if (!b || typeof b !== 'object') return out;
  for (const [pad, e] of Object.entries(b)) {
    if (e && Number.isFinite(e.pct)) out[pad] = { pct: e.pct, charging: !!e.charging };
  }
  return out;
}

const rect = (x, y, w, h, fill) => '<rect x="' + x + '" y="' + y + '" width="' + w + '" height="' + h + '" fill="' + fill + '"/>';

// One path per colour over a grid placed at (ox, oy), each horizontal run one rect (as pixel.js).
function runs(rows, ch, ox, oy) {
  let d = '';
  rows.forEach((row, y) => {
    for (let x = 0; x < row.length; x++) {
      if (row[x] !== ch) continue;
      let e = x;
      while (row[e + 1] === ch) e++;
      const n = e - x + 1;
      d += 'M' + (ox + x) + ' ' + (oy + y) + 'h' + n + 'v1h-' + n + 'z';
      x = e;
    }
  });
  return d;
}

// The 4x7 bolt at (4, 1) over its 1px dark ring (every empty 8-neighbour: x 3..8, y 0..8 of the 13x9 box),
// centred on the 12-wide battery body. CSS breathes the `.bolt` group.
export function boltSvg() {
  const g = withRing(BOLT);
  return '<g class="bolt"><path d="' + runs(g, 'r', 3, 0) + '" fill="' + OUTER_IN + '"/><path d="' + runs(g, '#', 3, 0) + '" fill="' + BOLT_FILL + '"/></g>';
}

// The 13x7 battery drawn at (0, 1) in a 13x9 box (rows 0 and 8 hold the bolt's ring), whole scales only.
export function battSvg(b, outline, scale = 1) {
  if (!Number.isInteger(scale) || scale < 1) throw new Error('battSvg: whole scales only');
  const lv = battLevel(b.pct);
  let s = '<g fill="' + outline + '"><rect x="0" y="1" width="12" height="1"/><rect x="0" y="7" width="12" height="1"/>' +
    '<rect x="0" y="1" width="1" height="7"/><rect x="11" y="1" width="1" height="7"/><rect x="12" y="3" width="1" height="3"/></g>' +
    rect(1, 2, 10, 5, OUTER_IN);
  for (let i = 0; i < lv; i++) s += rect(BARX[i], 3, 2, 3, BAR[lv]);
  if (b.charging) s += boltSvg();
  return '<svg width="' + 13 * scale + '" height="' + 9 * scale + '" viewBox="0 0 13 9" shape-rendering="crispEdges" aria-hidden="true">' + s + '</svg>';
}

// The hover plate: the battery doubled, then "80%" (Switch Pro: 0 / 25 / 50 / 75 / 100, never "約"),
// plus 充電中 while charging.
export function tipHTML(b) {
  return '<span class="btip">' + battSvg(b, '#ffffff', 2) + '<span>' + b.pct + '%' + (b.charging ? ' 充電中' : '') + '</span></span>';
}
