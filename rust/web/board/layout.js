// Board geometry (spec 4.2 / 4.3), copied from the approved mockup
// (keymap-board.html): groups of slots around the pad and orthogonal guide
// lines. 980×640 board units. The mockup's layout editor is not shipped.

// A 4-slot group with no middle slot packs tight, like the MapleStory quick slot: up and down
// meet in the centre column, every seam 2px (the plate under each slot shows through).
function diamond(cx, cy, keys, tags) {
  var pos = [[cx - 19, cy - 39], [cx - 59, cy - 19], [cx + 21, cy - 19], [cx - 19, cy + 1]];
  return keys.map(function (k, i) { return { key: k, tag: tags[i], x: pos[i][0], y: pos[i][1] }; });
}

function cross(cx, cy, ids) {
  var pos = [[cx - 19, cy - 59], [cx - 59, cy - 19], [cx + 21, cy - 19], [cx - 19, cy + 21]], tg = ['S:up', 'S:left', 'S:right', 'S:down'];
  return ids.slice(0, 4).map(function (k, i) { return { key: k, tag: tg[i], x: pos[i][0], y: pos[i][1] }; }).concat([{ key: ids[4], tag: 'S:press', x: cx - 19, y: cy - 19 }]);
}

function groupSlots(g) {
  if (g.kind === 'diamond') return diamond(g.x, g.y, g.keys, g.tags);
  if (g.kind === 'cross') return cross(g.x, g.y, g.keys);
  if (g.kind === 'pair') return diamond(g.x - 65, g.y, g.keys.slice(0, 4), ARROWS).concat(diamond(g.x + 65, g.y, g.keys.slice(4), g.tags));
  if (g.kind === 'quad') return [[-42, -42], [4, -42], [-42, 4], [4, 4]].map(function (o, i) { return { key: g.keys[i], tag: g.tags[i], x: g.x + o[0], y: g.y + o[1] }; });
  if (g.kind === 'slot') return [{ key: g.keys[0], tag: g.tags[0], x: g.x - 19, y: g.y - 19 }];
  return [];
}

function groupBox(g) {
  if (g.kind === 'pair') return [g.x - 124, g.y - 81, g.x + 124, g.y + 59];
  var h = { diamond: 59, cross: 59, quad: 42, slot: 19 }[g.kind];
  return [g.x - h, g.y - h, g.x + h, g.y + h];
}

function face(g, side) {
  var e = { diamond: 59, cross: 59, quad: 42, slot: 19, pair: 124 }[g.kind], ev = (g.kind === 'pair' || g.kind === 'diamond') ? 39 : e;
  var vx = g.kind === 'pair' ? g.x + (g.x < 490 ? 65 : -65) : g.x;
  return side === 'r' ? [g.x + e, g.y] : side === 'l' ? [g.x - e, g.y] : side === 't' ? [vx, g.y - ev] : [vx, g.y + ev];
}

function snap(v) { return Math.round(v / 4) * 4; }

function freshPath(f, d, side) {
  var o = 16;
  if (side === 'r' || side === 'l') {
    var out = side === 'r' ? 1 : -1;
    if (f[1] === d[1] && (d[0] - f[0]) * out > 0) return [f, d];
    var far = (d[0] - f[0]) * out > o * 2;
    var mx = far ? snap((f[0] + d[0]) / 2) : f[0] + out * o;
    return [f, [mx, f[1]], [mx, d[1]], d];
  }
  var outy = side === 'b' ? 1 : -1;
  if (f[0] === d[0] && (d[1] - f[1]) * outy > 0) return [f, d];
  var fary = (d[1] - f[1]) * outy > o * 2;
  var my = fary ? snap((f[1] + d[1]) / 2) : f[1] + outy * o;
  return [f, [f[0], my], [d[0], my], d];
}

