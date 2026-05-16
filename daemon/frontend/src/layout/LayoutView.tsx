import { clsx } from 'clsx';
import { type PointerEventHandler, type ReactNode, useEffect, useRef, useState } from 'react';
import { cellHeightOf, cellWidthOf } from '../terminal/renderer/font';
import { PaneContent } from './PaneContent';
import { type Bounds, computePaneLayout } from './paneBounds';
import type { PaneId } from './types';
import { UnknownLayoutNode } from './UnknownLayoutNode';
import type { DefaultWindowState } from './useDefaultWindow';
import { useWindowDimensions } from './useWindowDimensions';
import type { LayoutState } from './useWindowLayout';

interface AbsoluteBoxProps {
  bounds: Bounds;
  className?: string;
  active?: boolean;
  onPointerDown?: PointerEventHandler<HTMLDivElement>;
  children: ReactNode;
}

function AbsoluteBox({ bounds, className, active, onPointerDown, children }: AbsoluteBoxProps) {
  return (
    <div
      data-active={active}
      className={clsx('absolute', className)}
      onPointerDown={onPointerDown}
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

interface LayoutViewProps {
  windowState: DefaultWindowState;
  layoutState: LayoutState;
}

export function LayoutView({ windowState: def, layoutState: layout }: LayoutViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [metrics, setMetrics] = useState<{ cellW: number; cellH: number } | null>(null);
  const liveWid = layout.status !== 'gone' && layout.view !== null ? layout.view.id : null;

  // Probe cell metrics from the live container once it mounts. Metrics depend
  // on the CSS font, not on layout size, so we measure once. `liveWid` is in
  // the dep list so the effect re-runs after the live container actually
  // attaches the ref (the prior render returned a placeholder without the ref).
  // biome-ignore lint/correctness/useExhaustiveDependencies: liveWid is the signal that the live container has rendered
  useEffect(() => {
    const el = containerRef.current;
    if (!el || metrics !== null) return;
    const cellW = cellWidthOf(el);
    const cellH = cellHeightOf(el);
    if (cellW > 0 && cellH > 0) setMetrics({ cellW, cellH });
  }, [metrics, liveWid]);

  useWindowDimensions(liveWid, containerRef.current, {
    cellWidth: metrics?.cellW ?? 0,
    cellHeight: metrics?.cellH ?? 0,
  });

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

  const activate = (paneId: PaneId) => {
    if (paneId === view.active_pane) return;
    fetch(`/windows/${view.id}/panes/${paneId}/activate`, { method: 'POST' }).catch((err) => {
      console.warn('failed to activate pane', err);
    });
  };

  return (
    <div ref={containerRef} className="relative h-dvh w-dvw bg-background">
      {view.panes.map((pane) => {
        const b = bounds.get(pane.id);
        if (!b) return null;
        const isActive = pane.id === view.active_pane;
        return (
          <AbsoluteBox
            key={pane.id}
            bounds={b}
            active={isActive}
            onPointerDown={() => activate(pane.id)}
            className={clsx(
              'outline -outline-offset-2',
              isActive
                ? 'outline-2 outline-tmux-pane-active'
                : 'outline-1 outline-tmux-pane-border',
            )}
          >
            <PaneContent
              windowId={view.id}
              pane={pane}
              isActive={isActive}
              onActivate={() => activate(pane.id)}
            />
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
