//! Terminal entry component — VT canvas (DOM renderer).

import { clsx } from 'clsx';
import { useEffect, useRef, useSyncExternalStore } from 'react';
import { Cursor } from './overlay/Cursor';
import { IME } from './overlay/IME';
import { OverlayStoreContext, useOverlayState } from './overlay-store';
import { type GridStore, GridStoreContext } from './renderer/grid-store';
import { TerminalGrid } from './renderer/TerminalGrid';
import { ScrolledBadge } from './ScrolledBadge';
import { StatusBanner } from './StatusBanner';
import { TerminalScrollbar } from './TerminalScrollbar';
import { useCanvasTerminal } from './useCanvasTerminal';
import type { TerminalSocket } from './useTerminalSocket';

interface TerminalProps {
  windowId: string;
  paneId: string;
  activityId: string;
  isActive: boolean;
}

export function Terminal({ windowId, paneId, activityId, isActive }: TerminalProps) {
  const {
    paneRef,
    textareaRef,
    status,
    focus,
    blur,
    preedit,
    hyperlinks,
    fm,
    gridStore,
    overlayStore,
    socket,
  } = useCanvasTerminal(windowId, paneId, activityId, isActive);

  const prevActiveRef = useRef(isActive);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
    else if (!isActive && prevActiveRef.current) blur();
    prevActiveRef.current = isActive;
  }, [isActive]);

  return (
    <GridStoreContext.Provider value={gridStore}>
      <OverlayStoreContext.Provider value={overlayStore}>
        <TerminalPaneBody
          paneRef={paneRef}
          textareaRef={textareaRef}
          status={status}
          isActive={isActive}
          preedit={preedit}
          hyperlinks={hyperlinks}
          fm={fm}
          gridStore={gridStore}
          socket={socket}
        />
      </OverlayStoreContext.Provider>
    </GridStoreContext.Provider>
  );
}

interface PaneBodyProps {
  paneRef: React.RefObject<HTMLDivElement | null>;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  status: ReturnType<typeof useCanvasTerminal>['status'];
  isActive: boolean;
  preedit: string;
  hyperlinks: ReadonlyMap<number, string>;
  fm: ReturnType<typeof useCanvasTerminal>['fm'];
  gridStore: GridStore;
  socket: TerminalSocket;
}

function TerminalPaneBody({
  paneRef,
  textareaRef,
  status,
  isActive,
  preedit,
  hyperlinks,
  fm,
  gridStore,
  socket,
}: PaneBodyProps) {
  const overlay = useOverlayState();
  const scrollState = useSyncExternalStore(
    gridStore.subscribe,
    gridStore.getScrollSnapshot,
    gridStore.getScrollSnapshot,
  );
  const grid = useSyncExternalStore(
    gridStore.subscribe,
    gridStore.getSnapshot,
    gridStore.getSnapshot,
  );
  const viewportRows = grid.rows;
  return (
    <div
      ref={paneRef}
      className={clsx(
        'terminal-pane relative h-full w-full',
        isActive ? 'bg-background' : 'bg-tmux-pane-inactive-bg',
      )}
    >
      <TerminalGrid fm={fm} hyperlinks={hyperlinks} />
      <Cursor cursor={overlay.cursor} isActive={isActive} fm={overlay.fm} />
      {preedit && <IME preedit={preedit} cursor={overlay.cursor} fm={overlay.fm} />}
      <textarea
        ref={textareaRef}
        className="absolute inset-0 resize-none border-0 bg-transparent text-transparent caret-transparent outline-none pointer-events-none"
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        // biome-ignore lint/a11y/noAutofocus: keystroke sink — invisible
        autoFocus={isActive}
      />
      <TerminalScrollbar
        displayOffset={scrollState.displayOffset}
        historySize={scrollState.historySize}
        viewportRows={viewportRows}
      />
      <ScrolledBadge
        displayOffset={scrollState.displayOffset}
        onResume={() => socket.sendControl({ kind: 'scroll_to_bottom' })}
      />
      {status === 'disconnected' && <StatusBanner kind="disconnected" onReconnect={() => {}} />}
      {status === 'exited' && <StatusBanner kind="exited" onReconnect={() => {}} />}
      {!isActive && (
        <div
          className="absolute inset-0 z-10 pointer-events-none bg-tmux-pane-inactive-overlay"
          aria-hidden="true"
        />
      )}
    </div>
  );
}
