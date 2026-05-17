import { describe, expect, it } from 'vitest';
import { flattenVisibleRows, type VisibleRow } from './flattenVisibleRows';
import type { SessionTreeNode } from './types';

const tree: SessionTreeNode[] = [
  {
    id: 'sid-a',
    name: 'work',
    active_window: 'wid-a1',
    windows: [
      { id: 'wid-a0', name: 'build', index: 0 },
      { id: 'wid-a1', name: 'main', index: 1 },
    ],
  },
  {
    id: 'sid-b',
    name: 'experiments',
    active_window: null,
    windows: [{ id: 'wid-b0', name: 'play', index: 0 }],
  },
];

describe('flattenVisibleRows', () => {
  it('emits only session rows when no session is expanded', () => {
    const rows = flattenVisibleRows(tree, new Set<string>());
    expect(rows.map((r) => r.kind)).toEqual(['session', 'session']);
    expect((rows[0] as Extract<VisibleRow, { kind: 'session' }>).sessionId).toBe('sid-a');
  });

  it('emits window rows under expanded sessions', () => {
    const rows = flattenVisibleRows(tree, new Set(['sid-a']));
    expect(rows.map((r) => r.kind)).toEqual(['session', 'window', 'window', 'session']);
  });

  it('emits all rows when all sessions expanded', () => {
    const rows = flattenVisibleRows(tree, new Set(['sid-a', 'sid-b']));
    expect(rows.map((r) => r.kind)).toEqual(['session', 'window', 'window', 'session', 'window']);
  });
});
