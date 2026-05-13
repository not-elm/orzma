import { useEffect, useRef, useState } from 'react';
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
  const [reconnectKey, setReconnectKey] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(windowId, paneId, activityId, reconnectKey);
  const { focus, blur } = useXtermTerminal(containerRef, socket);

  // Seeded false so an initial isActive=true mount registers as a transition.
  const prevActiveRef = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus/blur are stabilized by React Compiler; adding them would re-run on every render and defeat transition-only semantics
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
    else if (!isActive && prevActiveRef.current) blur();
    prevActiveRef.current = isActive;
  }, [isActive]);

  const reconnect = () => setReconnectKey((k) => k + 1);

  return (
    <div className="relative h-full w-full bg-background">
      <div ref={containerRef} className="absolute inset-0" />
      {socket.status === 'disconnected' && (
        <StatusBanner kind="disconnected" onReconnect={reconnect} />
      )}
      {socket.status === 'exited' && <StatusBanner kind="exited" onReconnect={reconnect} />}
    </div>
  );
}