function defaultLayout(pad) {
  var ds = pad === 'dualsense', ns2 = pad === 'ns2_pro';
  var face4 = ds ? ['PS:tri', 'PS:sq', 'PS:ring', 'PS:x'] : ['X', 'Y', 'A', 'B'];
  var LS = ['15', '17', '18', '16', '7'], RS = ['19', '21', '22', '20', '8'];
  var G = [
    { id: 'qL', kind: 'pair', x: 143, y: 145, keys: ['L11', 'L13', 'L14', 'L12', 'L3', 'L2', 'L1', 'L0'], tags: face4, cap: (ds ? 'L2' : 'ZL') + ' 快捷頁' },
    { id: 'qR', kind: 'pair', x: 837, y: 145, keys: ['R11', 'R13', 'R14', 'R12', 'R3', 'R2', 'R1', 'R0'], tags: face4, cap: (ds ? 'R2' : 'ZR') + ' 快捷頁' },
    { id: 'b1L', kind: 'slot', x: 311, y: 192, keys: ['9'], tags: [ds ? 'L1' : 'L'] },
    { id: 'b1R', kind: 'slot', x: 669, y: 192, keys: ['10'], tags: [ds ? 'R1' : 'R'] },
    { id: 'face', kind: 'diamond', x: 884, y: 288, keys: ['3', '2', '1', '0'], tags: face4 },
    { id: 'rs', kind: 'cross', x: 736, y: 320, keys: RS }
  ];
  var R = [
    { id: 'zl', g: 'qL', side: 'r', pts: [[267, 145], [414, 145], [414, 172]], m: 'c' },
    { id: 'zr', g: 'qR', side: 'l', pts: [[713, 145], [566, 145], [566, 172]], m: 'c' },
    { id: 'l1', g: 'b1L', side: 'r', pts: [[330, 192], [348, 192], [348, 222], [367, 222]], m: 'c' },
    { id: 'r1', g: 'b1R', side: 'l', pts: [[650, 192], [632, 192], [632, 222], [613, 222]], m: 'c' },
    { id: 'rs', g: 'rs', side: 'l', pts: [[677, 320], [648, 320], [648, 286], [536, 286]], m: 'c' },
    { id: 'face', g: 'face', side: 'l', pts: [[825, 288], [792, 288], [792, 232], [566, 232]], m: 'c' }
  ];
  if (ds) {
    // DualSense: D-pad sits high on the pad, so its group is the outer, higher one; the left stick is the inner, lower one.
    G.push({ id: 'dpad', kind: 'diamond', x: 96, y: 288, keys: ['11', '13', '14', '12'], tags: ARROWS },
      { id: 'ls', kind: 'cross', x: 244, y: 320, keys: LS },
      { id: 'share', kind: 'slot', x: 399, y: 89, keys: ['4'], tags: ['Share'] },
      { id: 'tpq', kind: 'quad', x: 490, y: 112, keys: ['25', '26', '27', '28'], tags: ['T:tl', 'T:tr', 'T:bl', 'T:br'] },
      { id: 'opt', kind: 'slot', x: 617, y: 89, keys: ['6'], tags: ['Opt'] },
      { id: 'ps', kind: 'slot', x: 491, y: 372, keys: ['5'], tags: ['PS'] });
    R.push({ id: 'dpad', g: 'dpad', side: 'r', pts: [[155, 288], [188, 288], [188, 234], [416, 234]], m: 'c' },
      { id: 'ls', g: 'ls', side: 'r', pts: [[303, 320], [332, 320], [332, 286], [456, 286]], m: 'c' },
      { id: 'share', g: 'share', side: 'b', pts: [[399, 108], [399, 124], [436, 124], [436, 210]], r: 7 },
      { id: 'tp', g: 'tpq', side: 'b', pts: [[490, 154], [490, 219]], m: 'c' },
      { id: 'opt', g: 'opt', side: 'b', pts: [[617, 108], [617, 130], [544, 130], [544, 210]], r: 7 },
      { id: 'ps', g: 'ps', side: 't', pts: [[491, 353], [491, 270]], r: 7 });
  } else {
    // Switch: left stick high (outer group), D-pad low (inner group); small buttons in one row, each line straight down where it can be.
    G.push({ id: 'ls', kind: 'cross', x: 96, y: 288, keys: LS },
      { id: 'dpad', kind: 'diamond', x: 244, y: 320, keys: ['11', '13', '14', '12'], tags: ARROWS },
      { id: 'minus', kind: 'slot', x: 399, y: 89, keys: ['4'], tags: ['−'] },
      { id: 'cap', kind: 'slot', x: 466, y: 89, keys: ['29'], tags: ['Cap'] },
      { id: 'home', kind: 'slot', x: 514, y: 89, keys: ['5'], tags: ['Home'] },
      { id: 'plus', kind: 'slot', x: 583, y: 89, keys: ['6'], tags: ['+'] });
    R.push({ id: 'ls', g: 'ls', side: 'r', pts: [[155, 288], [188, 288], [188, 232], [420, 232]], m: 'c' },
      { id: 'dpad', g: 'dpad', side: 'r', pts: [[303, 320], [332, 320], [332, 286], [456, 286]], m: 'c' },
      { id: 'minus', g: 'minus', side: 'b', pts: [[399, 108], [399, 129], [452, 129], [452, 208.5]], r: 2.5 },
      { id: 'cap', g: 'cap', side: 'b', pts: [[466, 108], [466, 236]], r: 6 },
      { id: 'home', g: 'home', side: 'b', pts: [[514, 108], [514, 236]], r: 7 },
      { id: 'plus', g: 'plus', side: 'b', pts: [[583, 108], [583, 129], [528, 129], [528, 208]], r: 7.5 });
    if (ns2) {
      G.push({ id: 'c', kind: 'slot', x: 491, y: 372, keys: ['32'], tags: ['C'] },
        { id: 'gl', kind: 'slot', x: 344, y: 372, keys: ['30'], tags: ['GL'] },
        { id: 'gr', kind: 'slot', x: 636, y: 372, keys: ['31'], tags: ['GR'] });
      R.push({ id: 'c', g: 'c', side: 't', pts: [[491, 353], [491, 256]], r: 6 },
        { id: 'gl', g: 'gl', side: 'r', pts: [[363, 372], [418, 372], [418, 340]], r: 7 },
        { id: 'gr', g: 'gr', side: 'l', pts: [[617, 372], [562, 372], [562, 340]], r: 7 });
    }
  }
  return { groups: G, routes: R };
}

