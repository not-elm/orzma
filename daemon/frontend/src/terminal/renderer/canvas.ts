// NOTE: Phase 2B uses direct ctx.fillText per cell (no glyph atlas). The atlas
// optimization (spec § 5) is deferred to Phase 3 — a fillText-per-cell paint
// of 80×24 dirty rows fits comfortably inside one 60Hz frame budget.

import { colorToCss } from './colors';
import type { FontMetrics } from './font';
import type { Cell, Grid } from './grid';

const DEFAULT_FG = '#e5e5e5';
const DEFAULT_BG = '#0a0a0a';

const STYLE_BOLD = 1;
const STYLE_ITALIC = 2;
const STYLE_UNDERLINE = 4;
const STYLE_STRIKE = 8;
const STYLE_REVERSE = 16;
const STYLE_DIM = 32;

/** Canvas2D-based terminal renderer with dirty-row repainting. */
export interface CanvasRenderer {
  paint: (grid: Grid) => void;
}

/** Creates a renderer bound to the given canvas + font metrics. */
export function createCanvasRenderer(canvas: HTMLCanvasElement, fm: FontMetrics): CanvasRenderer {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    throw new Error('createCanvasRenderer: getContext("2d") returned null');
  }
  ctx.font = fm.fontCss;
  ctx.textBaseline = 'alphabetic';

  return {
    paint(grid: Grid) {
      for (const row of grid.dirtyRows) {
        paintRow(ctx, grid.cells[row] ?? [], row, fm, grid.cols);
      }
      grid.dirtyRows.clear();
    },
  };
}

function paintRow(
  ctx: CanvasRenderingContext2D,
  cells: readonly Cell[],
  row: number,
  fm: FontMetrics,
  cols: number,
): void {
  const y = row * fm.cellH;
  ctx.clearRect(0, y, cols * fm.cellW, fm.cellH);
  let xCell = 0;
  for (const cell of cells) {
    if (cell.width === 0) continue; // combining marks: drawn as part of prev cluster
    if (xCell >= cols) break;
    drawCell(ctx, cell, xCell * fm.cellW, y, fm);
    xCell += cell.width;
  }
}

function drawCell(
  ctx: CanvasRenderingContext2D,
  cell: Cell,
  px: number,
  py: number,
  fm: FontMetrics,
): void {
  const reverse = (cell.style & STYLE_REVERSE) !== 0;
  const fgCss = colorToCss(cell.fg, 'fg') ?? DEFAULT_FG;
  const bgCss = colorToCss(cell.bg, 'bg') ?? DEFAULT_BG;
  const fillFg = reverse ? bgCss : fgCss;
  const fillBg = reverse ? fgCss : bgCss;

  // Background
  ctx.fillStyle = fillBg;
  ctx.fillRect(px, py, fm.cellW * cell.width, fm.cellH);

  // Glyph
  if (cell.text && cell.text !== ' ') {
    ctx.fillStyle = fillFg;
    ctx.font = fontForStyle(cell.style, fm.fontCss);
    if ((cell.style & STYLE_DIM) !== 0) {
      ctx.fillStyle = applyDim(fillFg);
    }
    ctx.fillText(cell.text, px, py + fm.baseline);
  }

  // Underline / strike
  if ((cell.style & STYLE_UNDERLINE) !== 0) {
    ctx.fillStyle = fillFg;
    ctx.fillRect(px, py + fm.cellH - 2, fm.cellW * cell.width, 1);
  }
  if ((cell.style & STYLE_STRIKE) !== 0) {
    ctx.fillStyle = fillFg;
    ctx.fillRect(px, py + Math.floor(fm.cellH / 2), fm.cellW * cell.width, 1);
  }
}

function fontForStyle(style: number, base: string): string {
  const bold = (style & STYLE_BOLD) !== 0;
  const italic = (style & STYLE_ITALIC) !== 0;
  if (!bold && !italic) return base;
  const prefix = `${italic ? 'italic ' : ''}${bold ? 'bold ' : ''}`;
  return `${prefix}${base}`;
}

/** Approximates dim by appending an alpha byte to a 7-char hex. */
function applyDim(cssHex: string): string {
  return cssHex.length === 7 ? `${cssHex}99` : cssHex;
}
