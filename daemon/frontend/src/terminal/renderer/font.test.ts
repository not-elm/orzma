import { describe, expect, it } from 'vitest';
import { measureFont, widthOfGrapheme } from './font';

describe('measureFont', () => {
  it('returns positive cellW and cellH for a typical mono font', () => {
    const canvas = document.createElement('canvas');
    const fm = measureFont(canvas, '14px monospace');
    expect(fm.cellW).toBeGreaterThan(0);
    expect(fm.cellH).toBeGreaterThan(0);
    expect(fm.baseline).toBeGreaterThanOrEqual(0);
    expect(fm.fontCss).toBe('14px monospace');
    expect(fm.dpr).toBeGreaterThan(0);
  });
});

describe('widthOfGrapheme', () => {
  it('returns 1 for ASCII', () => {
    expect(widthOfGrapheme('a')).toBe(1);
    expect(widthOfGrapheme(' ')).toBe(1);
  });

  it('returns 2 for East Asian Wide', () => {
    expect(widthOfGrapheme('あ')).toBe(2);
    expect(widthOfGrapheme('漢')).toBe(2);
  });

  it('returns 0 for combining marks', () => {
    expect(widthOfGrapheme('́')).toBe(0); // combining acute accent
  });
});
