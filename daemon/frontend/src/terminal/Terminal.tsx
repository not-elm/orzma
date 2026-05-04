import { useCallback, useRef, useState } from "react";
import { StatusBanner } from "./StatusBanner";
import { useDefaultActivity } from "./useDefaultActivity";
import { useTerminalSocket } from "./useTerminalSocket";
import { useXtermTerminal } from "./useXtermTerminal";

export function Terminal() {
  const activity = useDefaultActivity();
  const [reconnectKey, setReconnectKey] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const socket = useTerminalSocket(
    activity.status === "ready" ? activity.activityId : null,
    reconnectKey,
  );
  useXtermTerminal(containerRef, socket);

  const reconnect = useCallback(() => setReconnectKey((k) => k + 1), []);

  if (activity.status === "loading") {
    return <div className="p-4 text-muted-foreground">Loading…</div>;
  }
  if (activity.status === "error") {
    return (
      <div className="p-4 text-destructive">
        Failed to discover activity: {activity.message}
      </div>
    );
  }

  return (
    <div className="relative h-dvh w-dvw bg-background">
      <div ref={containerRef} className="absolute inset-0" />
      {socket.status === "disconnected" && (
        <StatusBanner kind="disconnected" onReconnect={reconnect} />
      )}
      {socket.status === "exited" && (
        <StatusBanner kind="exited" onReconnect={reconnect} />
      )}
    </div>
  );
}