function attach(pts, f, side) {
  var horiz = side === 'r' || side === 'l';
  pts[0] = f;
  if (pts.length === 2) {
    var d = pts[1];
    if ((horiz && f[1] !== d[1]) || (!horiz && f[0] !== d[0])) return freshPath(f, d, side);
    return pts;
  }
  if (horiz) pts[1][1] = f[1]; else pts[1][0] = f[0];
  return pts;
}

function applyQuickMode(pad, L, modes) {
  ['L', 'R'].forEach(function (side) {
    var gid = side === 'L' ? 'qL' : 'qR', g = L.byId[gid];
    g.q = side;
    if ((side === 'L' ? modes.l : modes.r) !== 'key') return;
    g.kind = 'slot'; g.keys = [side === 'L' ? '23' : '24']; g.tags = [pad === 'dualsense' ? side + '2' : 'Z' + side];
  });
}

function clean(p) {
  var q = [p[0]];
  for (var i = 1; i < p.length; i++) {
    var a = q[q.length - 1], b = p[i];
    if (a[0] === b[0] && a[1] === b[1]) continue;
    if (q.length >= 2 && i < p.length) {
      var z = q[q.length - 2];
      if ((z[0] === a[0] && a[0] === b[0]) || (z[1] === a[1] && a[1] === b[1])) q.pop();
    }
    q.push(b);
  }
  return q;
}

