import type { SessionId, WindowId } from '../layout/types';

/** A single window entry within a session tree node. */
export interface SessionTreeWindow {
  id: WindowId;
  name: string;
  index: number;
}

/** A session with its windows, as returned by `GET /sessions/tree`. */
export interface SessionTreeNode {
  id: SessionId;
  name: string;
  active_window: WindowId | null;
  windows: SessionTreeWindow[];
}

/** Async state for the session tree fetch. */
export type SessionTreeState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; tree: SessionTreeNode[] };
