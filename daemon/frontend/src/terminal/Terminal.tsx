import { useCallback, useRef, useState } from 'react';
import { StatusBanner } from './StatusBanner';
import { useTerminalSocket } from './useTerminalSocket';
import { useXtermTerminal } from './useXtermTerminal';

interface TerminalProps {
  activityId: string;
}

export function Terminal({ activityId }: TerminalProps) {
  const [reconnectKey, setReconnectKey] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(activityId, reconnectKey);
  useXtermTerminal(containerRef, socket);

  const reconnect = useCallback(() => setReconnectKey((k) => k + 1), []);

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
