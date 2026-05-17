import type { SessionId } from '../layout/types';
import { TreeView } from './TreeView';
import { useChooseTreeController } from './useChooseTreeController';

interface ChooseTreeOverlayProps {
  onClose: () => void;
  attachedSessionId: SessionId | null;
  setAttachedSession: (sid: SessionId) => void;
}

/**
 * Center-modal tree picker. All imperative state and side effects live
 * in `useChooseTreeController`; this component is pure layout/markup.
 */
export function ChooseTreeOverlay({
  onClose,
  attachedSessionId,
  setAttachedSession,
}: ChooseTreeOverlayProps) {
  const {
    treeState,
    rows,
    cursor,
    activeRowId,
    sessionCount,
    windowCount,
    rootRef,
    onBackdropPointerDown,
    onRowClick,
  } = useChooseTreeController({ onClose, attachedSessionId, setAttachedSession });

  return (
    // biome-ignore lint/a11y/useAriaPropsSupportedByRole: dialog manages focus on behalf of the tree; aria-activedescendant here is the correct ARIA 1.2 pattern for a focus-managing container
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Sessions and windows"
      aria-activedescendant={activeRowId}
      ref={rootRef}
      tabIndex={-1}
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/85 outline-none backdrop-blur-sm"
      onPointerDown={onBackdropPointerDown}
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
            <TreeView rows={rows} cursor={cursor} onRowClick={onRowClick} />
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
