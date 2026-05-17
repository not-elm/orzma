import { useCallback, useEffect, useReducer, useRef } from 'react';
import type { SessionId, WindowId } from '../layout/types';
import { windowSelect } from '../statusbar/windowSelect';
import { flattenVisibleRows } from './flattenVisibleRows';
import { activeRowKey, TreeView } from './TreeView';
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

  const rows = flattenVisibleRows(tree, state.expanded);
  const activeId = activeRowKey(rows, state.cursor);

  const sessionCount = tree.length;
  const windowCount = tree.reduce((acc, s) => acc + s.windows.length, 0);

  return (
    // biome-ignore lint/a11y/useAriaPropsSupportedByRole: dialog manages focus on behalf of the tree; aria-activedescendant here is the correct ARIA 1.2 pattern for a focus-managing container
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Sessions and windows"
      aria-activedescendant={activeId}
      ref={rootRef}
      tabIndex={-1}
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/85 outline-none backdrop-blur-sm"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) onCloseRef.current();
      }}
    >
      {/* biome-ignore lint/plugin: modal sizing must be viewport-relative; no semantic token exists */}
      <div className="flex max-h-[75vh] w-[70vw] max-w-3xl flex-col overflow-hidden rounded-lg border border-border bg-card shadow-2xl ring-1 ring-primary/20">
        <header className="flex shrink-0 items-center justify-between border-b border-border bg-tmux-status-bar px-3 py-1.5 font-mono text-xs">
          <span className="text-tmux-status-bar-foreground">(choose-tree)</span>
          <span className="text-muted-foreground">
            {sessionCount} {sessionCount === 1 ? 'session' : 'sessions'} · {windowCount}{' '}
            {windowCount === 1 ? 'window' : 'windows'}
          </span>
        </header>
        <div className="min-h-0 flex-1 overflow-auto">
          {treeState.status === 'loading' && (
            <div className="p-3 font-mono text-sm text-muted-foreground">Loading sessions…</div>
          )}
          {treeState.status === 'error' && (
            <div className="p-3 font-mono text-sm text-destructive">
              Failed to load sessions: {treeState.message}
            </div>
          )}
          {treeState.status === 'ready' && (
            <TreeView
              rows={rows}
              cursor={state.cursor}
              onRowClick={(cursor) => dispatch({ type: 'set-cursor', cursor })}
            />
          )}
        </div>
        <footer className="flex shrink-0 items-center gap-4 border-t border-border bg-tmux-status-bar px-3 py-1.5 font-mono text-xs text-muted-foreground">
          <KeyHint keys={['↑', '↓']} label="navigate" />
          <KeyHint keys={['→', '↵']} label="select" />
          <KeyHint keys={['←']} label="collapse" />
          <KeyHint keys={['esc']} label="cancel" />
        </footer>
      </div>
    </div>
  );
}

interface KeyHintProps {
  keys: string[];
  label: string;
}

function KeyHint({ keys, label }: KeyHintProps) {
  return (
    <span className="flex items-center gap-1.5">
      <span className="flex items-center gap-0.5">
        {keys.map((k) => (
          <kbd
            key={k}
            className="rounded border border-border bg-background px-1.5 py-0 text-tmux-status-bar-foreground"
          >
            {k}
          </kbd>
        ))}
      </span>
      <span>{label}</span>
    </span>
  );
}
