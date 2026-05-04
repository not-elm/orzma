import { useCallback, useRef, useState } from 'react';
import { StatusBanner } from './StatusBanner';
import { useDefaultActivity } from './useDefaultActivity';
import { useTerminalSocket } from './useTerminalSocket';
import { useXtermTerminal } from './useXtermTerminal';

export function Terminal() {
  const activity = useDefaultActivity();
  const [reconnectKey, setReconnectKey] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(
    activity.status === 'ready' ? activity.activityId : null,
    reconnectKey,
  );
  useXtermTerminal(containerRef, socket);

  const reconnect = useCallback(() => setReconnectKey((k) => k + 1), []);

  return (
    <div className="relative h-dvh w-dvw bg-background">
      <div ref={containerRef} className="absolute inset-0" />
      {activity.status === 'loading' && (
        <div className="absolute inset-0 flex items-center justify-center text-muted-foreground">
          Loading…
        </div>
      )}
      {activity.status === 'error' && (
        <div className="absolute inset-0 flex items-center justify-center p-4 text-destructive">
          Failed to discover activity: {activity.message}
        </div>
      )}
      {socket.status === 'disconnected' && (
        <StatusBanner kind="disconnected" onReconnect={reconnect} />
      )}
      {socket.status === 'exited' && <StatusBanner kind="exited" onReconnect={reconnect} />}
    </div>
  );
}
