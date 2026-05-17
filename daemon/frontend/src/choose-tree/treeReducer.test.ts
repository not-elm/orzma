import { describe, expect, it } from 'vitest';
import { initialTreeState, type TreeState, treeReducer } from './treeReducer';
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

describe('treeReducer', () => {
  it('initialTreeState places cursor on attached session active window when present', () => {
    const s = initialTreeState(tree, 'sid-a');
    expect(s.cursor).toEqual({ kind: 'window', sessionId: 'sid-a', windowId: 'wid-a1' });
    expect(s.expanded.has('sid-a')).toBe(true);
    expect(s.expanded.has('sid-b')).toBe(true);
  });

  it('initialTreeState falls back to first window of attached session when active_window missing', () => {
    const s = initialTreeState(tree, 'sid-b');
    expect(s.cursor).toEqual({ kind: 'window', sessionId: 'sid-b', windowId: 'wid-b0' });
  });

  it('move down skips collapsed children', () => {
    const start: TreeState = {
      cursor: { kind: 'session', sessionId: 'sid-a' },
      expanded: new Set(),
    };
    const next = treeReducer(start, { type: 'move', direction: 'down' }, tree);
    expect(next.cursor).toEqual({ kind: 'session', sessionId: 'sid-b' });
  });

  it('move down enters expanded children', () => {
    const start: TreeState = {
      cursor: { kind: 'session', sessionId: 'sid-a' },
      expanded: new Set(['sid-a']),
    };
    const next = treeReducer(start, { type: 'move', direction: 'down' }, tree);
    expect(next.cursor).toEqual({ kind: 'window', sessionId: 'sid-a', windowId: 'wid-a0' });
  });

  it('expand on a session row adds to expanded with a NEW Set', () => {
    const start: TreeState = {
      cursor: { kind: 'session', sessionId: 'sid-a' },
      expanded: new Set(),
    };
    const next = treeReducer(start, { type: 'expand' }, tree);
    expect(next.expanded.has('sid-a')).toBe(true);
    expect(next.expanded).not.toBe(start.expanded);
  });

  it('collapse on a window row moves cursor up to its parent session', () => {
    const start: TreeState = {
      cursor: { kind: 'window', sessionId: 'sid-a', windowId: 'wid-a1' },
      expanded: new Set(['sid-a', 'sid-b']),
    };
    const next = treeReducer(start, { type: 'collapse' }, tree);
    expect(next.cursor).toEqual({ kind: 'session', sessionId: 'sid-a' });
  });

  it('tree-reloaded normalises cursor when target row no longer exists', () => {
    const start: TreeState = {
      cursor: { kind: 'window', sessionId: 'sid-a', windowId: 'gone' as never },
      expanded: new Set(['sid-a']),
    };
    const next = treeReducer(
      start,
      { type: 'tree-reloaded', tree, attachedSessionId: 'sid-a' },
      tree,
    );
    if (next.cursor.kind === 'window') {
      const w = tree
        .find((s) => s.id === next.cursor.sessionId)
        ?.windows.find(
          (w) => w.id === (next.cursor as Extract<typeof next.cursor, { kind: 'window' }>).windowId,
        );
      expect(w).toBeDefined();
    } else {
      expect(['sid-a', 'sid-b']).toContain(next.cursor.sessionId);
    }
  });
});
