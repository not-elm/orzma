//! Terminal entry component — DOM renderer.

import { clsx } from 'clsx';
import { useEffect, useRef, useSyncExternalStore } from 'react';
import { Cursor } from './overlay/Cursor';
import { IME } from './overlay/IME';
import { OverlayStoreContext, useOverlayState } from './overlay-store';
import { type GridStore, GridStoreContext } from './renderer/grid-store';
import { TerminalGrid } from './renderer/TerminalGrid';
import { StatusBanner } from './StatusBanner';
import { TerminalScrollbar } from './TerminalScrollbar';
import { useTerminal } from './useTerminal';

interface TerminalProps {
  windowId: string;
  paneId: string;
  activityId: string;
  isActive: boolean;
  replay?: string;
  recordPerf?: boolean;
}

export function Terminal({
  windowId,
  paneId,
  activityId,
  isActive,
  replay,
  recordPerf,
}: TerminalProps) {
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
  } = useTerminal(windowId, paneId, activityId, { replay, recordPerf });

  // NOTE: seeded false so an initial isActive=true mount registers as a transition.
  const prevActiveRef = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler; adding them would re-run on every render and defeat transition-only semantics
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
        />
      </OverlayStoreContext.Provider>
    </GridStoreContext.Provider>
  );
}

interface PaneBodyProps {
  paneRef: React.RefObject<HTMLDivElement | null>;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  status: ReturnType<typeof useTerminal>['status'];
  isActive: boolean;
  preedit: string;
  hyperlinks: ReadonlyMap<number, string>;
  fm: ReturnType<typeof useTerminal>['fm'];
  gridStore: GridStore;
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
