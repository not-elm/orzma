//! Row component for the DOM renderer. Renders one terminal row as a
//! <div> containing inline-block <span> runs (or <a> for OSC 8 / URL regex
//! matches). Wide chars get per-span letterSpacing correction. Underline/
//! overline runs swap space → NBSP so text-decoration paints.
//!
//! NOTE: rows do NOT carry `pointer-events: none` — that would disable native
//! mouse-driven text selection on the inline span content (D1 dominates N1).
//! React.memo + rowVersions still bounds re-renders to actual changes, so the
//! xterm.js "click during replace" landmine (N1) has a smaller surface than
//! it does for xterm.js's unconditional row replacement.

import { clsx } from 'clsx';
import { memo } from 'react';
import type { Color } from '../protocol/frame';
import { colorToCss, dimColor } from './colors';
import { measureGlyph } from './font';
import type { Cell } from './grid';

interface RowProps {
  cells: readonly Cell[];
  /** Bumped by Grid.applyFrame when the row content changes. React.memo
   *  compares this to short-circuit re-renders. */
  version: number;
  fm: { cellW: number; cellH: number };
  hyperlinks: ReadonlyMap<number, string>;
  /** DOM container used for glyph width probes. In production this is the
   *  terminal-grid root; tests inject their own. */
  probeRef: HTMLElement | null;
}

// Attribute bitmask constants — must match daemon/terminal/src/vt/frame.rs
const STYLE_BOLD = 0b000001;
const STYLE_ITALIC = 0b000010;
const STYLE_UNDERLINE = 0b000100;
const STYLE_STRIKE = 0b001000;
const STYLE_REVERSE = 0b010000;
const STYLE_DIM = 0b100000;

