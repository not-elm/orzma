import { type RefObject, useEffect, useRef, useState } from "react";
import { terminalWsUrl } from "./api";

export type SocketStatus = "connecting" | "connected" | "disconnected" | "exited";

export interface TerminalSocket {
  status: SocketStatus;
  sendBinary: (data: Uint8Array) => void;
  sendControl: (msg: object) => void;
  /** Direct WS access for the xterm hook to attach a 'message' listener. */
  wsRef: RefObject<WebSocket | null>;
}

export function useTerminalSocket(
  activityId: string | null,
  reconnectKey: number,
): TerminalSocket {
  const wsRef = useRef<WebSocket | null>(null);
  const [status, setStatus] = useState<SocketStatus>("connecting");

  useEffect(() => {
    if (!activityId) return;
    const ws = new WebSocket(terminalWsUrl(activityId));
    ws.binaryType = "arraybuffer";
    wsRef.current = ws;
    setStatus("connecting");

    ws.onopen = () => setStatus("connected");
    ws.onmessage = (ev) => {
      // Text frames carry server control messages; we update status here.
      // Binary frames are handled by useXtermTerminal via addEventListener.
      if (typeof ev.data === "string") {
        try {
          const msg = JSON.parse(ev.data);
          if (msg?.type === "exit") setStatus("exited");
        } catch {
          // Ignore malformed text frames
        }
      }
    };
    ws.onclose = () => setStatus((s) => (s === "exited" ? s : "disconnected"));
    ws.onerror = () => setStatus("disconnected");

    return () => {
      wsRef.current = null;
      ws.close();
    };
  }, [activityId, reconnectKey]);

  return {
    status,
    sendBinary: (data) => {
      const ws = wsRef.current;
      if (ws?.readyState === WebSocket.OPEN) ws.send(data.buffer as ArrayBuffer);
    },
    sendControl: (msg) => {
      const ws = wsRef.current;
      if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
    },
    wsRef,
  };
}
