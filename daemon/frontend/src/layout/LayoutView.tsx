import { clsx } from 'clsx';
import type { ReactNode } from 'react';
import { PaneContent } from './PaneContent';
import { type Bounds, computePaneLayout } from './paneBounds';
import { UnknownLayoutNode } from './UnknownLayoutNode';
import { useDefaultWindow } from './useDefaultWindow';
import { useWindowLayout } from './useWindowLayout';

interface AbsoluteBoxProps {
  bounds: Bounds;
  className?: string;
  active?: boolean;
  children: ReactNode;
}

function AbsoluteBox({ bounds, className, active, children }: AbsoluteBoxProps) {
  return (
    <div
      data-active={active}
      className={clsx('absolute', className)}
      // biome-ignore lint/plugin: bounds are computed at runtime as percentages of the window
      style={{
        left: `${bounds.x}%`,
        top: `${bounds.y}%`,
        width: `${bounds.w}%`,
        height: `${bounds.h}%`,
      }}
    >
      {children}
    </div>
  );
}

export function LayoutView() {
  const def = useDefaultWindow();
  const wid = def.status === 'ready' ? def.windowId : null;
  const layout = useWindowLayout(wid);

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
          <AbsoluteBox
            key={pane.id}
            bounds={b}
            active={isActive}
            className={clsx(
              'outline -outline-offset-2',
              isActive
                ? 'outline-2 outline-tmux-pane-active'
                : 'outline-1 outline-tmux-pane-border',
            )}
          >
            <PaneContent pane={pane} />
          </AbsoluteBox>
        );
      })}
      {unknown.map((u) => (
        <AbsoluteBox key={u.cell_id} bounds={u.bounds}>
          <UnknownLayoutNode type={u.type} />
        </AbsoluteBox>
      ))}
      {layout.status === 'reconnecting' && (
        <div className="absolute right-2 top-2 rounded bg-warning px-2 py-1 text-xs text-warning-foreground">
          Reconnecting…
        </div>
      )}
    </div>
  );
}
