import stringWidth from 'string-width';

/** Per-cell font geometry produced by the DOM probes (`cellWidthOf` / `cellHeightOf`). */
export interface FontMetrics {
  /** Pixel width of one terminal cell (rounded). */
  cellW: number;
  /** Pixel height of one terminal cell (rounded). */
  cellH: number;
  /** Baseline offset from the top of the cell, in pixels. */
  baseline: number;
  /** CSS `font` value used to render this metric set. */
  fontCss: string;
  /** Device pixel ratio at measurement time. */
  dpr: number;
  /** Negative tracking (usually) that compresses the font's natural glyph
   *  advance into the rounded [`cellW`] grid. Applied as `letter-spacing` on
   *  the row container so that column N of rendered text aligns with the
   *  cursor's `cursor.x * cellW` position. xterm.js `DomRenderer` uses the
   *  same trick (`_setDefaultSpacing`); without it, the per-char drift of
   *  `naturalCellW − cellW` (~0.2 CSS px at 9pt) accumulates to roughly one
   *  cell across 30–40 columns and the cursor visibly lags the text. */
  letterSpacing: number;
}

/** Class that makes an element pick up the configured terminal font and size
 *  from the runtime palette stylesheet (`palette.ts`). `font-mono` is the
 *  pre-injection fallback. */
export const PROBE_CLASS = 'ozmux-font-probe';

/** Per-style font-family classes emitted by `palette.ts` and applied to run
 *  spans (`Row.tsx`) and glyph probes. */
export const FACE_BOLD = 'tf-bold';
export const FACE_ITALIC = 'tf-italic';
export const FACE_BOLD_ITALIC = 'tf-bold-italic';

const PROBE_BASE_CLASS = `font-mono ${PROBE_CLASS} leading-none`;

/** Returns the `tf-*` font-family class for a (bold, italic) pair, or `''`
 *  for the normal face. */
export function faceClass(bold: boolean, italic: boolean): string {
  if (bold && italic) return FACE_BOLD_ITALIC;
  if (bold) return FACE_BOLD;
  if (italic) return FACE_ITALIC;
  return '';
}

/** Returns the column width of one grapheme cluster (0, 1, or 2). */
export function widthOfGrapheme(text: string): 0 | 1 | 2 {
  const w = stringWidth(text);
  if (w === 0) return 0;
  if (w === 2) return 2;
  return 1;
}

/** Creates a hidden measurement probe inside `container`, reads its bounding
 *  rect via `read`, then removes it. `container` only determines where the
 *  probe attaches (for theme-token / stacking-context inheritance) — the
 *  probe's own classes drive the measured font. */
function withProbe<T>(
  container: HTMLElement,
  className: string,
  text: string,
  read: (rect: DOMRect) => T,
  configure?: (probe: HTMLElement) => void,
): T {
  const probe = document.createElement('span');
  probe.style.visibility = 'hidden';
  probe.style.position = 'absolute';
  probe.style.whiteSpace = 'pre';
  probe.className = className;
  configure?.(probe);
  probe.textContent = text;
  container.appendChild(probe);
  const result = read(probe.getBoundingClientRect());
  container.removeChild(probe);
  return result;
}

/** Measures the rendered width of "W" in the terminal font. Used by Row.tsx
 *  for `letterSpacing = cellW - cellWidthOf(...)` to prevent sub-pixel drift
 *  on long rows (xterm.js `DomRenderer._setDefaultSpacing`).
 *
 *  Rounded to integer CSS px: `getBoundingClientRect()` returns sub-pixel
 *  floats which cascade into row `height`/`line-height`/cursor coordinates
 *  and cause text to render against fractional pixel boundaries — glyphs
 *  end up rasterised across two device pixels and look softer than
 *  Alacritty's CoreText output. xterm.js #985 fixed the same class of
 *  blur by quantising the cell box.
 */
export function cellWidthOf(container: HTMLElement): number {
  return withProbe(container, PROBE_BASE_CLASS, 'W', (r) => Math.round(r.width));
}

/** Measures the unrounded width of "W" in the terminal font. Paired with
 *  [`cellWidthOf`] to derive the row-level `letter-spacing` correction. */
export function naturalCellWidthOf(container: HTMLElement): number {
  return withProbe(container, PROBE_BASE_CLASS, 'W', (r) => r.width);
}

/** Measures the line-height-1 height of one row in the terminal font, so
 *  `cellH` matches the actual row line-box height. Rounded to integer CSS px
 *  for the same reason as [`cellWidthOf`]. */
export function cellHeightOf(container: HTMLElement): number {
  return withProbe(container, PROBE_BASE_CLASS, 'W', (r) => Math.round(r.height));
}

// NOTE: keyed by (chars, bold, italic) only — must be cleared if the font
// config changes at runtime (currently loaded once before first render).
const glyphWidthCache = new Map<string, number>();

/** Measures the rendered width of `chars` in the terminal font, optionally
 *  with bold / italic applied. Cached by (chars, bold, italic) key. */
export function measureGlyph(
  container: HTMLElement,
  chars: string,
  bold: boolean,
  italic: boolean,
): number {
  const key = `${bold ? 'b' : ''}${italic ? 'i' : ''}|${chars}`;
  const hit = glyphWidthCache.get(key);
  if (hit !== undefined) return hit;
  const face = faceClass(bold, italic);
  const width = withProbe(
    container,
    face ? `${PROBE_BASE_CLASS} ${face}` : PROBE_BASE_CLASS,
    chars,
    (r) => r.width,
    (probe) => {
      if (bold) probe.style.fontWeight = 'bold';
      if (italic) probe.style.fontStyle = 'italic';
    },
  );
  glyphWidthCache.set(key, width);
  return width;
}

/** Test helper — clears the glyph width cache between vitest cases. */
export function __resetGlyphWidthCacheForTests(): void {
  glyphWidthCache.clear();
}
