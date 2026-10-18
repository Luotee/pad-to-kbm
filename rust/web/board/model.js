// The board's vocabulary <-> the config (pure: no DOM, no IPC).
//
// Slots: '11' = base id 11; 'L11' / 'R0' = quick_l 11 / quick_r 0.
// Tiles: palette key names ('Q', '↑', 'LShift'), mouse tiles ('左鍵' …) and
// macros as 'M:<name>' (a macro's identity is its name, spec 8).

export const PADS = ['dualsense', 'ns_pro', 'ns2_pro'];
export const PAD_NAMES = { dualsense: 'DualSense', ns_pro: 'Switch Pro', ns2_pro: 'Switch 2 Pro' };
export const QUICK_IDS = [0, 1, 2, 3, 11, 12, 13, 14];
export const MOUSE_TILE = { left: '左鍵', right: '右鍵', middle: '中鍵', 'wheel-up': '上滾', 'wheel-down': '下滾' };
const MOUSE_VALUE = Object.fromEntries(Object.entries(MOUSE_TILE).map(([v, t]) => [t, v]));
const PAGE_FIELD = { base: 'buttons', quick_l: 'quick_l', quick_r: 'quick_r' };

export function slotRef(key) {
  const s = String(key);
  if (s[0] === 'L') return { page: 'quick_l', id: Number(s.slice(1)) };
  if (s[0] === 'R') return { page: 'quick_r', id: Number(s.slice(1)) };
  return { page: 'base', id: Number(s) };
}

export function slotKey(page, id) {
  return page === 'quick_l' ? 'L' + id : page === 'quick_r' ? 'R' + id : String(id);
}

export function tableOf(config, pad, page) {
  return config?.pads?.[pad]?.[PAGE_FIELD[page]] || {};
}

// A config entry as a tile name, or '' when unbound.
export function tileOfEntry(e) {
  if (!e || e.type === 'unbound') return '';
  if (e.type === 'key') return keyToTile(e.value);
  if (e.type === 'mouse') return MOUSE_TILE[e.value] || e.value;
  if (e.type === 'macro') return 'M:' + e.value;
  return '';
}

export function boardBinds(config, pad) {
  const out = {};
  for (const page of ['base', 'quick_l', 'quick_r']) {
    for (const [id, e] of Object.entries(tableOf(config, pad, page))) {
      const t = tileOfEntry(e);
      if (t) out[slotKey(page, Number(id))] = t;
    }
  }
  return out;
}

export function entryForTile(tile, label) {
  if (!tile) return { label, type: 'unbound' };
  if (tile.startsWith('M:')) return { label, type: 'macro', value: tile.slice(2) };
  if (MOUSE_VALUE[tile]) return { label, type: 'mouse', value: MOUSE_VALUE[tile] };
  return { label, type: 'key', value: tileToKey(tile) };
}

const ALIAS = {
  shift: 'LShift', lshift: 'LShift', rshift: 'RShift',
  control: 'LCtrl', ctrl: 'LCtrl', lcontrol: 'LCtrl', lctrl: 'LCtrl', rcontrol: 'RCtrl', rctrl: 'RCtrl',
  alt: 'LAlt', lalt: 'LAlt', ralt: 'RAlt', meta: 'Win', win: 'Win', cmd: 'Win',
  up: '↑', down: '↓', left: '←', right: '→', escape: 'Esc', esc: 'Esc', enter: 'Enter', return: 'Enter',
  delete: 'Del', del: 'Del', backspace: 'Backspace', pageup: 'PageUp', pagedown: 'PageDown',
  home: 'Home', end: 'End', tab: 'Tab', space: 'Space',
};

// A config key name (any case, any alias parse_key accepts) as a palette tile.
export function keyToTile(value) {
  const v = String(value ?? '');
  if ([...v].length === 1) return v.toUpperCase();
  const low = v.toLowerCase();
  if (ALIAS[low]) return ALIAS[low];
  if (/^f([1-9]|1[0-2])$/.test(low)) return 'F' + low.slice(1);
  return v;
}

const ARROW_KEY = { '↑': 'Up', '↓': 'Down', '←': 'Left', '→': 'Right' };

// A palette tile as the key name written to the config.
export function tileToKey(tile) {
  if (ARROW_KEY[tile]) return ARROW_KEY[tile];
  if ([...tile].length === 1) return tile.toLowerCase();
  return tile;
}

// The label stored with an entry: the base entry's label for that id.
export function slotLabel(config, pad, key) {
  const { id } = slotRef(key);
  return tableOf(config, pad, 'base')[String(id)]?.label ?? '';
}
