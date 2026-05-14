//! Terminal entry component — branches between xterm.js and VT canvas based on `?mode=vt`.

import { useEffect, useRef } from 'react';
import { Cursor } from './overlay/Cursor';
import { IME } from './overlay/IME';
import { useOverlayState } from './overlay-store';
import { TerminalGrid } from './renderer/TerminalGrid';
import { StatusBanner } from './StatusBanner';
import { useCanvasTerminal } from './useCanvasTerminal';
import { useTerminalSocket } from './useTerminalSocket';
import { useXtermTerminal } from './useXtermTerminal';

interface TerminalProps {
  windowId: string;
  paneId: string;
  activityId: string;
  isActive: boolean;
}

function isVtMode(): boolean {
  if (typeof location === 'undefined') return false;
  return new URLSearchParams(location.search).get('mode') === 'vt';
}

export function Terminal(props: TerminalProps) {
  return isVtMode() ? <VtTerminal {...props} /> : <XtermTerminal {...props} />;
}

function VtTerminal({ windowId, paneId, activityId, isActive }: TerminalProps) {
  const { paneRef, textareaRef, status, focus, blur, preedit, hyperlinks, fm } = useCanvasTerminal(
    windowId,
    paneId,
    activityId,
    isActive,
  );
  const overlay = useOverlayState();

  const prevActiveRef = useRef(isActive);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
    else if (!isActive && prevActiveRef.current) blur();
    prevActiveRef.current = isActive;
  }, [isActive]);

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

function XtermTerminal({ windowId, paneId, activityId, isActive }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(windowId, paneId, activityId);
  const { focus, blur } = useXtermTerminal(containerRef, socket);

  const prevActiveRef = useRef(isActive);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler; adding them would re-run on every render and defeat transition-only semantics
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
    else if (!isActive && prevActiveRef.current) blur();
    prevActiveRef.current = isActive;
  }, [isActive]);

  return (
    <div className="relative h-full w-full bg-background">
      <div ref={containerRef} className="absolute inset-0" />
      {socket.status === 'disconnected' && (
        <StatusBanner
          kind="disconnected"
          onReconnect={() => {
            // TODO: Phase 3 wires ReconnectController
          }}
        />
      )}
      {socket.status === 'exited' && (
        <StatusBanner
          kind="exited"
          onReconnect={() => {
            // TODO: Phase 3 wires ReconnectController
          }}
        />
      )}
    </div>
  );
}
