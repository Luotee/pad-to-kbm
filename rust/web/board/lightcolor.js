// Light bar colours for the 燈條 panel (pure): the six preset slots, the speed names, and the
// hue / saturation strips of the 自訂 colour picker (24 cells each, value always 1).

export const SPEEDS = ['slow', 'medium', 'fast', 'hyper'];
export const SPEED_TXT = ['慢', '中', '快', '極快'];
export const PRESETS = [['白', '#FFFFFF'], ['紅', '#FF2A2A'], ['橘', '#FF7A00'], ['綠', '#2BD94A'], ['天藍', '#2EA8FF'], ['紫', '#8A3CFF']];

// h in degrees, s and v in 0..1 -> '#RRGGBB' (upper case).
export function hsvHex(h, s, v) {
  const f = n => { const k = (n + h / 60) % 6; return v - v * s * Math.max(0, Math.min(k, 4 - k, 1)); };
  return '#' + [5, 3, 1].map(n => Math.round(f(n) * 255).toString(16).padStart(2, '0')).join('').toUpperCase();
}

export function hexHsv(hex) {
  const [r, g, b] = [1, 3, 5].map(o => parseInt(hex.slice(o, o + 2), 16) / 255);
  const mx = Math.max(r, g, b), d = mx - Math.min(r, g, b);
  let h = d === 0 ? 0 : mx === r ? 60 * (((g - b) / d) % 6) : mx === g ? 60 * ((b - r) / d + 2) : 60 * ((r - g) / d + 4);
  if (h < 0) h += 360;
  return { h, s: mx === 0 ? 0 : d / mx, v: mx };
}

// The picker cells a colour falls on: hue cell hi (15 degrees apart), saturation cell si (s = (si + 1) / 24).
export function pickerIndex(hex) {
  const { h, s } = hexHsv(hex);
  return { hi: Math.round(h / 15) % 24, si: Math.max(0, Math.round(s * 24) - 1) };
}

export const hueCells = si => Array.from({ length: 24 }, (_, i) => hsvHex(i * 15, (si + 1) / 24, 1));
export const satCells = hi => Array.from({ length: 24 }, (_, k) => hsvHex(hi * 15, (k + 1) / 24, 1));

// What the 色碼 field accepts: six hex digits, with or without '#', any case.
export function normHex(text) {
  const m = /^#?([0-9a-f]{6})$/i.exec(String(text).trim());
  return m ? '#' + m[1].toUpperCase() : null;
}
