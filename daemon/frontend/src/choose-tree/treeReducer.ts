import type { SessionId, WindowId } from '../layout/types';
import { flattenVisibleRows, type VisibleRow } from './flattenVisibleRows';
import type { SessionTreeNode } from './types';

export type TreeCursor =
  | { kind: 'session'; sessionId: SessionId }
  | { kind: 'window'; sessionId: SessionId; windowId: WindowId };

export interface TreeState {
  cursor: TreeCursor;
  expanded: ReadonlySet<SessionId>;
}

export type TreeAction =
  | { type: 'move'; direction: 'up' | 'down' }
  | { type: 'expand' }
  | { type: 'collapse' }
  | { type: 'set-cursor'; cursor: TreeCursor }
  | { type: 'tree-reloaded'; tree: SessionTreeNode[]; attachedSessionId: SessionId | null };

/**
 * Tests whether a visible row matches the given cursor. Exported so the
 * `TreeView` selection rendering uses the same predicate the reducer
 * relies on for navigation; keeping both call sites in lockstep avoids
 * a class of "selected row does not match the row the cursor lands on"
 * bugs if the cursor shape ever grows.
 */
export function rowMatches(row: VisibleRow, cursor: TreeCursor): boolean {
  if (cursor.kind === 'session')
    return row.kind === 'session' && row.sessionId === cursor.sessionId;
  return (
    row.kind === 'window' && row.sessionId === cursor.sessionId && row.windowId === cursor.windowId
  );
}

function cursorFromRow(row: VisibleRow): TreeCursor {
  if (row.kind === 'session') return { kind: 'session', sessionId: row.sessionId };
  return { kind: 'window', sessionId: row.sessionId, windowId: row.windowId };
}

function resolveInitialCursor(
  tree: SessionTreeNode[],
  attachedSessionId: SessionId | null,
): TreeCursor {
  const attached = attachedSessionId ? tree.find((s) => s.id === attachedSessionId) : undefined;
  if (attached) {
    if (attached.active_window) {
      return { kind: 'window', sessionId: attached.id, windowId: attached.active_window };
    }
    const first = attached.windows[0];
    if (first) return { kind: 'window', sessionId: attached.id, windowId: first.id };
    return { kind: 'session', sessionId: attached.id };
  }
  const firstSession = tree[0];
  if (!firstSession) return { kind: 'session', sessionId: '' as SessionId };
  return { kind: 'session', sessionId: firstSession.id };
}

/** Computes a sensible starting cursor for a freshly-opened picker. */
export function initialTreeState(
  tree: SessionTreeNode[],
  attachedSessionId: SessionId | null,
): TreeState {
  const expanded = new Set(tree.map((s) => s.id));
  const cursor = resolveInitialCursor(tree, attachedSessionId);
  return { cursor, expanded };
}

/** Pure reducer for the session-tree picker state machine. */
export function treeReducer(
  state: TreeState,
  action: TreeAction,
  tree: SessionTreeNode[],
): TreeState {
  switch (action.type) {
    case 'move': {
      const rows = flattenVisibleRows(tree, state.expanded);
      const idx = rows.findIndex((r) => rowMatches(r, state.cursor));
      if (idx === -1) return state;
      const next = action.direction === 'down' ? idx + 1 : idx - 1;
      if (next < 0 || next >= rows.length) return state;
      const targetRow = rows[next];
      if (!targetRow) return state;
      return { ...state, cursor: cursorFromRow(targetRow) };
    }
    case 'expand': {
      if (state.cursor.kind !== 'session') return state;
      if (state.expanded.has(state.cursor.sessionId)) return state;
      const next = new Set(state.expanded);
      next.add(state.cursor.sessionId);
      return { ...state, expanded: next };
    }
    case 'collapse': {
      if (state.cursor.kind === 'window') {
        return { ...state, cursor: { kind: 'session', sessionId: state.cursor.sessionId } };
      }
      if (!state.expanded.has(state.cursor.sessionId)) return state;
      const next = new Set(state.expanded);
      next.delete(state.cursor.sessionId);
      return { ...state, expanded: next };
    }
    case 'set-cursor':
      return { ...state, cursor: action.cursor };
    case 'tree-reloaded': {
      const newSessions = action.tree.map((s) => s.id);
      const expanded = new Set([...state.expanded, ...newSessions]);
      const rows = flattenVisibleRows(action.tree, expanded);
      const stillExists = rows.some((r) => rowMatches(r, state.cursor));
      const cursor = stillExists
        ? state.cursor
        : resolveInitialCursor(action.tree, action.attachedSessionId);
      return { cursor, expanded };
    }
    default: {
      const _exhaustive: never = action;
      void _exhaustive;
      return state;
    }
  }
}
