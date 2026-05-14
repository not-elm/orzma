//! Unified pointer listener — fans out pointermove on a single RAF to selection
//! drag (when active) and link hover (when not dragging). See spec § 5.

import type { FontMetrics } from '../renderer/font';
import type { Grid } from '../renderer/grid';
import { shouldRouteToSelection } from './mouse';

/** Half-open at column (head exclusive), inclusive at row. CodeMirror-style
 *  naming: anchor is where the drag began, head is where the pointer is now. */
export interface SelectionRange {
  anchor: { col: number; row: number };
  head: { col: number; row: number };
}

/** Hover state — the underlined run on a single row. */
export interface LinkHover {
  rangeStart: number;
  rangeEnd: number;
  row: number;
  uri: string;
}

interface Span {
  start: number;
  end: number;
  uri: string;
  /** 0 = OSC 8 (wins on overlap), 1 = URL regex fallback. */
  priority: 0 | 1;
}

interface RowHoverCache {
  version: number;
  spans: Span[];
}

// NOTE: xterm.js WebLinksAddon URL regex (parity with the 5.5.0 release). Per-row
// scope only — wrapped lines are deferred to Phase 3D.
const URL_RE = /(https?|HTTPS?):\/\/[^\s"'!*(){}|\\^<>`]*[^\s"':,.!?{}|\\^~[\]`()<>]/g;

function clampHover(col: number, row: number, grid: Grid): { col: number; row: number } {
  return {
    col: Math.max(0, Math.min(grid.cols - 1, col)),
    row: Math.max(0, Math.min(grid.rows - 1, row)),
  };
}

function clampSelectionEndpoint(
  col: number,
  row: number,
  grid: Grid,
): { col: number; row: number } {
  // NOTE: col can equal grid.cols (selection extends past the last column);
  // row is clamped to a valid row index.
  return {
    col: Math.max(0, Math.min(grid.cols, col)),
    row: Math.max(0, Math.min(grid.rows - 1, row)),
  };
}

function pointToCol(canvas: HTMLCanvasElement, ev: { clientX: number }, fm: FontMetrics): number {
  const rect = canvas.getBoundingClientRect();
  return Math.floor((ev.clientX - rect.left) / fm.cellW);
}

function pointToRow(canvas: HTMLCanvasElement, ev: { clientY: number }, fm: FontMetrics): number {
  const rect = canvas.getBoundingClientRect();
  return Math.floor((ev.clientY - rect.top) / fm.cellH);
}

function buildRowSpans(grid: Grid, row: number, hyperlinks: ReadonlyMap<number, string>): Span[] {
  const out: Span[] = [];
  const cells = grid.cells[row];
  if (!cells) return out;

  // OSC 8 spans first: walk visible cells, group adjacent same-hyperlink runs.
  let runningCol = 0;
  let osc: { start: number; uri: string } | null = null;
  for (const cell of cells) {
    if (cell.width === 0) continue;
    const uri = cell.hyperlinkId != null ? hyperlinks.get(cell.hyperlinkId) : undefined;
    if (osc && uri !== osc.uri) {
      out.push({ start: osc.start, end: runningCol, uri: osc.uri, priority: 0 });
      osc = null;
    }
    if (uri && !osc) {
      osc = { start: runningCol, uri };
    }
    runningCol += cell.width;
  }
  if (osc) {
    out.push({ start: osc.start, end: runningCol, uri: osc.uri, priority: 0 });
  }

  // URL regex spans: build a text→column map so regex matches translate back.
  const visible = cells.filter((c) => c.width > 0);
  const rowText = visible.map((c) => c.text).join('');
  const colByTextIdx: number[] = [];
  let runCol = 0;
  for (const cell of visible) {
    for (let i = 0; i < cell.text.length; i++) {
      colByTextIdx.push(runCol);
    }
    runCol += cell.width;
  }

  URL_RE.lastIndex = 0;
  let m: RegExpExecArray | null = URL_RE.exec(rowText);
  while (m !== null) {
    const textStart = m.index;
    const textEnd = m.index + m[0].length;
    const colStart = colByTextIdx[textStart] ?? 0;
    const colEnd = colByTextIdx[textEnd - 1] ?? colStart;
    out.push({ start: colStart, end: colEnd + 1, uri: m[0], priority: 1 });
    m = URL_RE.exec(rowText);
  }

  // OSC wins over regex on overlap; stable order within priority.
  out.sort((a, b) => a.priority - b.priority || a.start - b.start);
  return out;
}

function findSpan(spans: Span[], col: number): Span | null {
  for (const s of spans) {
    if (col >= s.start && col < s.end) return s;
  }
  return null;
}

/** Wires pointer listeners that drive the selection overlay (drag) and link
 *  hover overlay (idle). One RAF per pointermove fans out to both. */
export function setupPointerOverlays(
  target: HTMLElement,
  canvas: HTMLCanvasElement,
  fmRef: { current: FontMetrics },
  modesRef: { current: ReadonlySet<string> },
  gridRef: { current: Grid },
  hyperlinksRef: { current: ReadonlyMap<number, string> },
  setSelection: (next: SelectionRange | null) => void,
  setLinkHover: (next: LinkHover | null) => void,
): () => void {
  let dragPointerId: number | null = null;
  let dragAnchor: { col: number; row: number } | null = null;
  let rafScheduled = false;
  let pendingMove: { clientX: number; clientY: number } | null = null;
  let lastHoverCell: { col: number; row: number } | null = null;
  const hoverCache = new Map<number, RowHoverCache>();

  function onPointerDown(e: PointerEvent): void {
    if (e.button < 0 || e.button > 2) return;
    if (!shouldRouteToSelection(e, modesRef.current)) return;
    const grid = gridRef.current;
    const fm = fmRef.current;
    const endpoint = clampSelectionEndpoint(
      pointToCol(canvas, e, fm),
      pointToRow(canvas, e, fm),
      grid,
    );
    dragAnchor = endpoint;
    dragPointerId = e.pointerId;
    setSelection({ anchor: endpoint, head: endpoint });
    try {
      target.setPointerCapture(e.pointerId);
    } catch {
      // NOTE: setPointerCapture throws NotFoundError if the pointer is no
      // longer active; ignore.
    }
  }

  function flushMove(): void {
    const ev = pendingMove;
    pendingMove = null;
    if (!ev) return;
    const grid = gridRef.current;
    const fm = fmRef.current;

    if (dragAnchor) {
      const head = clampSelectionEndpoint(
        pointToCol(canvas, ev, fm),
        pointToRow(canvas, ev, fm),
        grid,
      );
      setSelection({ anchor: dragAnchor, head });
      return;
    }

    const { col, row } = clampHover(pointToCol(canvas, ev, fm), pointToRow(canvas, ev, fm), grid);
    if (lastHoverCell && lastHoverCell.col === col && lastHoverCell.row === row) return;
    lastHoverCell = { col, row };

    const version = grid.rowVersions[row] ?? 0;
    let cache = hoverCache.get(row);
    if (!cache || cache.version !== version) {
      cache = { version, spans: buildRowSpans(grid, row, hyperlinksRef.current) };
      hoverCache.set(row, cache);
    }
    const span = findSpan(cache.spans, col);
    setLinkHover(span ? { rangeStart: span.start, rangeEnd: span.end, row, uri: span.uri } : null);
  }

  function onPointerMove(e: PointerEvent): void {
    pendingMove = { clientX: e.clientX, clientY: e.clientY };
    if (rafScheduled) return;
    rafScheduled = true;
    requestAnimationFrame(() => {
      rafScheduled = false;
      flushMove();
    });
  }

  function onPointerUp(e: PointerEvent): void {
    if (dragPointerId !== null && e.pointerId === dragPointerId) {
      dragPointerId = null;
      dragAnchor = null;
      try {
        target.releasePointerCapture(e.pointerId);
      } catch {
        // benign: pointer already released
      }
    }
  }

  function onPointerCancel(e: PointerEvent): void {
    if (dragPointerId !== null && e.pointerId === dragPointerId) {
      dragPointerId = null;
      dragAnchor = null;
    }
  }

  function onPointerLeave(): void {
    if (dragAnchor === null) {
      setLinkHover(null);
      lastHoverCell = null;
    }
  }

  target.addEventListener('pointerdown', onPointerDown);
  target.addEventListener('pointermove', onPointerMove);
  target.addEventListener('pointerup', onPointerUp);
  target.addEventListener('pointercancel', onPointerCancel);
  target.addEventListener('pointerleave', onPointerLeave);

  return () => {
    target.removeEventListener('pointerdown', onPointerDown);
    target.removeEventListener('pointermove', onPointerMove);
    target.removeEventListener('pointerup', onPointerUp);
    target.removeEventListener('pointercancel', onPointerCancel);
    target.removeEventListener('pointerleave', onPointerLeave);
    hoverCache.clear();
  };
}
