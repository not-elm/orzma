import stringWidth from 'string-width';

/** Per-cell font geometry derived from canvas measureText. */
export interface FontMetrics {
  /** Pixel width of one terminal cell. */
  cellW: number;
  /** Pixel height of one terminal cell. */
  cellH: number;
  /** Baseline offset from the top of the cell, in pixels. */
  baseline: number;
  /** CSS `font` value used to render this metric set. */
  fontCss: string;
  /** Device pixel ratio at measurement time. */
  dpr: number;
}

/** Measures one cell in the given CSS font. Returns ceil-rounded pixel sizes. */
export function measureFont(canvas: HTMLCanvasElement, fontCss: string): FontMetrics {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    throw new Error('measureFont: getContext("2d") returned null');
  }
  ctx.font = fontCss;
  const m = ctx.measureText('M');
  const ascent = m.actualBoundingBoxAscent ?? 12;
  const descent = m.actualBoundingBoxDescent ?? 3;
  return {
    cellW: Math.ceil(m.width),
    cellH: Math.ceil(ascent + descent),
    baseline: Math.ceil(ascent),
    fontCss,
    dpr: typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1,
  };
}

/** Returns the column width of one grapheme cluster (0, 1, or 2). */
export function widthOfGrapheme(text: string): 0 | 1 | 2 {
  const w = stringWidth(text);
  if (w === 0) return 0;
  if (w === 2) return 2;
  return 1;
}
