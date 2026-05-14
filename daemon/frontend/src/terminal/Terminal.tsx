import { useEffect, useRef } from 'react';
import { StatusBanner } from './StatusBanner';
import { useTerminalSocket } from './useTerminalSocket';
import { useXtermTerminal } from './useXtermTerminal';

interface TerminalProps {
  windowId: string;
  paneId: string;
  activityId: string;
  isActive: boolean;
}

export function Terminal({ windowId, paneId, activityId, isActive }: TerminalProps) {
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
