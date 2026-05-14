//! Terminal entry component — branches between xterm.js and VT canvas based on `?mode=vt`.

import { useEffect, useRef } from 'react';
import { Cursor } from './overlay/Cursor';
import { IME } from './overlay/IME';
import { Link } from './overlay/Link';
import { Selection } from './overlay/Selection';
import { useOverlayState } from './overlay-store';
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
  const { canvasRef, textareaRef, status, focus, blur, preedit, selection, linkHover } =
    useCanvasTerminal(windowId, paneId, activityId, isActive);
  const overlay = useOverlayState();

  const prevActiveRef = useRef(isActive);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler; adding them would re-run on every render and defeat transition-only semantics
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
    else if (!isActive && prevActiveRef.current) blur();
    prevActiveRef.current = isActive;
  }, [isActive]);

  return (
    <div className="relative h-full w-full bg-background">
      <canvas ref={canvasRef} className="absolute left-0 top-0" />
      <Cursor cursor={overlay.cursor} isActive={isActive} fm={overlay.fm} />
      {selection && <Selection selection={selection} cols={overlay.cols} fm={overlay.fm} />}
      {linkHover && <Link hover={linkHover} fm={overlay.fm} />}
      {preedit && <IME preedit={preedit} cursor={overlay.cursor} fm={overlay.fm} />}
      <textarea
        ref={textareaRef}
        className="absolute inset-0 resize-none border-0 bg-transparent text-transparent caret-transparent outline-none"
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        // biome-ignore lint/a11y/noAutofocus: terminal pane requires focus to receive keystrokes; this textarea is invisible and exists solely as the keyboard sink for the canvas.
        autoFocus={isActive}
      />
      {status === 'disconnected' && (
        <StatusBanner
          kind="disconnected"
          onReconnect={() => {
            // TODO: Phase 3 wires ReconnectController
          }}
        />
      )}
      {status === 'exited' && (
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
