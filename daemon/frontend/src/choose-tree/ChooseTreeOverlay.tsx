import { useCallback, useEffect, useReducer, useRef } from 'react';
import type { SessionId, WindowId } from '../layout/types';
import { windowSelect } from '../statusbar/windowSelect';
import { TreeView } from './TreeView';
import { keyToAction } from './treeKeymap';
import {
  initialTreeState,
  type TreeAction,
  type TreeCursor,
  type TreeState,
  treeReducer,
} from './treeReducer';
import type { SessionTreeNode } from './types';
import { useSessionTree } from './useSessionTree';

interface ChooseTreeOverlayProps {
  onClose: () => void;
  attachedSessionId: SessionId | null;
  setAttachedSession: (sid: SessionId) => void;
}

/**
 * Center-modal tree picker. Owns its own cursor / expanded state via
 * `treeReducer`, listens for native keydown on its root, and routes
 * confirm actions through `windowSelect`. On a windowSelect failure
 * the overlay stays open so the user can retry.
 */
export function ChooseTreeOverlay({
  onClose,
  attachedSessionId,
  setAttachedSession,
}: ChooseTreeOverlayProps) {
  const treeState = useSessionTree(true);
  const tree: SessionTreeNode[] = treeState.status === 'ready' ? treeState.tree : [];

  // NOTE: refs track the latest values without re-running the keydown effect;
  // useReducer captures its reducer at mount, so the reducer reads `tree` and
  // friends through these refs instead of a stale closure.
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

  // Capture previously focused element so we can restore on close.
  const returnFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => {
    returnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => {
      const prev = returnFocusRef.current;
      if (prev && document.contains(prev)) prev.focus();
    };
  }, []);

  // When the tree finishes loading or reloads, normalise the cursor against
  // the new tree.
  useEffect(() => {
    if (treeState.status === 'ready') {
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
    // NOTE: on failure leave the overlay open so the user can retry.
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
      if (resolved.type === 'confirm') {
        void confirmCursor(stateRef.current.cursor);
        return;
      }
      dispatch(resolved);
    };
    root.addEventListener('keydown', handler);
    return () => root.removeEventListener('keydown', handler);
  }, [confirmCursor]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Sessions and windows"
      ref={rootRef}
      tabIndex={-1}
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/60 outline-none"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onCloseRef.current();
      }}
    >
      {/* biome-ignore lint/plugin: modal sizing must be viewport-relative; no semantic token exists */}
      <div className="max-h-[75vh] w-[70vw] max-w-3xl overflow-auto rounded-md border border-border bg-popover p-3 shadow-xl">
        {treeState.status === 'loading' && (
          <div className="p-2 text-muted-foreground">Loading…</div>
        )}
        {treeState.status === 'error' && (
          <div className="p-2 text-destructive">Failed to load sessions: {treeState.message}</div>
        )}
        {treeState.status === 'ready' && (
          <TreeView
            tree={tree}
            expanded={state.expanded}
            cursor={state.cursor}
            onRowClick={(cursor) => dispatch({ type: 'set-cursor', cursor })}
          />
        )}
      </div>
    </div>
  );
}
