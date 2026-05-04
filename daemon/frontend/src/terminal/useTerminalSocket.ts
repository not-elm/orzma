import { useCallback, useEffect, useRef, useState } from 'react';
import { terminalWsUrl } from './api';

export type SocketStatus = 'connecting' | 'connected' | 'disconnected' | 'exited';

export type BinaryHandler = (data: Uint8Array) => void;

/** Mirrors the backend `ClientControl` enum (daemon/core/src/http/activities.rs). */
export type ClientControl = { type: 'resize'; cols: number; rows: number };

/** Mirrors the backend `ServerControl` enum. */
export type ServerControl = { type: 'exit'; code: number | null };

export interface TerminalSocket {
  status: SocketStatus;
  sendBinary: (data: Uint8Array) => void;
  sendControl: (msg: ClientControl) => void;
  /**
   * Register a handler for inbound binary frames (PTY output).
   * If a frame arrived before a handler was registered, the buffered frames
   * are delivered synchronously when the handler is set. Pass null to clear.
   */
  setBinaryHandler: (handler: BinaryHandler | null) => void;
}

export function useTerminalSocket(activityId: string | null, reconnectKey: number): TerminalSocket {
  const wsRef = useRef<WebSocket | null>(null);
  const handlerRef = useRef<BinaryHandler | null>(null);
  const pendingRef = useRef<Uint8Array[]>([]);
  const [status, setStatus] = useState<SocketStatus>('connecting');

  // reconnectKey is a re-run trigger (its value isn't read inside the effect),
  // which biome flags as an "extra" dependency. Suppress the warning.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reconnectKey is a re-run trigger
  useEffect(() => {
    if (!activityId) return;
    const ws = new WebSocket(terminalWsUrl(activityId));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    pendingRef.current = [];
    setStatus('connecting');

    ws.onopen = () => setStatus('connected');
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        try {
          const msg = JSON.parse(ev.data);
          if (msg?.type === 'exit') setStatus('exited');
        } catch {
          // Ignore malformed text frames
        }
        return;
      }
      if (ev.data instanceof ArrayBuffer) {
        const bytes = new Uint8Array(ev.data);
        const handler = handlerRef.current;
        if (handler) {
          handler(bytes);
        } else {
          pendingRef.current.push(bytes);
        }
      }
    };
    ws.onclose = () => setStatus((s) => (s === 'exited' ? s : 'disconnected'));
    ws.onerror = () => setStatus('disconnected');

    return () => {
      wsRef.current = null;
      handlerRef.current = null;
      pendingRef.current = [];
      ws.close();
    };
  }, [activityId, reconnectKey]);

  const setBinaryHandler = useCallback((handler: BinaryHandler | null) => {
    handlerRef.current = handler;
    if (handler) {
      const drained = pendingRef.current;
      pendingRef.current = [];
      for (const bytes of drained) handler(bytes);
    }
  }, []);

  const sendBinary = useCallback((data: Uint8Array) => {
    const ws = wsRef.current;
    if (ws?.readyState === WebSocket.OPEN) ws.send(data.buffer as ArrayBuffer);
  }, []);

  const sendControl = useCallback((msg: ClientControl) => {
    const ws = wsRef.current;
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
  }, []);

  return {
    status,
    sendBinary,
    sendControl,
    setBinaryHandler,
  };
}
