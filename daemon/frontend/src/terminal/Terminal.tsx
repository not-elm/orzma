//! Terminal entry component — VT canvas (DOM renderer).

import { useEffect, useRef } from 'react';
import { Cursor } from './overlay/Cursor';
import { IME } from './overlay/IME';
import { OverlayStoreContext, useOverlayState } from './overlay-store';
import { GridStoreContext } from './renderer/grid-store';
import { TerminalGrid } from './renderer/TerminalGrid';
import { StatusBanner } from './StatusBanner';
import { useCanvasTerminal } from './useCanvasTerminal';

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
}

function TerminalPaneBody({
  paneRef,
  textareaRef,
  status,
  isActive,
  preedit,
  hyperlinks,
  fm,
}: PaneBodyProps) {
  const overlay = useOverlayState();
  return (
    <div ref={paneRef} className="terminal-pane relative h-full w-full bg-background">
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
      {status === 'disconnected' && <StatusBanner kind="disconnected" onReconnect={() => {}} />}
      {status === 'exited' && <StatusBanner kind="exited" onReconnect={() => {}} />}
    </div>
  );
}
