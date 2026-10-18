// The palette under the stage: keyboard, mouse, macro box (spec 4.1 / 4.6),
// from the approved mockup. Pure HTML builders over the board state.

import { MAIN, NAV, LABEL, U, GAP, ROW } from './keys.js';
import { esc } from './tags.js';
import { routeHTML } from './layout.js';
import { ICON } from './icons.js';

export const PER = 8;

// Silkscreen has no arrows: the arrow keys draw a 7x7 pixel arrow (3-row head, 3-row stem), turned on
// the pixel grid, white over a 1px dark shadow like the letters.
const UP = 'M3 0h1v1h1v1h1v1h-2v3h-1v-3h-2v-1h1v-1h1z', ARW = { '↑': 0, '→': 90, '↓': 180, '←': 270 };
function arrowSvg(k) {
  const r = 'rotate(' + ARW[k] + ' 3.5 3.5)';
  return '<svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><g transform="translate(1 1)"><path d="' + UP + '" transform="' + r +
    '" fill="rgba(10,0,4,.8)"/></g><path d="' + UP + '" transform="' + r + '" fill="#fff"/></svg>';
}

// `used`: tiles already bound on the shown pad, drawn pale (display only; they still drag and copy).
function tile(st, key, text, w, cls, used) {
  const dis = !!cls && cls.includes('dis');
  return '<button type="button" class="tile' + (st.picked === key ? ' picked' : '') + (st.hot[key] ? ' hit' : '') + (used.has(key) ? ' used' : '') + (cls ? ' ' + cls : '') + '"' +
    (dis ? ' disabled aria-label="' + esc(text) + '（不支援）"' : ' draggable="true" data-tile="' + esc(key) + '"') + ' style="width:' + w + 'px">' + text + '</button>';
}

function krow(st, row, fn, justify, used) {
  const widths = row.map(k => Math.round(k[1] * U - GAP));
  if (justify && row.length) {
    const used = widths.reduce((a, b) => a + b, 0) + (row.length - 1) * GAP;
    widths[widths.length - 1] += ROW - used;
  }
  let h = '<div class="krow' + (fn ? ' fn' : '') + '">';
  row.forEach((k, i) => {
    if (!k[0]) { h += '<span class="gap" style="width:' + widths[i] + 'px"></span>'; return; }
    const label = LABEL[k[0]] || k[0];
    h += tile(st, k[0], k[0] in ARW ? arrowSvg(k[0]) : esc(label), widths[i], k[2] || '', used);
  });
  return h + '</div>';
}

export function keyboardHTML(st, used = new Set()) {
  return '<div class="kbd base" role="group" aria-label="鍵盤"><div class="kmain">' + MAIN.map((r, i) => krow(st, r, i === 0, true, used)).join('') +
    '</div><div class="knav">' + NAV.map((r, i) => krow(st, r, i === 0, false, used)).join('') + '</div></div>';
}

// Mouse: left/right from the top corners (centre dots); wheel up / click /
// down grouped bottom-right (edge stops).
export function mouseHTML(st, used = new Set()) {
  let h = '<div class="m3"><svg class="shell" style="position:absolute;left:53px;top:18px" width="97" height="141" viewBox="0 0 110 160" aria-hidden="true">' +
    '<path class="body" d="M55,3 C88,3 107,30 107,72 L107,110 C107,140 85,157 55,157 C25,157 3,140 3,110 L3,72 C3,30 22,3 55,3 Z"></path>' +
    '<path class="seam" d="M55,4 L55,34 M55,62 L55,80 M6,80 Q55,90 104,80"></path>' +
    '<rect x="49" y="34" width="12" height="28" rx="6" fill="#9aa1b2" stroke="#454c60" stroke-width="1.5"></rect>' +
    '<rect class="wheel" x="51" y="36.5" width="8" height="23" rx="4"></rect>' +
    '<path d="M52,41 H58 M52,45 H58 M52,49 H58 M52,53 H58 M52,57 H58" stroke="#6c7386" stroke-width="1"></path>' +
    '<path class="ink" d="M55,20 L59,26 L51,26 Z"></path><path class="ink" d="M51,70 L59,70 L55,76 Z"></path></svg>';
  const t = (key, x, y) => '<button type="button" class="tile' + (st.picked === key ? ' picked' : '') + (st.hot[key] ? ' hit' : '') + (used.has(key) ? ' used' : '') +
    '" draggable="true" data-tile="' + key + '" style="left:' + x + 'px;top:' + y + 'px;width:51px">' + key + '</button>';
  h += t('左鍵', 4, 4) + t('右鍵', 174, 4) + t('上滾', 174, 97) + t('中鍵', 174, 123) + t('下滾', 174, 150);
  h += '<svg class="lines" viewBox="0 0 260 210" aria-hidden="true">' +
    routeHTML([[62, 16], [80, 16], [80, 45]], 0, 'c') + routeHTML([[198, 16], [150, 16], [150, 45]], 0, 'c') +
    routeHTML([[198, 122], [192, 122], [192, 50], [115, 50]], 0, 'c') +
    routeHTML([[198, 152], [184, 152], [184, 68], [115, 68]], 0, 'c') +
    routeHTML([[198, 182], [176, 182], [176, 86], [115, 86]], 0, 'c') + '</svg>';
  return h + '</div>';
}

