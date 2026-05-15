import stringWidth from 'string-width';

/** Per-cell font geometry produced by the DOM probes (`cellWidthOf` / `cellHeightOf`). */
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

/** Returns the column width of one grapheme cluster (0, 1, or 2). */
export function widthOfGrapheme(text: string): 0 | 1 | 2 {
  const w = stringWidth(text);
  if (w === 0) return 0;
  if (w === 2) return 2;
  return 1;
}

/** Measures the rendered width of "W" in the monospace font.
 *  Used by Row.tsx for `letterSpacing = cellW - cellWidthOf(...)` to prevent
 *  sub-pixel drift on long rows (xterm.js `DomRenderer._setDefaultSpacing`).
 *
 *  NOTE: probe carries `font-mono ozmux-font-probe leading-none` — `ozmux-font-probe` is
 *  what makes the probe pick up the configured terminal font/size from the runtime palette
 *  stylesheet (`palette.ts`); `font-mono` is the pre-injection fallback. `container` only
 *  determines where the probe is attached (so it inherits the same theme tokens / parent
 *  stacking context). The container's own font does NOT need to be monospace. */
export function cellWidthOf(container: HTMLElement): number {
  const probe = document.createElement('span');
  probe.style.visibility = 'hidden';
  probe.style.position = 'absolute';
  probe.style.whiteSpace = 'pre';
  probe.className = 'font-mono ozmux-font-probe leading-none';
  probe.textContent = 'W';
  container.appendChild(probe);
  const width = probe.getBoundingClientRect().width;
  container.removeChild(probe);
  return width;
}

/** Measures the rendered line-height-1 height of one row in the monospace
 *  font. Mirrors `.terminal-grid` environment (font-mono ozmux-font-probe leading-none) so
 *  `cellH` matches the actual row line-box height.
 *
 *  NOTE: `ozmux-font-probe` picks up the configured terminal font/size from the runtime
 *  palette stylesheet (`palette.ts`); `font-mono` is the pre-injection fallback. */
export function cellHeightOf(container: HTMLElement): number {
  const probe = document.createElement('span');
  probe.style.visibility = 'hidden';
  probe.style.position = 'absolute';
  probe.style.whiteSpace = 'pre';
  probe.className = 'font-mono ozmux-font-probe leading-none';
  probe.textContent = 'W';
  container.appendChild(probe);
  const height = probe.getBoundingClientRect().height;
  container.removeChild(probe);
  return height;
}

const glyphWidthCache = new Map<string, number>();

/** Measures the rendered width of `chars` in the monospace font, optionally
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
  const probe = document.createElement('span');
  probe.style.visibility = 'hidden';
  probe.style.position = 'absolute';
  probe.style.whiteSpace = 'pre';
  const styleClass =
    bold && italic ? ' tf-bold-italic' : bold ? ' tf-bold' : italic ? ' tf-italic' : '';
  probe.className = `font-mono ozmux-font-probe leading-none${styleClass}`;
  if (bold) probe.style.fontWeight = 'bold';
  if (italic) probe.style.fontStyle = 'italic';
  probe.textContent = chars;
  container.appendChild(probe);
  const width = probe.getBoundingClientRect().width;
  container.removeChild(probe);
  glyphWidthCache.set(key, width);
  return width;
}

/** Test helper — clears the glyph width cache between vitest cases. */
export function __resetGlyphWidthCacheForTests(): void {
  glyphWidthCache.clear();
}