function lint(L) {
  var segs = [], slots = [], n = { cross: 0, through: 0, overlap: 0 };
  L.routes.forEach(function (r, ri) { for (var i = 0; i + 1 < r.pts.length; i++) segs.push([ri, r.pts[i], r.pts[i + 1]]); });
  L.groups.forEach(function (g) { slots = slots.concat(groupSlots(g)); });
  for (var i = 0; i < segs.length; i++) for (var j = i + 1; j < segs.length; j++) {
    var s1 = segs[i], s2 = segs[j];
    if (s1[0] === s2[0]) continue;
    var h1 = s1[1][1] === s1[2][1], h2 = s2[1][1] === s2[2][1];
    if (h1 === h2) continue;
    var hs = h1 ? s1 : s2, vs = h1 ? s2 : s1, x = vs[1][0], y = hs[1][1];
    if (x > Math.min(hs[1][0], hs[2][0]) && x < Math.max(hs[1][0], hs[2][0]) && y > Math.min(vs[1][1], vs[2][1]) && y < Math.max(vs[1][1], vs[2][1])) n.cross++;
  }
  for (var a = 0; a < slots.length; a++) for (var c = a + 1; c < slots.length; c++) {
    var A = slots[a], B = slots[c];
    if (A.x < B.x + 38 && B.x < A.x + 38 && A.y < B.y + 38 && B.y < A.y + 38) n.overlap++;
  }
  segs.forEach(function (sg) {
    var x0 = Math.min(sg[1][0], sg[2][0]), x1 = Math.max(sg[1][0], sg[2][0]), y0 = Math.min(sg[1][1], sg[2][1]), y1 = Math.max(sg[1][1], sg[2][1]);
    var st = L.routes[sg[0]].pts[0];
    slots.forEach(function (sl) {
      if (x0 < sl.x + 37 && sl.x + 1 < x1 + 1 && y0 < sl.y + 37 && sl.y + 1 < y1 + 1) {
        var own = st[0] >= sl.x - 1 && st[0] <= sl.x + 39 && st[1] >= sl.y - 1 && st[1] <= sl.y + 39;
        if (!own) n.through++;
      }
    });
  });
  return n;
}

var ARROWS = ['↑', '←', '→', '↓'];

// The layout of one pad, with each trigger in 'page' (two diamonds) or
// 'key' (one slot, id 23 / 24) mode.
export function layoutFor(pad, modes) {
  var L = defaultLayout(pad), byId = {};
  L.groups.forEach(function (g) { byId[g.id] = g; });
  L.byId = byId;
  applyQuickMode(pad, L, modes || { l: 'page', r: 'page' });
  L.routes.forEach(function (rt) {
    rt.pts = clean(attach(rt.pts.map(function (q) { return [q[0], q[1]]; }), face(byId[rt.g], rt.side), rt.side));
  });
  return L;
}

export function slotsOf(L) {
  var out = [];
  L.groups.forEach(function (g) { out = out.concat(groupSlots(g)); });
  return out;
}

export { groupSlots, lint };

// One guide line as one SVG path (spec 4.3: every line, not only the lit
// one), in an <svg class="lines"> overlay. m === 'c' ends at the target's
// centre; otherwise the end stops r + 4 short of it (small buttons).
export function routeHTML(p, r, m, hit) {
  var pts = p.map(function (q) { return [q[0], q[1]]; });
  var n = pts.length, c = pts[n - 1], prev = pts[n - 2];
  if (m !== 'c') {
    var dx = Math.sign(c[0] - prev[0]), dy = Math.sign(c[1] - prev[1]), stop = r + 4;
    pts[n - 1] = [c[0] - dx * stop, c[1] - dy * stop];
  }
  var d = pts.map(function (q, i) { return (i ? 'L' : 'M') + (q[0] + 0.5) + ',' + (q[1] + 0.5); }).join(' '), e = pts[n - 1];
  var ex = e[0] + 0.5, ey = e[1] + 0.5;
  if (hit) {
    return '<g class="hit"><path d="' + d + '" fill="none" stroke="#7a5c00" stroke-width="5" stroke-linejoin="miter"/>' +
      '<circle cx="' + ex + '" cy="' + ey + '" r="4.5" fill="#7a5c00"/><path d="' + d + '" fill="none" stroke="#ffcf2e" stroke-width="3" stroke-linejoin="miter"/>' +
      '<circle cx="' + ex + '" cy="' + ey + '" r="3.5" fill="#ffcf2e"/></g>';
  }
  return '<g><path d="' + d + '" fill="none" stroke="#c4232c" stroke-width="3" stroke-linejoin="miter"/><circle cx="' + ex + '" cy="' + ey + '" r="3.5" fill="#c4232c"/></g>';
}
