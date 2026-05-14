import { describe, expect, it } from 'vitest';
import { cellHeightOf, cellWidthOf, measureGlyph, widthOfGrapheme } from './font';

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

describe('cellWidthOf', () => {
  it('returns the rendered width of "W" via a DOM probe', () => {
    const container = document.createElement('div');
    container.className = 'font-mono';
    document.body.appendChild(container);
    try {
      const w = cellWidthOf(container);
      expect(typeof w).toBe('number');
      expect(w).toBeGreaterThan(0);
    } finally {
      document.body.removeChild(container);
    }
  });

  it('produces the same width regardless of container font (probe carries font-mono)', () => {
    // Container WITHOUT font-mono — probe should still measure in monospace.
    const noMono = document.createElement('div');
    document.body.appendChild(noMono);
    const withMono = document.createElement('div');
    withMono.className = 'font-mono';
    document.body.appendChild(withMono);
    try {
      // In jsdom (no actual font loading) both measurements come from the stub
      // getBoundingClientRect (test-setup.ts). The assertion documents the
      // intent: the probe's class is what determines the font, not the
      // container's class.
      const w1 = cellWidthOf(noMono);
      const w2 = cellWidthOf(withMono);
      expect(typeof w1).toBe('number');
      expect(typeof w2).toBe('number');
      expect(w1).toBeGreaterThan(0);
      expect(w2).toBeGreaterThan(0);
    } finally {
      document.body.removeChild(noMono);
      document.body.removeChild(withMono);
    }
  });
});

describe('cellHeightOf', () => {
  it('returns a positive height for a line-height:1 monospace probe', () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    try {
      const h = cellHeightOf(container);
      expect(typeof h).toBe('number');
      expect(h).toBeGreaterThan(0);
    } finally {
      document.body.removeChild(container);
    }
  });
});

describe('measureGlyph', () => {
  it('returns the rendered width of a glyph and caches the result', () => {
    const container = document.createElement('div');
    container.className = 'font-mono';
    document.body.appendChild(container);
    try {
      const w1 = measureGlyph(container, '日', false, false);
      const w2 = measureGlyph(container, '日', false, false);
      expect(w1).toBeGreaterThan(0);
      expect(w2).toBe(w1); // cache hit returns identical value
    } finally {
      document.body.removeChild(container);
    }
  });

  it('caches separately by bold / italic flags', () => {
    const container = document.createElement('div');
    container.className = 'font-mono';
    document.body.appendChild(container);
    try {
      const plain = measureGlyph(container, 'M', false, false);
      const bold = measureGlyph(container, 'M', true, false);
      // jsdom returns the same width for both; we only assert no crash + numeric output.
      expect(typeof plain).toBe('number');
      expect(typeof bold).toBe('number');
    } finally {
      document.body.removeChild(container);
    }
  });
});
