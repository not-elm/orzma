import { type RefObject, useCallback, useEffect, useReducer, useRef } from 'react';
import type { SessionId, WindowId } from '../layout/types';
import { windowSelect } from '../statusbar/windowSelect';
import { flattenVisibleRows, type VisibleRow } from './flattenVisibleRows';
import { activeRowKey } from './TreeView';
import { keyToAction } from './treeKeymap';
import {
  initialTreeState,
  type TreeAction,
  type TreeCursor,
  type TreeState,
  treeReducer,
} from './treeReducer';
import type { SessionTreeNode, SessionTreeState } from './types';
import { useSessionTree } from './useSessionTree';

interface UseChooseTreeControllerProps {
  onClose: () => void;
  attachedSessionId: SessionId | null;
  setAttachedSession: (sid: SessionId) => void;
}

/** Everything the overlay JSX needs to render — derived state plus a focus ref and pointer handler. */
export interface UseChooseTreeControllerResult {
  treeState: SessionTreeState;
  rows: VisibleRow[];
  cursor: TreeCursor;
  activeRowId: string | undefined;
  sessionCount: number;
  windowCount: number;
  rootRef: RefObject<HTMLDivElement | null>;
  onBackdropPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
  onRowClick: (cursor: TreeCursor) => void;
}

/**
 * Owns the imperative side of the choose-tree picker:
 *
 * - Fetches the session tree
 * - Drives the reducer for cursor / expanded state
 * - Wires the native `keydown` listener with IME / repeat guards
 * - Routes confirm actions through `setAttachedSession` + `windowSelect`,
 *   keeping the overlay open on failure
 * - Captures and restores focus around the overlay's lifetime
 *
 * Returns only what the JSX needs. Latest values of `onClose`,
 * `attachedSessionId`, and `setAttachedSession` are read through refs
 * so the keydown listener doesn't re-attach on every render.
 */
export function useChooseTreeController({
  onClose,
  attachedSessionId,
  setAttachedSession,
}: UseChooseTreeControllerProps): UseChooseTreeControllerResult {
  const treeState = useSessionTree(true);
  const tree: SessionTreeNode[] = treeState.status === 'ready' ? treeState.tree : [];

  const treeRef = useRef<SessionTreeNode[]>(tree);
  treeRef.current = tree;

  const reducer = useCallback(
    (s: TreeState, action: TreeAction) => treeReducer(s, action, treeRef.current),
    [],
  );
  const [state, dispatch] = useReducer(reducer, null, () =>
    initialTreeState([], attachedSessionId),
  );

  const stateRef = useRef<TreeState>(state);
  stateRef.current = state;

  const attachedSessionIdRef = useRef<SessionId | null>(attachedSessionId);
  attachedSessionIdRef.current = attachedSessionId;

  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const setAttachedSessionRef = useRef(setAttachedSession);
  setAttachedSessionRef.current = setAttachedSession;

  const returnFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    returnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      const prev = returnFocusRef.current;
      if (prev && document.contains(prev)) prev.focus();
    };
  }, []);

  useEffect(() => {
    if (treeState.status === 'ready') {
      if (treeState.tree.length === 0) {
        console.warn('choose-tree: no sessions available; closing picker');
        onCloseRef.current();
        return;
      }
      dispatch({ type: 'tree-reloaded', tree: treeState.tree, attachedSessionId });
    }
  }, [treeState, attachedSessionId]);

  const rootRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    rootRef.current?.focus();
  }, []);

  const attachAndSelect = useCallback(async (sid: SessionId, wid: WindowId) => {
    if (sid !== attachedSessionIdRef.current) setAttachedSessionRef.current(sid);
    const ok = await windowSelect(wid);
    if (ok) onCloseRef.current();
  }, []);

  const confirmCursor = useCallback(
    async (cursor: TreeCursor) => {
      const t = treeRef.current;
      if (cursor.kind === 'session') {
        const session = t.find((s) => s.id === cursor.sessionId);
        if (!session) return;
        if (!stateRef.current.expanded.has(session.id)) {
          dispatch({ type: 'expand' });
          return;
        }
        const target = session.active_window ?? session.windows[0]?.id;
        if (!target) return;
        await attachAndSelect(session.id, target);
        return;
      }
      await attachAndSelect(cursor.sessionId, cursor.windowId);
    },
    [attachAndSelect],
  );

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const handler = (e: KeyboardEvent) => {
      if (e.isComposing || e.repeat) return;
      const resolved = keyToAction(e);
      if (!resolved) return;
      e.preventDefault();
      e.stopPropagation();
      if (resolved.type === 'cancel') {
        onCloseRef.current();
        return;
      }
      const isExpandActingAsConfirm =
        resolved.type === 'expand' &&
        (stateRef.current.cursor.kind === 'window' ||
          stateRef.current.expanded.has(stateRef.current.cursor.sessionId));
      if (resolved.type === 'confirm' || isExpandActingAsConfirm) {
        void confirmCursor(stateRef.current.cursor);
        return;
      }
      dispatch(resolved);
    };
    root.addEventListener('keydown', handler);
    return () => root.removeEventListener('keydown', handler);
  }, [confirmCursor]);

  const onRowClick = useCallback(
    (cursor: TreeCursor) => dispatch({ type: 'set-cursor', cursor }),
    [],
  );

  const onBackdropPointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.target === e.currentTarget) onCloseRef.current();
  }, []);

  const rows = flattenVisibleRows(tree, state.expanded);
  const activeRowId = activeRowKey(rows, state.cursor);
  const sessionCount = tree.length;
  const windowCount = tree.reduce((acc, s) => acc + s.windows.length, 0);

  return {
    treeState,
    rows,
    cursor: state.cursor,
    activeRowId,
    sessionCount,
    windowCount,
    rootRef,
    onBackdropPointerDown,
    onRowClick,
  };
}
