import { useEffect, useRef, useState } from 'react';
import { StatusBanner } from './StatusBanner';
import { useTerminalSocket } from './useTerminalSocket';
import { useXtermTerminal } from './useXtermTerminal';

interface TerminalProps {
  activityId: string;
  isActive: boolean;
}

export function Terminal({ activityId, isActive }: TerminalProps) {
  const [reconnectKey, setReconnectKey] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(activityId, reconnectKey);
  const { focus } = useXtermTerminal(containerRef, socket);

  const prevActiveRef = useRef(isActive);
  // biome-ignore lint/correctness/useExhaustiveDependencies: focus is stabilized by React Compiler; adding it would re-run on every render and defeat transition-only semantics
  useEffect(() => {
    if (isActive && !prevActiveRef.current) focus();
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
