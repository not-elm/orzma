import { type PointerEventHandler, type ReactNode, useEffect, useState } from 'react';
import { cellHeightOf, cellWidthOf } from '../terminal/renderer/font';
import { PaneContent } from './PaneContent';
import { type Bounds, computePaneLayout } from './paneBounds';
import type { DefaultWindowState, PaneId } from './types';
import { UnknownLayoutNode } from './UnknownLayoutNode';
import { useWindowDimensions } from './useWindowDimensions';
import type { LayoutState } from './useWindowLayout';

interface AbsoluteBoxProps {
  bounds: Bounds;
  active?: boolean;
  onPointerDown?: PointerEventHandler<HTMLDivElement>;
  children: ReactNode;
}

function AbsoluteBox({ bounds, active, onPointerDown, children }: AbsoluteBoxProps) {
  return (
    <div
      data-active={active}
      className="absolute"
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
  replay?: string;
  recordPerf?: boolean;
}

export function LayoutView({
  windowState: def,
  layoutState: layout,
  replay,
  recordPerf,
}: LayoutViewProps) {
  const [container, setContainer] = useState<HTMLDivElement | null>(null);
  const [metrics, setMetrics] = useState<{ cellW: number; cellH: number } | null>(null);
  const liveWid = layout.status !== 'gone' && layout.view !== null ? layout.view.id : null;

  // NOTE: font metrics depend on CSS font, not layout size — measure once.
  useEffect(() => {
    if (!container || metrics !== null) return;
    const cellW = cellWidthOf(container);
    const cellH = cellHeightOf(container);
    if (cellW > 0 && cellH > 0) setMetrics({ cellW, cellH });
  }, [container, metrics]);

  useWindowDimensions(liveWid, container, {
    cellWidth: metrics?.cellW ?? 0,
    cellHeight: metrics?.cellH ?? 0,
  });

  if (def.status === 'loading') {
    return (
      <div className="flex h-full w-full items-center justify-center text-muted-foreground">
        Loading…
      </div>
    );
  }
  if (def.status === 'error') {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-destructive">
        Failed to discover window: {def.message}
      </div>
    );
  }
  if (layout.status === 'gone') {
    return (
      <div className="flex h-full w-full items-center justify-center p-4 text-destructive">
        Window is gone ({layout.reason}).
      </div>
    );
  }
  if (layout.view === null) {
    return (
      <div className="flex h-full w-full items-center justify-center text-muted-foreground">
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
    <div ref={setContainer} className="relative h-full w-full bg-tmux-status-bar">
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
          >
            <PaneContent
              windowId={view.id}
              pane={pane}
              isActive={isActive}
              onActivate={() => activate(pane.id)}
              replay={replay}
              recordPerf={recordPerf}
            />
          </AbsoluteBox>
        );
      })}
      {unknown.map((u) => (
        <AbsoluteBox key={u.cell_id} bounds={u.bounds}>
          <UnknownLayoutNode type={u.type} />
        </AbsoluteBox>
      ))}
    </div>
  );
}
