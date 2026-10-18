// Slot corner tags (spec 4.2), copied from the approved mockup.

export function esc(s) { return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'); }

export const TAGTXT = { 'PS:tri': '三角', 'PS:sq': '方塊', 'PS:ring': '圓圈', 'PS:x': '叉叉', 'S:up': '搖桿上', 'S:left': '搖桿左', 'S:right': '搖桿右', 'S:down': '搖桿下', 'S:press': '搖桿按下', 'T:tl': '觸控板左上', 'T:tr': '觸控板右上', 'T:bl': '觸控板左下', 'T:br': '觸控板右下' };
// The four PlayStation symbols are drawn, not typed: the pixel font has △ ○ □ but not the cross (U+2715),
// which fell back to a system font and looked unlike the other three.
var PSSYM = {
  tri: '<path d="M5,1.2 L9,8.4 L1,8.4 Z" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>',
  ring: '<circle cx="5" cy="5" r="3.7" fill="none" stroke="currentColor" stroke-width="1.4"/>',
  x: '<path d="M1.6,1.6 L8.4,8.4 M8.4,1.6 L1.6,8.4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>',
  sq: '<rect x="1.4" y="1.4" width="7.2" height="7.2" fill="none" stroke="currentColor" stroke-width="1.4"/>'
};
// Tags share one geometry: ink exactly 8 px tall starting at the tag's top-left. Text uses the pixel font
// at 11 px, whose capitals (and ascenders) are 8 px above the baseline, so baseline y = 8 puts ink at 0..8.
var PS8 = {
  tri: '<path d="M4,0.75 L7.5,7.35 L0.5,7.35 Z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>',
  ring: '<circle cx="4" cy="4" r="3.35" fill="none" stroke="currentColor" stroke-width="1.3"/>',
  x: '<path d="M0.9,0.9 L7.1,7.1 M7.1,0.9 L0.9,7.1" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>',
  sq: '<rect x="0.65" y="0.65" width="6.7" height="6.7" fill="none" stroke="currentColor" stroke-width="1.3"/>'
};
// The d-pad tags (spec 4.2, reskin v3.3.0): a solid 8px pixel triangle, turned on the pixel grid.
var TRI = '<path d="M3 2h2v1h1v1h1v1h1v1h-8v-1h1v-1h1v-1h1z" fill="currentColor"/>';
var DPAD = { '↑': 0, '→': 90, '↓': 180, '←': 270 };
export function tagSvg(t) {
  if (t in DPAD) return '<svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><g transform="rotate(' + DPAD[t] + ' 4 4)">' + TRI + '</g></svg>';
  if (t.indexOf('PS:') === 0) return '<svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">' + PS8[t.slice(3)] + '</svg>';
  if (t.indexOf('S:') === 0) {
    var p = { up: [4, 1.9], down: [4, 6.1], left: [1.9, 4], right: [6.1, 4], press: [4, 4] }[t.slice(2)];
    return '<svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><circle cx="4" cy="4" r="3.4" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="' + p[0] + '" cy="' + p[1] + '" r="1.4" fill="currentColor"/></svg>';
  }
  if (t.indexOf('T:') === 0) {
    var q = { tl: [0, 0], tr: [1, 0], bl: [0, 1], br: [1, 1] }[t.slice(2)];
    return '<svg width="12" height="8" viewBox="0 0 12 8" aria-hidden="true"><rect x="0.6" y="0.6" width="10.8" height="6.8" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.2"/><circle cx="' + (3.1 + q[0] * 5.8) + '" cy="' + (2.9 + q[1] * 2.2) + '" r="1.35" fill="currentColor"/></svg>';
  }
  var o = inkOff(t);
  return '<svg width="1" height="8" aria-hidden="true"><text x="' + o[0] + '" y="' + o[1] + '" font-size="11" font-family="Cubic11,Microsoft JhengHei,sans-serif" fill="currentColor">' + esc(t) + '</text></svg>';
}
// Glyphs carry their own side bearings (an arrow starts 1-2 px right of a letter), so each text tag is
// shifted by its measured ink box: the ink then starts at the same corner as the icon tags.
let INK = {};
export function inkOff(t) {
  if (INK[t]) return INK[t];
  var k = 4, c = document.createElement('canvas'); c.width = 80 * k; c.height = 24 * k;
  var x = c.getContext('2d'); x.scale(k, k); x.font = '11px Cubic11'; x.fillStyle = '#000'; x.fillText(t, 10, 16);
  var d = x.getImageData(0, 0, c.width, c.height).data, minX = 1e9, minY = 1e9;
  for (var yy = 0; yy < c.height; yy++) for (var xx = 0; xx < c.width; xx++) if (d[(yy * c.width + xx) * 4 + 3] > 110) { if (xx < minX) minX = xx; if (yy < minY) minY = yy; }
  var r = minX > 1e8 ? [0, 8] : [+(10 - minX / k).toFixed(2), +(16 - minY / k).toFixed(2)];
  if (document.fonts && document.fonts.status === 'loaded') INK[t] = r;
  return r;
}

// Forget measured ink boxes (call after document.fonts.ready).
export function resetInk() { INK = {}; }
