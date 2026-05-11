import type { CellId, PaneId, WindowLayoutNode } from './types';

export interface Bounds {
  x: number; // percentage 0..100
  y: number;
  w: number;
  h: number;
}

export interface UnknownNode {
  cell_id: CellId;
  type: string;
  bounds: Bounds;
}

export interface PaneLayout {
  panes: Map<PaneId, Bounds>;
  unknown: UnknownNode[];
}

function normalizeRatio(r: number): number {
  if (!Number.isFinite(r)) return 0.5;
  return Math.max(0, Math.min(1, r));
}

export function computePaneLayout(
  node: WindowLayoutNode,
  bounds: Bounds = { x: 0, y: 0, w: 100, h: 100 },
): PaneLayout {
  const out: PaneLayout = { panes: new Map(), unknown: [] };
  walk(node, bounds, out);
  return out;
}

function walk(node: WindowLayoutNode, b: Bounds, out: PaneLayout): void {
  switch (node.type) {
    case 'pane':
      out.panes.set(node.pane_id, b);
      return;
    case 'root':
      walk(node.child, b, out);
      return;
    case 'split': {
      const ratio = normalizeRatio(node.split_ratio);
      if (node.orientation === 'horizontal') {
        const lhsW = b.w * ratio;
        walk(node.lhs, { x: b.x, y: b.y, w: lhsW, h: b.h }, out);
        walk(node.rhs, { x: b.x + lhsW, y: b.y, w: b.w - lhsW, h: b.h }, out);
      } else {
        const lhsH = b.h * ratio;
        walk(node.lhs, { x: b.x, y: b.y, w: b.w, h: lhsH }, out);
        walk(node.rhs, { x: b.x, y: b.y + lhsH, w: b.w, h: b.h - lhsH }, out);
      }
      return;
    }
    default: {
      const u = node as unknown as { type: string; cell_id: CellId };
      out.unknown.push({ cell_id: u.cell_id, type: u.type, bounds: b });
      return;
    }
  }
}