export function macroNames(config) {
  return Object.keys(config?.macros || {});
}

export function macroPages(config) {
  return Math.ceil((macroNames(config).length + 1) / PER);
}

export function macroBoxHTML(st, lock, used = new Set()) {
  const names = macroNames(st.config), total = names.length + 1, pages = Math.ceil(total / PER), pg = Math.min(st.mpage, pages - 1);
  let h = '<div class="macros base' + lock + '" role="group" aria-label="巨集"><div class="mgrid">';
  for (let i = pg * PER; i < Math.min(total, (pg + 1) * PER); i++) {
    if (i === names.length) { h += '<button type="button" class="mnew" data-act="mnew" aria-label="新增巨集" title="新增巨集">＋</button>'; continue; }
    const name = names[i], key = 'M:' + name;
    h += '<span class="mt"><button type="button" class="tile mac' + (st.picked === key ? ' picked' : '') + (st.hot[key] ? ' hit' : '') + (used.has(key) ? ' used' : '') + '" draggable="true" data-tile="' + esc(key) + '" title="' + esc(name) + '">' + esc(name) + '</button>' +
      '<button type="button" class="pen" data-medit="' + esc(name) + '" aria-label="編輯巨集 ' + esc(name) + '" title="編輯">' + ICON.pen + '</button></span>';
  }
  h += '</div><div class="mpg"><button type="button" data-act="mprev" aria-label="上一頁"' + (pg === 0 ? ' disabled' : '') + '>' + ICON.prev + '</button>';
  for (let q = 0; q < pages; q++) h += '<button type="button" class="pdot' + (q === pg ? ' on' : '') + '" data-mpage="' + q + '" aria-label="第 ' + (q + 1) + ' 頁" aria-current="' + (q === pg) + '"></button>';
  return h + '<button type="button" data-act="mnext" aria-label="下一頁"' + (pg >= pages - 1 ? ' disabled' : '') + '>' + ICON.next + '</button></div></div>';
}

// Centre every keyboard / mouse label on its glyph ink, on a whole pixel (reskin v3.3.0). Flex centring
// uses the advance box, and pixel glyphs carry side bearings ('1', 'I', '\\'), so a label read 1px off.
// ASCII: the canvas ink box; CJK: the advance less the 1px bearing each side. The 1px text shadow falls
// in the right margin, so the white letters themselves are centred. Run after render and once fonts load.
const cx = typeof document !== 'undefined' ? document.createElement('canvas').getContext('2d') : null;
export function fitKeyLabels(root) {
  for (const el of root.querySelectorAll('.pal .tile')) {
    if (el.closest('.mt')) continue;
    el.style.justifyContent = 'flex-start';
    if (el.firstElementChild?.tagName === 'svg') { el.style.paddingLeft = Math.floor((el.clientWidth - 8) / 2) + 'px'; continue; }
    const kb = !!el.closest('.kbd');
    cx.font = kb ? '8px Silk' : '12px Cubic11';
    let inkL, inkW;
    if (/^[\x20-\x7e]+$/.test(el.textContent)) {
      const m = cx.measureText(el.textContent);
      inkL = Math.round(-m.actualBoundingBoxLeft); inkW = Math.round(m.actualBoundingBoxRight + m.actualBoundingBoxLeft);
    } else {
      const r = document.createRange(); r.selectNodeContents(el);
      // The board may be scaled (app.js fit): measure in layout pixels.
      const k = el.getBoundingClientRect().width / el.offsetWidth || 1;
      inkL = 1; inkW = Math.round(r.getBoundingClientRect().width / k) - 2;
    }
    el.style.paddingLeft = Math.max(0, Math.floor((el.clientWidth - inkW) / 2) - inkL) + 'px';
  }
}

// The drag image of a key (reskin Q7 C): the 38px bound slot it will become.
export function dragGhostHTML(key) {
  const mac = key.startsWith('M:');
  return '<div class="slot on' + (mac ? ' mac' : '') + ' dragimg"><span class="val">' + esc(mac ? key.slice(2) : (LABEL[key] || key)) + '</span></div>';
}
