import { useEffect, useRef, useState } from 'react';
import { terminalWsUrl, vtTerminalWsUrl } from './api';

export type SocketStatus = 'connecting' | 'connected' | 'disconnected' | 'exited';

export type BinaryHandler = (data: Uint8Array) => void;
export type ControlHandler = (text: string) => void;

/** Client-side control messages (xterm legacy uses `type`; VT mode uses `kind`). */
export type ClientControl =
  | { type: 'resize'; cols: number; rows: number }
  | { kind: 'resize'; cols: number; rows: number };

/** Server-side control messages on the raw-bytes path. */
export type ServerControl = { type: 'exit'; code: number | null };

/** Options for the socket hook. Default is xterm raw-bytes mode (backwards compat). */
export interface TerminalSocketOptions {
  mode?: 'vt' | 'xterm';
  lastSeq?: number;
}

/** Ref-stable handle returned by `useTerminalSocket`. */
export interface TerminalSocket {
  status: SocketStatus;
  sendBinary: (data: Uint8Array) => void;
  sendControl: (msg: ClientControl) => void;
  setBinaryHandler: (handler: BinaryHandler | null) => void;
  setFrameHandler: (handler: BinaryHandler | null) => void;
  setControlHandler: (handler: ControlHandler | null) => void;
  reportDecodeError: (message: string) => void;
}

export function useTerminalSocket(
  windowId: string,
  paneId: string,
  activityId: string | null,
  options?: TerminalSocketOptions,
): TerminalSocket {
  const wsRef = useRef<WebSocket | null>(null);
  const binaryHandlerRef = useRef<BinaryHandler | null>(null);
  const frameHandlerRef = useRef<BinaryHandler | null>(null);
  const controlHandlerRef = useRef<ControlHandler | null>(null);
  const pendingBinaryRef = useRef<Uint8Array[]>([]);
  const pendingFrameRef = useRef<Uint8Array[]>([]);
  const pendingControlRef = useRef<string[]>([]);
  const [status, setStatus] = useState<SocketStatus>('connecting');
  const statusRef = useRef<SocketStatus>('connecting');
  statusRef.current = status;
  const apiRef = useRef<TerminalSocket | null>(null);

  const mode = options?.mode ?? 'xterm';
  const lastSeq = options?.lastSeq;

  useEffect(() => {
    if (!activityId) return;
    const url =
      mode === 'vt'
        ? vtTerminalWsUrl(windowId, paneId, activityId, lastSeq)
        : terminalWsUrl(windowId, paneId, activityId);
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    pendingBinaryRef.current = [];
    pendingFrameRef.current = [];
    pendingControlRef.current = [];
    setStatus('connecting');

    ws.onopen = () => setStatus('connected');
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        if (mode === 'vt') {
          const handler = controlHandlerRef.current;
          if (handler) handler(ev.data);
          else pendingControlRef.current.push(ev.data);
        } else {
          try {
            const msg = JSON.parse(ev.data) as { type?: string };
            if (msg?.type === 'exit') setStatus('exited');
          } catch {
            // Ignore malformed text frames
          }
        }
        return;
      }
      if (ev.data instanceof ArrayBuffer) {
        const bytes = new Uint8Array(ev.data);
        if (mode === 'vt') {
          const handler = frameHandlerRef.current;
          if (handler) handler(bytes);
          else pendingFrameRef.current.push(bytes);
        } else {
          const handler = binaryHandlerRef.current;
          if (handler) handler(bytes);
          else pendingBinaryRef.current.push(bytes);
        }
      }
    };
    ws.onclose = () => setStatus((s) => (s === 'exited' ? s : 'disconnected'));
    ws.onerror = () => setStatus('disconnected');

    return () => {
      wsRef.current = null;
      binaryHandlerRef.current = null;
      frameHandlerRef.current = null;
      controlHandlerRef.current = null;
      pendingBinaryRef.current = [];
      pendingFrameRef.current = [];
      pendingControlRef.current = [];
      ws.close();
    };
  }, [windowId, paneId, activityId, mode, lastSeq]);

  if (apiRef.current === null) {
    const api: TerminalSocket = {
      get status() {
        return statusRef.current;
      },
      sendBinary(data) {
        const ws = wsRef.current;
        if (ws?.readyState === WebSocket.OPEN) ws.send(data.buffer as ArrayBuffer);
      },
      sendControl(msg) {
        const ws = wsRef.current;
        if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
      },
      setBinaryHandler(handler) {
        binaryHandlerRef.current = handler;
        if (handler) {
          const drained = pendingBinaryRef.current;
          pendingBinaryRef.current = [];
          for (const bytes of drained) handler(bytes);
        }
      },
      setFrameHandler(handler) {
        frameHandlerRef.current = handler;
        if (handler) {
          const drained = pendingFrameRef.current;
          pendingFrameRef.current = [];
          for (const bytes of drained) handler(bytes);
        }
      },
      setControlHandler(handler) {
        controlHandlerRef.current = handler;
        if (handler) {
          const drained = pendingControlRef.current;
          pendingControlRef.current = [];
          for (const text of drained) handler(text);
        }
      },
      reportDecodeError(message) {
        // NOTE: Phase 2B logs only; Phase 3 wires ReconnectController + client_error frame.
        console.warn('[terminal] decode error:', message);
      },
    };
    apiRef.current = api;
  }

  return apiRef.current;
}
