//! Row component for the DOM renderer. Renders one terminal row as a
//! pointer-events-none <div> containing inline-block <span> runs (or <a>
//! for OSC 8 / URL regex matches — added in Task 7). Wide chars get
//! per-span letterSpacing correction. Underline/overline runs swap space
//! → NBSP so text-decoration paints.

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
}

function coalesceCells(cells: readonly Cell[]): Run[] {
  const runs: Run[] = [];
  let col = 0;
  for (const cell of cells) {
    if (cell.width === 0) continue;
    const isWide = cell.width === 2;
    const last = runs[runs.length - 1];
    const sameAttrs =
      last !== undefined &&
      last.style === cell.style &&
      last.fg === cell.fg &&
      last.bg === cell.bg &&
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

export const Row = memo(function Row({ cells, fm, probeRef }: RowProps) {
  const runs = coalesceCells(cells);
  return (
    <div
      className="block whitespace-pre pointer-events-none"
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
