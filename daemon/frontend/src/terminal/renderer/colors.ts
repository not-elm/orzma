import type { Color } from '../protocol/frame';

/** Standard 16-color palette (xterm defaults). */
const ANSI_16: readonly string[] = [
  '#000000',
  '#cd0000',
  '#00cd00',
  '#cdcd00',
  '#0000ee',
  '#cd00cd',
  '#00cdcd',
  '#e5e5e5',
  '#7f7f7f',
  '#ff0000',
  '#00ff00',
  '#ffff00',
  '#5c5cff',
  '#ff00ff',
  '#00ffff',
  '#ffffff',
];

/** 6-value level used by the 256-color cube. */
const CUBE_LEVELS: readonly number[] = [0, 95, 135, 175, 215, 255];

/** Converts a wire `Color` to a CSS hex string. Returns null for the default color. */
export function colorToCss(color: Color, _channel: 'fg' | 'bg'): string | null {
  if (color === null) return null;
  if (Array.isArray(color)) {
    const [r, g, b] = color;
    return `#${hex2(r)}${hex2(g)}${hex2(b)}`;
  }
  // Indexed
  if (color < 16) return ANSI_16[color];
  if (color < 232) {
    const idx = color - 16;
    const r = CUBE_LEVELS[Math.floor(idx / 36)];
    const g = CUBE_LEVELS[Math.floor((idx % 36) / 6)];
    const b = CUBE_LEVELS[idx % 6];
    return `#${hex2(r)}${hex2(g)}${hex2(b)}`;
  }
  // Grayscale ramp: 232-255 → 8, 18, 28, ... 238
  const level = 8 + (color - 232) * 10;
  return `#${hex2(level)}${hex2(level)}${hex2(level)}`;
}

function hex2(n: number): string {
  return n.toString(16).padStart(2, '0');
}
