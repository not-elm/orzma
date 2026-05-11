import { describe, expect, it } from 'vitest';
import { computePaneLayout } from './paneBounds';
import type { WindowLayoutNode } from './types';

describe('computePaneLayout', () => {
  it('places a single pane at full bounds under root', () => {
    const node: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: { type: 'pane', cell_id: 'c', pane_id: 'p1' },
    };
    const { panes, unknown } = computePaneLayout(node);
    expect(panes.get('p1')).toEqual({ x: 0, y: 0, w: 100, h: 100 });
    expect(unknown).toEqual([]);
  });

  it('splits a horizontal pair by split_ratio', () => {
    const node: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's',
        orientation: 'horizontal',
        split_ratio: 0.7,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
      },
    };
    const { panes } = computePaneLayout(node);
    expect(panes.get('pa')).toEqual({ x: 0, y: 0, w: 70, h: 100 });
    expect(panes.get('pb')).toEqual({ x: 70, y: 0, w: 30, h: 100 });
  });

  it('splits a vertical pair by split_ratio', () => {
    const node: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's',
        orientation: 'vertical',
        split_ratio: 0.4,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
      },
    };
    const { panes } = computePaneLayout(node);
    expect(panes.get('pa')).toEqual({ x: 0, y: 0, w: 100, h: 40 });
    expect(panes.get('pb')).toEqual({ x: 0, y: 40, w: 100, h: 60 });
  });

  it('handles nested splits', () => {
    const node: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's1',
        orientation: 'horizontal',
        split_ratio: 0.5,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: {
          type: 'split',
          cell_id: 's2',
          orientation: 'vertical',
          split_ratio: 0.5,
          lhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
          rhs: { type: 'pane', cell_id: 'c', pane_id: 'pc' },
        },
      },
    };
    const { panes } = computePaneLayout(node);
    expect(panes.get('pa')).toEqual({ x: 0, y: 0, w: 50, h: 100 });
    expect(panes.get('pb')).toEqual({ x: 50, y: 0, w: 50, h: 50 });
    expect(panes.get('pc')).toEqual({ x: 50, y: 50, w: 50, h: 50 });
  });

  it('clamps split_ratio above 1', () => {
    const tooBig: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's',
        orientation: 'horizontal',
        split_ratio: 1.5,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
      },
    };
    const result = computePaneLayout(tooBig);
    expect(result.panes.get('pa')!.w).toBeCloseTo(100);
    expect(result.panes.get('pb')!.w).toBeCloseTo(0);
  });

  it('clamps split_ratio below 0', () => {
    const negative: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's',
        orientation: 'horizontal',
        split_ratio: -0.3,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
      },
    };
    const result = computePaneLayout(negative);
    expect(result.panes.get('pa')!.w).toBeCloseTo(0);
    expect(result.panes.get('pb')!.w).toBeCloseTo(100);
  });

  it('falls back to 0.5 when split_ratio is NaN', () => {
    const node: WindowLayoutNode = {
      type: 'root',
      cell_id: 'r',
      child: {
        type: 'split',
        cell_id: 's',
        orientation: 'horizontal',
        split_ratio: Number.NaN,
        lhs: { type: 'pane', cell_id: 'a', pane_id: 'pa' },
        rhs: { type: 'pane', cell_id: 'b', pane_id: 'pb' },
      },
    };
    const { panes } = computePaneLayout(node);
    expect(panes.get('pa')!.w).toBeCloseTo(50);
    expect(panes.get('pb')!.w).toBeCloseTo(50);
  });

  it('collects unknown node types into the unknown list', () => {
    const node = {
      type: 'root',
      cell_id: 'r',
      child: { type: 'mystery_node', cell_id: 'm' },
    } as unknown as WindowLayoutNode;
    const { panes, unknown } = computePaneLayout(node);
    expect(panes.size).toBe(0);
    expect(unknown).toHaveLength(1);
    expect(unknown[0].type).toBe('mystery_node');
    expect(unknown[0].cell_id).toBe('m');
    expect(unknown[0].bounds).toEqual({ x: 0, y: 0, w: 100, h: 100 });
  });
});