// xterm.js WebLinksAddon-equivalent URL regex (parity with the 5.5.0 release).
const URL_RE = /(https?|HTTPS?):\/\/[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~[\]`()<>]/g;
const ALLOWED_PROTOCOLS = new Set(['http:', 'https:', 'mailto:']);

function isAllowedUri(uri: string): boolean {
  try {
    const u = new URL(uri);
    return ALLOWED_PROTOCOLS.has(u.protocol);
  } catch {
    return false;
  }
}

function styleClasses(style: number): string {
  return clsx(
    style & STYLE_BOLD && 'font-bold',
    style & STYLE_ITALIC && 'italic',
    style & STYLE_UNDERLINE && 'underline',
    style & STYLE_STRIKE && 'line-through',
  );
}

function colorClass(c: Color, channel: 'fg' | 'bg'): string {
  if (c === null) return `${channel}-default`;
  if (typeof c === 'number') return `${channel}-${c}`;
  return ''; // truecolor → inline style
}

function colorInlineStyle(
  fg: Color,
  bg: Color,
  style: number,
): { color?: string; backgroundColor?: string } {
  const reverse = (style & STYLE_REVERSE) !== 0;
  const effFg = reverse ? bg : fg;
  const effBg = reverse ? fg : bg;
  const out: { color?: string; backgroundColor?: string } = {};
  if (Array.isArray(effFg)) {
    let c = colorToCss(effFg, 'fg') ?? '';
    if (style & STYLE_DIM) c = dimColor(c);
    out.color = c;
  } else if (style & STYLE_DIM && effFg !== null) {
    // Indexed fg + DIM: resolve through colorToCss then dim.
    const css = colorToCss(effFg, 'fg');
    if (css) out.color = dimColor(css);
  }
  if (Array.isArray(effBg)) {
    out.backgroundColor = colorToCss(effBg, 'bg') ?? '';
  }
  return out;
}

interface Run {
  text: string;
  style: number;
  fg: Color;
  bg: Color;
  startCol: number;
  endCol: number;
  /** True if the cell was a width=2 wide glyph rendered into one grapheme. */
  isWide: boolean;
  linkUri?: string;
}

interface LinkSpan {
  start: number;
  end: number;
  uri: string;
  priority: 0 | 1; // 0 = OSC 8, 1 = URL regex
}

function buildLinkSpans(
  cells: readonly Cell[],
  hyperlinks: ReadonlyMap<number, string>,
): LinkSpan[] {
  const out: LinkSpan[] = [];
  // OSC 8 spans first.
  let col = 0;
  let osc: { start: number; uri: string } | null = null;
  for (const cell of cells) {
    if (cell.width === 0) continue;
    const uri = cell.hyperlinkId != null ? hyperlinks.get(cell.hyperlinkId) : undefined;
    if (osc && uri !== osc.uri) {
      out.push({ start: osc.start, end: col, uri: osc.uri, priority: 0 });
      osc = null;
    }
    if (uri && !osc) osc = { start: col, uri };
    col += cell.width;
  }
  if (osc) out.push({ start: osc.start, end: col, uri: osc.uri, priority: 0 });

  // URL regex spans.
  const visible = cells.filter((c) => c.width > 0);
  const rowText = visible.map((c) => c.text).join('');
  const colByTextIdx: number[] = [];
  let rcol = 0;
  for (const cell of visible) {
    for (let i = 0; i < cell.text.length; i++) colByTextIdx.push(rcol);
    rcol += cell.width;
  }
  URL_RE.lastIndex = 0;
  let m: RegExpExecArray | null = URL_RE.exec(rowText);
  while (m !== null) {
    const colStart = colByTextIdx[m.index] ?? 0;
    const colEnd = (colByTextIdx[m.index + m[0].length - 1] ?? colStart) + 1;
    out.push({ start: colStart, end: colEnd, uri: m[0], priority: 1 });
    m = URL_RE.exec(rowText);
  }
  out.sort((a, b) => a.priority - b.priority || a.start - b.start);
  return out;
}

function spanAt(spans: LinkSpan[], col: number): LinkSpan | null {
  for (const s of spans) {
    if (col >= s.start && col < s.end) return s;
  }
  return null;
}

function coalesceCellsWithLinks(cells: readonly Cell[], linkSpans: LinkSpan[]): Run[] {
  const runs: Run[] = [];
  let col = 0;
  for (const cell of cells) {
    if (cell.width === 0) continue;
    const link = spanAt(linkSpans, col);
    const isWide = cell.width === 2;
    const last = runs[runs.length - 1];
    const sameAttrs =
      last !== undefined &&
      last.style === cell.style &&
      last.fg === cell.fg &&
      last.bg === cell.bg &&
      last.linkUri === link?.uri &&
      !last.isWide &&
      !isWide;
    if (sameAttrs) {
      last.text += cell.text;
      last.endCol = col + cell.width;
    } else {
      runs.push({
        text: cell.text,
        style: cell.style,
        fg: cell.fg,
        bg: cell.bg,
        startCol: col,
        endCol: col + cell.width,
        isWide,
        linkUri: link?.uri,
      });
    }
    col += cell.width;
  }
  return runs;
}

function swapSpacesForDecoration(text: string, style: number): string {
  // R4: underline / overline cells need NBSP so text-decoration paints under
  // trailing spaces. We don't have an overline bit yet — apply for underline.
  if ((style & STYLE_UNDERLINE) === 0) return text;
  return text.replace(/ /g, '\xa0');
}

export const Row = memo(function Row({ cells, fm, hyperlinks, probeRef }: RowProps) {
  const linkSpans = buildLinkSpans(cells, hyperlinks);
  const runs = coalesceCellsWithLinks(cells, linkSpans);
  return (
    <div
      className="block whitespace-pre"
      // biome-ignore lint/plugin: row height from measured cell metrics
      style={{ height: `${fm.cellH}px` }}
    >
      {runs.map((run) => {
        const fgClass = !Array.isArray(run.fg)
          ? colorClass(run.style & STYLE_REVERSE ? run.bg : run.fg, 'fg')
          : '';
        const bgClass = !Array.isArray(run.bg)
          ? colorClass(run.style & STYLE_REVERSE ? run.fg : run.bg, 'bg')
          : '';
        const attrClasses = styleClasses(run.style);
        const inlineStyle: React.CSSProperties = colorInlineStyle(run.fg, run.bg, run.style);
        const text = swapSpacesForDecoration(run.text, run.style);

        if (run.isWide && probeRef) {
          const bold = (run.style & STYLE_BOLD) !== 0;
          const italic = (run.style & STYLE_ITALIC) !== 0;
          const measured = measureGlyph(probeRef, run.text, bold, italic);
          inlineStyle.letterSpacing = `${2 * fm.cellW - measured}px`;
        }

        const isLink = run.linkUri !== undefined && isAllowedUri(run.linkUri);
        if (isLink) {
          return (
            <a
              key={run.startCol}
              href={run.linkUri}
              target="_blank"
              rel="noopener noreferrer"
              className={clsx(
                'inline-block no-underline hover:underline',
                fgClass,
                bgClass,
                attrClasses,
              )}
              // biome-ignore lint/plugin: ANSI/truecolor + DIM derived colors outside theme tokens
              style={inlineStyle}
            >
              {text}
            </a>
          );
        }

        return (
          <span
            key={run.startCol}
            className={clsx('inline-block', fgClass, bgClass, attrClasses)}
            // biome-ignore lint/plugin: ANSI/truecolor + DIM derived colors outside theme tokens
            style={inlineStyle}
          >
            {text}
          </span>
        );
      })}
    </div>
  );
});
