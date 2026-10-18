// icon-art.mjs — the app and tray icon (reskin Q25 4 甲): a Famicom / NES style pad drawn in the
// reference's emoticon manner (black outline, cream body with a white highlight and warm shade).
// Each ICO size is a whole multiple of a hand-placed grid, never a resample: 16 / 24 / 32 are drawn
// as is, 48 is the 16 grid x3, 256 the 32 grid x8. Read by scripts/build-icons.mjs.
import { pixelSvg } from '../rust/web/board/pixel.js';

// o outline, h highlight, f body, z shade, t face plate, d dark (D-pad, select / start), w labels, r A / B.
export const ART = {
  16: [
    '................',
    '................',
    '................',
    '................',
    '.oooooooooooooo.',
    'ohhhhhhhhhhhhhzo',
    'ohtdttttttttttzo',
    'ohdddtttttrrtrro',
    'ohtdttddtdrrtrro',
    'ohttttttttttttzo',
    'ohffffffffffffzo',
    'ozzzzzzzzzzzzzzo',
    '.oooooooooooooo.',
    '................',
    '................',
    '................',
  ],
  24: [
    '........................',
    '........................',
    '........................',
    '........................',
    '........................',
    '........................',
    '........................',
    '.oooooooooooooooooooooo.',
    'ohhhhhhhhhhhhhhhhhhhhhzo',
    'ohttttttttttttttttttttzo',
    'ohtttdttttttttttttttttzo',
    'ohtttdtttwwtwwttrtttrtzo',
    'ohtdddddtttttttrrrtrrrzo',
    'ohtttdtttddtddttrtttrtzo',
    'ohtttdttttttttttttttttzo',
    'ohttttttttttttttttttttzo',
    'ozzzzzzzzzzzzzzzzzzzzzzo',
    '.oooooooooooooooooooooo.',
    '........................',
    '........................',
    '........................',
    '........................',
    '........................',
    '........................',
  ],
  32: [
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '..oooooooooooooooooooooooooooo..',
    '.ohhhhhhhhhhhhhhhhhhhhhhhhhhhzo.',
    '.ohffffffffffffffffffffffffffzo.',
    '.ohttttttttttttttttttttttttttzo.',
    '.ohttttddttttttttttttttttttttzo.',
    '.ohttttddtttttttttttrrtttrrttzo.',
    '.ohttddddddtwwtwwttrrrrtrrrrtzo.',
    '.ohttddddddttttttttrrrrtrrrrtzo.',
    '.ohttttddtttddtddtttrrtttrrttzo.',
    '.ohttttddttttttttttttttttttttzo.',
    '.ohttttttttttttttttttttttttttzo.',
    '.ohffffffffffffffffffffffffffzo.',
    '.ohffffffffffffffffffffffffffzo.',
    '.ozzzzzzzzzzzzzzzzzzzzzzzzzzzzo.',
    '..oooooooooooooooooooooooooooo..',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
    '................................',
  ],
};
const CREAM = { o: '#100804', h: '#ffffff', f: '#ffefdc', z: '#efd0b0', t: '#e6cdb0', d: '#100804', w: '#ffffff', r: '#e0424f' };
const SOURCE = { 16: [16, 1], 24: [24, 1], 32: [32, 1], 48: [16, 3], 256: [32, 8] };

const grey = c => {
  const n = parseInt(c.slice(1), 16), l = Math.round(0.3 * (n >> 16) + 0.59 * ((n >> 8) & 255) + 0.11 * (n & 255));
  return '#' + l.toString(16).padStart(2, '0').repeat(3);
};

// grey: the tray icon without a controller (desaturated, faded).
export function appIconSvg(size, o = {}) {
  if (!SOURCE[size]) throw new Error('appIconSvg: no art for ' + size + ' px');
  const [grid, k] = SOURCE[size];
  const fills = Object.fromEntries(Object.entries(CREAM).map(([ch, c]) => [ch, o.grey ? grey(c) : c]));
  const s = pixelSvg(ART[grid], fills, k).replace('<svg ', '<svg xmlns="http://www.w3.org/2000/svg" ');
  return o.grey ? s.replace(/(<svg [^>]*>)(.*)(<\/svg>)$/, '$1<g opacity=".55">$2</g>$3') : s;
}
