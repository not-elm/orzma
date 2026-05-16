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

function walk(node: WindowLayoutNode, bounds: Bounds, out: PaneLayout): void {
  switch (node.type) {
    case 'pane':
      out.panes.set(node.pane_id, bounds);
      return;
    case 'root':
      walk(node.child, bounds, out);
      return;
    case 'split': {
      const ratio = normalizeRatio(node.split_ratio);
      if (node.orientation === 'horizontal') {
        const lhsW = bounds.w * ratio;
        walk(node.lhs, { x: bounds.x, y: bounds.y, w: lhsW, h: bounds.h }, out);
        walk(node.rhs, { x: bounds.x + lhsW, y: bounds.y, w: bounds.w - lhsW, h: bounds.h }, out);
      } else {
        const lhsH = bounds.h * ratio;
        walk(node.lhs, { x: bounds.x, y: bounds.y, w: bounds.w, h: lhsH }, out);
        walk(node.rhs, { x: bounds.x, y: bounds.y + lhsH, w: bounds.w, h: bounds.h - lhsH }, out);
      }
      return;
    }
    default: {
      const u = node as unknown as { type: string; cell_id: CellId };
      out.unknown.push({ cell_id: u.cell_id, type: u.type, bounds });
      return;
    }
  }
}
