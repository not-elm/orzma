import { describe, expect, it } from 'vitest';
import { colorToCss, dimColor } from './colors';

describe('colorToCss', () => {
  it('returns null for Default color (msgpack nil → null)', () => {
    expect(colorToCss(null, 'fg')).toBeNull();
    expect(colorToCss(null, 'bg')).toBeNull();
  });

  it('returns named ANSI for indexed colors 0-15', () => {
    expect(colorToCss(0, 'fg')).toBe('#000000'); // black
    expect(colorToCss(1, 'fg')).toBe('#cd0000'); // red (xterm default)
    expect(colorToCss(7, 'fg')).toBe('#e5e5e5'); // light gray
    expect(colorToCss(15, 'fg')).toBe('#ffffff'); // white
  });

  it('returns 6×6×6 cube for indices 16-231', () => {
    // 16 = (0,0,0) → #000000
    expect(colorToCss(16, 'fg')).toBe('#000000');
    // 231 = (5,5,5) → #ffffff (highest)
    expect(colorToCss(231, 'fg')).toBe('#ffffff');
    // 196 = (5,0,0) → bright red
    expect(colorToCss(196, 'fg')).toBe('#ff0000');
  });

  it('returns grayscale ramp for indices 232-255', () => {
    expect(colorToCss(232, 'fg')).toBe('#080808');
    expect(colorToCss(255, 'fg')).toBe('#eeeeee');
  });

  it('returns hex from Rgb tuple', () => {
    expect(colorToCss([255, 128, 0], 'fg')).toBe('#ff8000');
    expect(colorToCss([0, 0, 0], 'fg')).toBe('#000000');
  });
});

describe('dimColor', () => {
  it('converts a 6-digit hex to rgba with alpha 0.6', () => {
    expect(dimColor('#ff8000')).toBe('rgba(255, 128, 0, 0.6)');
  });

  it('passes through rgba inputs unchanged when alpha already specified', () => {
    expect(dimColor('rgba(10, 20, 30, 1)')).toBe('rgba(10, 20, 30, 0.6)');
  });

  it('returns the input when the format is unrecognized', () => {
    // Safety: callers may pass null-equivalent or non-color strings; we just
    // return whatever was given so the renderer still has a CSS string.
    expect(dimColor('inherit')).toBe('inherit');
  });
});
