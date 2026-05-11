import { computePaneLayout } from './paneBounds';
import { PaneContent } from './PaneContent';
import { UnknownLayoutNode } from './UnknownLayoutNode';
import { useDefaultWindow } from './useDefaultWindow';
import { useWindowLayout } from './useWindowLayout';

export function LayoutView() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);

  // All hooks above are called unconditionally. The branches below are
  // pure return statements — no hooks — so Rules of Hooks is satisfied.

  if (def.status === 'loading') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center text-muted-foreground">
        Loading…
      </div>
    );
  }
  if (def.status === 'error') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center p-4 text-destructive">
        Failed to discover window: {def.message}
      </div>
    );
  }
  if (layout.status === 'gone') {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center p-4 text-destructive">
        Window is gone ({layout.reason}).
      </div>
    );
  }
  if (layout.view === null) {
    return (
      <div className="flex h-dvh w-dvw items-center justify-center text-muted-foreground">
        Connecting…
      </div>
    );
  }

  const view = layout.view;
  const { panes: bounds, unknown } = computePaneLayout(view.layout);

  return (
    <div className="relative h-dvh w-dvw bg-background">
      {view.panes.map((pane) => {
        const b = bounds.get(pane.id);
        if (!b) return null; // pane not represented in layout; skip silently
        const isActive = pane.id === view.active_pane;
        return (
          <div
            key={pane.id}
            data-active={isActive}
            style={{
              position: 'absolute',
              left: `${b.x}%`,
              top: `${b.y}%`,
              width: `${b.w}%`,
              height: `${b.h}%`,
              outline: isActive
                ? '2px solid var(--color-tmux-pane-active)'
                : '1px solid var(--color-tmux-pane-border)',
              outlineOffset: '-2px',
            }}
          >
            <PaneContent pane={pane} />
          </div>
        );
      })}
      {unknown.map((u) => (
        <div
          key={u.cell_id}
          style={{
            position: 'absolute',
            left: `${u.bounds.x}%`,
            top: `${u.bounds.y}%`,
            width: `${u.bounds.w}%`,
            height: `${u.bounds.h}%`,
          }}
        >
          <UnknownLayoutNode type={u.type} />
        </div>
      ))}
      {layout.status === 'reconnecting' && (
        <div className="absolute right-2 top-2 rounded bg-warning px-2 py-1 text-xs text-warning-foreground">
          Reconnecting…
        </div>
      )}
    </div>
  );
}
