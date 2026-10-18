// Controller art (spec 4.4, "B 貼紙" sticker style with contour shoulders),
// copied from the approved mockup (keymap-board.html PADART / pad-styles.html).

function circ(cx, cy, r) { return 'M' + (cx - r) + ',' + cy + ' a' + r + ',' + r + ' 0 1,0 ' + 2 * r + ',0 a' + r + ',' + r + ' 0 1,0 ' + -2 * r + ',0 Z'; }
function rr(x, y, w, h, r) { return 'M' + (x + r) + ',' + y + ' h' + (w - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + r + ',' + r + ' v' + (h - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + -r + ',' + r + ' h' + -(w - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + -r + ',' + -r + ' v' + -(h - 2 * r) + ' a' + r + ',' + r + ' 0 0 1 ' + r + ',' + -r + ' Z'; }
function cross(x, y, a, b) { return 'M' + (x - a) + ',' + (y - b) + ' h' + 2 * a + ' v' + (b - a) + ' h' + (b - a) + ' v' + 2 * a + ' h' + -(b - a) + ' v' + (b - a) + ' h' + -2 * a + ' v' + -(b - a) + ' h' + -(b - a) + ' v' + -2 * a + ' h' + (b - a) + ' Z'; }
var BODY_NS = 'M60,40 C90,28 210,28 240,40 C268,50 285,80 290,120 C296,170 280,205 250,205 C228,205 214,186 200,170 L100,170 C86,186 72,205 50,205 C20,205 4,170 10,120 C15,80 32,50 60,40 Z';
var BODY_DS = 'M54,40 C86,30 214,30 246,40 C272,50 286,84 291,124 C297,176 281,206 253,206 C230,206 216,186 204,168 L96,168 C84,186 70,206 47,206 C19,206 3,176 9,124 C14,84 28,50 54,40 Z';
var FACEPOS = [[226, 62], [246, 82], [226, 102], [206, 82]];
var MIR = 'translate(300,0) scale(-1,1)';

// ---- shoulder designs (left side; the right side is mirrored) ----
// back: parts drawn before the body; front: after the body. Anchors: [x, y, from]. Labels: [x, y, rotate].
var SHOULDERS = [
  {
    id: 'contour', name: '貼著機身弧線', tag: '最像實機',
    note: 'L／R 是一條沿著手把上緣彎下來的粗帶，扳機從它後面露出一截，像從正面看真的手把。',
    back: ['M32,62 C36,36 56,18 86,14 L97,13 Q104,12.5 104,19 L104,32 L60,40 C46,45 38,53 35,64 Z'],
    front: ['M19,84 C22,56 42,37 70,32 L98,29.5 Q104,29 104,35.5 Q104,42 98,42.5 L72,45 C53,49 39,63 34,86 Q33,92 26.5,91 Q18.5,90 19,84 Z'],
    trigA: [74, 22, 'u'], bumpA: [27, 72, 'l'],
    trigL: [85, 22, 0], bumpL: [83, 37.2, -5]
  }
];

function partD(p) { return typeof p === 'string' ? { d: p } : p; }
function model(pad, sh) {
  var ds = pad === 'dualsense', ns2 = pad === 'ns2_pro', P = [], Gl = [];
  function add(d, role, tf) { P.push({ d: d, role: role, tf: tf || '' }); }
  function both(p, role) { p = partD(p); add(p.d, role, p.tf); add(p.d, role, MIR + (p.tf ? ' ' + p.tf : '')); }
  if (ns2) { add(rr(66, 182, 24, 20, 6), 'back'); add(rr(210, 182, 24, 20, 6), 'back'); }
  sh.back.forEach(function (p) { both(p, 'trig'); });
  add(ds ? BODY_DS : BODY_NS, 'body');
  sh.front.forEach(function (p, i) { both(p, sh.frontRoles ? sh.frontRoles[i] : 'bump'); });
  var tl = sh.trigL, bl = sh.bumpL;
  Gl.push({ kind: 'text', t: ds ? 'L2' : 'ZL', x: tl[0], y: tl[1], rot: tl[2], role: 'trigT' }, { kind: 'text', t: ds ? 'R2' : 'ZR', x: 300 - tl[0], y: tl[1], rot: -tl[2], role: 'trigT' },
    { kind: 'text', t: ds ? 'L1' : 'L', x: bl[0], y: bl[1], rot: bl[2], role: 'bumpT' }, { kind: 'text', t: ds ? 'R1' : 'R', x: 300 - bl[0], y: bl[1], rot: -bl[2], role: 'bumpT' });
  function stick(x, y) { add(circ(x, y, 21), 'well'); add(circ(x, y, 14), 'cap'); add(circ(x, y, 9), 'capTop'); }
  if (ds) {
    add(rr(108, 40, 84, 58, 8), 'tpad');
    add(rr(92, 53, 8, 14, 4), 'small'); add(rr(200, 53, 8, 14, 4), 'small');
    [[76, 72], [88, 84], [76, 96], [64, 84]].forEach(function (q, i) { add(i % 2 ? rr(q[0] - 6, q[1] - 5, 12, 10, 3) : rr(q[0] - 5, q[1] - 6, 10, 12, 3), 'dkey'); });
    stick(116, 136); stick(196, 136);
    add(circ(151, 120, 7), 'small'); Gl.push({ kind: 'ps', x: 151, y: 120, role: 'smallT' });
  } else {
    add(cross(116, 136, 6, 15), 'dcross');
    stick(80, 82); stick(196, 136);
    add(rr(105, 56, 14, 5, 2.5), 'small');
    add('M181,55.5 h5 v-5 h4 v5 h5 v5 h-5 v5 h-4 v-5 h-5 Z', 'small');
    add(rr(120, 80, 12, 12, 3), 'small'); Gl.push({ kind: 'dot', x: 126, y: 86, role: 'smallT' });
    add(circ(174, 86, 7), 'small'); Gl.push({ kind: 'home', x: 174, y: 86, role: 'smallT' });
    if (ns2) { add(circ(151, 106, 6), 'small'); Gl.push({ kind: 'c', x: 151, y: 106, role: 'smallT' }); }
  }
  var sym = ds ? ['tri', 'ring', 'cross', 'sq'] : ['X', 'A', 'B', 'Y'];  // a 1-letter id would be drawn as text
  FACEPOS.forEach(function (q, i) { add(circ(q[0], q[1], 8), 'face'); Gl.push({ kind: sym[i].length === 1 ? 'text' : sym[i], t: sym[i], x: q[0], y: q[1], role: 'faceT' }); });
  return { parts: P, glyphs: Gl };
}

function pixGlyph(rows, cx, cy, c) {
  var w = rows[0].length, h = rows.length, x0 = cx - w / 2, y0 = cy - h / 2, d = '';
  rows.forEach(function (r, yy) { for (var xx = 0; xx < w; xx++) if (r[xx] === '#') d += 'M' + (x0 + xx) + ',' + (y0 + yy) + 'h1v1h-1Z'; });
  return '<path d="' + d + '" fill="' + c + '" shape-rendering="crispEdges"/>';
}
// Labels on the controller are drawn as stroke letters (cap height 7 units): text glyphs from the pixel
// font are not centred on their ink and blur off the pixel grid, so A/B/X/Y and L/R read off-centre.
var VL = {
  A: [6, 'M0,7 L3,0 L6,7 M1.3,4.4 H4.7'], B: [5, 'M0,0 V7 H3 Q5,7 5,5.1 Q5,3.5 3,3.5 H0 M0,0 H2.7 Q4.6,0 4.6,1.75 Q4.6,3.5 2.7,3.5'],
  X: [5, 'M0,0 L5,7 M5,0 L0,7'], Y: [6, 'M0,0 L3,3.4 L6,0 M3,3.4 V7'], L: [4, 'M0,0 V7 H4'], R: [5, 'M0,7 V0 H3 Q5,0 5,2 Q5,4 3,4 H0 M3,4 L5,7'],
  Z: [5, 'M0,0 H5 L0,7 H5'], P: [5, 'M0,7 V0 H3 Q5,0 5,2 Q5,4 3,4 H0'], S: [5, 'M5,1 Q4.4,0 2.6,0 Q0.2,0 0.2,1.8 Q0.2,3.2 2.5,3.5 Q5,3.8 5,5.3 Q5,7 2.5,7 Q0.6,7 0,6'],
  '1': [3, 'M0,1.5 L1.8,0 V7 M0,7 H3.6'], '2': [5, 'M0,1.8 Q0,0 2.5,0 Q5,0 5,2 Q5,3.4 3.6,4.4 L0,7 H5']
};
function vtext(t, cx, cy, cap, c, sw, rot) {
  var k = cap / 7, gap = 1.8, w = -gap, d = '';
  t.split('').forEach(function (ch) { w += VL[ch][0] + gap; });
  var x = 0;
  t.split('').forEach(function (ch) { d += '<path d="' + VL[ch][1] + '" transform="translate(' + x + ' 0)"/>'; x += VL[ch][0] + gap; });
  return '<g transform="' + (rot ? 'rotate(' + rot + ' ' + cx + ' ' + cy + ') ' : '') + 'translate(' + (cx - w * k / 2) + ' ' + (cy - cap / 2) + ') scale(' + k + ')" fill="none" stroke="' + c + '" stroke-width="' + (sw / k) + '" stroke-linecap="round" stroke-linejoin="round">' + d + '</g>';
}
function glyphSvg(g, c) {
  var x = g.x, y = g.y;
  if (g.kind === 'text' && /^[ABXYLRZ12]+$/.test(g.t)) return vtext(g.t, x, y, g.role === 'faceT' ? 7 : 6, c, g.role === 'faceT' ? 1.6 : 1.4, g.rot);
  if (g.kind === 'text') {
    var fs = g.role === 'faceT' ? 10 : 9;
    return '<text x="' + x + '" y="' + (y + fs * 0.36) + '"' + (g.rot ? ' transform="rotate(' + g.rot + ' ' + x + ' ' + y + ')"' : '') + ' text-anchor="middle" font-size="' + fs + '" font-family="Cubic11,Microsoft JhengHei,sans-serif" fill="' + c + '">' + g.t + '</text>';
  }
  if (g.kind === 'tri') return '<path d="M' + x + ',' + (y - 4.5) + ' L' + (x + 4.5) + ',' + (y + 3.5) + ' L' + (x - 4.5) + ',' + (y + 3.5) + ' Z" fill="none" stroke="' + c + '" stroke-width="1.6" stroke-linejoin="round"/>';
  if (g.kind === 'ring') return '<circle cx="' + x + '" cy="' + y + '" r="4" fill="none" stroke="' + c + '" stroke-width="1.6"/>';
  if (g.kind === 'cross') return '<path d="M' + (x - 3.6) + ',' + (y - 3.6) + ' L' + (x + 3.6) + ',' + (y + 3.6) + ' M' + (x + 3.6) + ',' + (y - 3.6) + ' L' + (x - 3.6) + ',' + (y + 3.6) + '" stroke="' + c + '" stroke-width="1.6" stroke-linecap="round"/>';
  if (g.kind === 'sq') return '<rect x="' + (x - 3.6) + '" y="' + (y - 3.6) + '" width="7.2" height="7.2" fill="none" stroke="' + c + '" stroke-width="1.6"/>';
  if (g.kind === 'dot') return '<circle cx="' + x + '" cy="' + y + '" r="2.6" fill="' + c + '"/>';
  // Home = pixel house (H3), PS = the letters PS in pixels (P4), user picks 2026-10-13. Both are 8 px
  // bitmaps on whole pixels, centred on the r7 button with clearance from its rim, 1 px strokes like the font.
  if (g.kind === 'home') return '<g transform="translate(' + x + ' ' + y + ')" fill="none" stroke="' + c + '" stroke-width="1.9" stroke-linejoin="round" stroke-linecap="round"><path d="M-3.6,-0.4 L0,-3.8 L3.6,-0.4"/><path d="M-2.4,-1.2 V3.4 H2.4 V-1.2"/><rect x="-0.8" y="0.9" width="1.6" height="2.5" fill="' + c + '" stroke="none"/></g>';
  if (g.kind === 'c') return '<path d="M' + (x + 2.2) + ',' + (y - 2.4) + ' A3.2,3.2 0 1 0 ' + (x + 2.2) + ',' + (y + 2.4) + '" fill="none" stroke="' + c + '" stroke-width="1.5"/>';
  if (g.kind === 'ps') return vtext('PS', x, y, 4.6, c, 1.5);  // smaller so it clears the r7 rim
  return '';
}

// Style B (sticker) palette.
var K = '#2d3142';
function fill(role) {
  if (role === 'body') return ['#ffffff', 5];
  if (role === 'tpad') return ['#e9ecf2', 3.5];
  if (role === 'trig') return ['#6e3a52', 3.5];
  if (role === 'bump') return ['#a8587a', 3.5];
  if (role === 'well') return ['#d9dde6', 3.5];
  if (role === 'cap') return ['#4b5a7c', 3];
  if (role === 'capTop') return ['#6c7ba0', 0];
  if (role === 'dcross' || role === 'dkey') return ['#4b5a7c', 3];
  if (role === 'back') return ['#6e3a52', 3];
  if (role === 'face') return ['#a8587a', 3];
  return ['#ffcf2e', 2.5];
}
function glyphColor(g) { return g.role === 'smallT' ? K : '#ffffff'; }
// The board pad is line work like the MapleStory controller (v3.3.0): navy ink, the body shows the panel
// through, slate keys, no colour, no drop shadow. The app / tray icons keep the coloured art.
var INK = '#2d4359';
var LINE = { body: '#b2bac0', tpad: '#b2bac0', trig: '#7b8188', back: '#7b8188', bump: '#8f969d', well: '#b2bac0', cap: '#79838f', capTop: '#8a939d', dcross: '#79838f', dkey: '#79838f', face: '#79838f' };
function boardFill(role) { var f = fill(role); return [LINE[role] || '#79838f', f[1] ? Math.min(f[1], 3) : 0]; }

function padArt(pad, sh) {
  var M = model(pad, sh), h = '<svg viewBox="-30 -30 360 270" aria-hidden="true">';
  M.parts.forEach(function (p) {
    var f = boardFill(p.role);
    h += '<path d="' + p.d + '"' + (p.tf ? ' transform="' + p.tf + '"' : '') + ' fill="' + f[0] + '"' + (f[1] ? ' stroke="' + INK + '" stroke-width="' + f[1] + '" stroke-linejoin="round"' : '') + '/>';
  });
  h += M.glyphs.map(function (g) { return glyphSvg(g, g.role === 'smallT' ? INK : '#ffffff'); }).join('');
  return h + '<g class="ancg"></g></svg>';
}


// The pad drawing, positioned on the 980×640 board.
export function padSvg(pad) {
  return padArt(pad, SHOULDERS[0]).replace('<svg viewBox="-30 -30 360 270" aria-hidden="true">',
    '<svg class="pad" style="left:340px;top:150px;overflow:visible" width="300" height="210" viewBox="0 0 300 210" aria-hidden="true">');
}

// The pad drawing as an app / tray icon (scripts/build-icons.mjs): square viewBox on the art's bounds,
// no drop shadow. Small sizes drop the labels (glyphs), the small buttons and thicken every outline;
// minimal (16 / 24 px) also drops the face buttons, d-pad keys and stick tops.
// grey: desaturated and faded (tray, no controller connected).
export function padIconSvg(pad, opts) {
  var o = opts || {}, k = o.strokeScale || 1, M = model(pad, SHOULDERS[0]);
  var sw = 5 * k, x0 = 3 - sw / 2, x1 = 297 + sw / 2, y0 = 12.5 - sw / 2, y1 = 206 + sw / 2;
  var side = Math.max(x1 - x0, y1 - y0) * 1.04, cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
  var r = function (v) { return Math.round(v * 100) / 100; };
  var h = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="' + [r(cx - side / 2), r(cy - side / 2), r(side), r(side)].join(' ') + '">';
  if (o.grey) h += '<filter id="grey"><feColorMatrix type="saturate" values="0"/></filter><g filter="url(#grey)" opacity=".55">';
  M.parts.forEach(function (p) {
    if (!o.small && p.role === 'small') return;
    if (o.minimal && (p.role === 'face' || p.role === 'dkey' || p.role === 'capTop')) return;
    var f = fill(p.role);
    h += '<path d="' + p.d + '"' + (p.tf ? ' transform="' + p.tf + '"' : '') + ' fill="' + f[0] + '"' + (f[1] ? ' stroke="' + K + '" stroke-width="' + r(f[1] * k) + '" stroke-linejoin="round"' : '') + '/>';
  });
  if (o.glyphs) h += M.glyphs.filter(function (g) { return o.small || g.role !== 'smallT'; }).map(function (g) { return glyphSvg(g, glyphColor(g)); }).join('');
  if (o.grey) h += '</g>';
  return h + '</svg>';
}
