// NOTE: Phase 2B uses direct ctx.fillText per cell (no glyph atlas). The atlas
// optimization (spec § 5) is deferred to Phase 3 — a fillText-per-cell paint
// of 80×24 dirty rows fits comfortably inside one 60Hz frame budget.

import { colorToCss } from './colors';
import type { FontMetrics } from './font';
import type { Cell, Grid } from './grid';

const DEFAULT_FG = '#e5e5e5';
/** Terminal canvas background color. Exported so the hook can pre-fill the
 * canvas on resize and avoid exposing the parent pane's bg through transparent
 * pixels. */
export const DEFAULT_BG = '#0a0a0a';

const STYLE_BOLD = 1;
const STYLE_ITALIC = 2;
const STYLE_UNDERLINE = 4;
const STYLE_STRIKE = 8;
const STYLE_REVERSE = 16;
const STYLE_DIM = 32;

const CURSOR_COLOR_VAR_ACTIVE = '--color-tmux-cursor';
const CURSOR_COLOR_VAR_INACTIVE = '--color-tmux-cursor-inactive';
const CURSOR_FALLBACK_ACTIVE = '#ffffff';
const CURSOR_FALLBACK_INACTIVE = '#e5e5e5';

/** Per-paint options that vary independently of the grid. */
export interface PaintOptions {
  isActive: boolean;
}

/** Canvas2D-based terminal renderer with dirty-row repainting. */
export interface CanvasRenderer {
  paint: (grid: Grid, opts: PaintOptions) => void;
}

/** Creates a renderer bound to the given canvas + font metrics. */
export function createCanvasRenderer(canvas: HTMLCanvasElement, fm: FontMetrics): CanvasRenderer {
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    throw new Error('createCanvasRenderer: getContext("2d") returned null');
  }
  ctx.font = fm.fontCss;
  ctx.textBaseline = 'alphabetic';

  let lastCursorRow: number | null = null;
  return {
    paint(grid: Grid, opts: PaintOptions) {
      // NOTE: ensure both the previous cursor row and the current one repaint so
      // a moved cursor leaves no ghost. Phase 3 will replace this with a
      // dedicated <Cursor> overlay (spec § 7).
      if (lastCursorRow !== null) grid.dirtyRows.add(lastCursorRow);
      if (grid.cursor.visible) grid.dirtyRows.add(grid.cursor.y);
      for (const row of grid.dirtyRows) {
        paintRow(ctx, grid.cells[row] ?? [], row, fm, grid.cols);
      }
      grid.dirtyRows.clear();
      drawCursor(ctx, canvas, grid, fm, opts.isActive);
      lastCursorRow = grid.cursor.visible ? grid.cursor.y : null;
    },
  };
}

function drawCursor(
  ctx: CanvasRenderingContext2D,
  canvas: HTMLCanvasElement,
  grid: Grid,
  fm: FontMetrics,
  isActive: boolean,
): void {
  if (!grid.cursor.visible) return;
  const px = grid.cursor.x * fm.cellW;
  const py = grid.cursor.y * fm.cellH;
  ctx.fillStyle = resolveCursorColor(canvas, isActive);
  ctx.globalAlpha = isActive ? 1 : 0.6;
  ctx.fillRect(px, py, fm.cellW, fm.cellH);
  ctx.globalAlpha = 1;
}

function resolveCursorColor(canvas: HTMLCanvasElement, isActive: boolean): string {
  const varName = isActive ? CURSOR_COLOR_VAR_ACTIVE : CURSOR_COLOR_VAR_INACTIVE;
  const fallback = isActive ? CURSOR_FALLBACK_ACTIVE : CURSOR_FALLBACK_INACTIVE;
  const target = canvas.parentElement ?? canvas;
  const resolved = getComputedStyle(target).getPropertyValue(varName).trim();
  return resolved.length > 0 ? resolved : fallback;
}

function paintRow(
  ctx: CanvasRenderingContext2D,
  cells: readonly Cell[],
  row: number,
  fm: FontMetrics,
  cols: number,
): void {
  const y = row * fm.cellH;
  // NOTE: fill (not clearRect) the entire row width covered by the canvas,
  // not just `cols * fm.cellW`. clearRect leaves transparent pixels that show
  // the parent pane's bg through; and during a resize-in-flight the canvas
  // CSS width can exceed grid.cols * cellW briefly. fill_with DEFAULT_BG
  // guarantees a clean terminal background everywhere.
  const cssW = ctx.canvas.width / ctx.getTransform().a;
  const rowW = Math.max(cols * fm.cellW, cssW);
  ctx.fillStyle = DEFAULT_BG;
  ctx.fillRect(0, y, rowW, fm.cellH);
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
