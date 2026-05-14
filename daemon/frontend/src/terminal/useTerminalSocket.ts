import { useEffect, useRef, useState } from 'react';
import { vtTerminalWsUrl } from './api';

export type SocketStatus = 'connecting' | 'connected' | 'disconnected' | 'exited';

export type BinaryHandler = (data: Uint8Array) => void;
export type ControlHandler = (text: string) => void;

/** Client-side control messages on the VT WebSocket. */
export type ClientControl = { kind: 'resize'; cols: number; rows: number };

/** Options for the socket hook. */
export interface TerminalSocketOptions {
  lastSeq?: number;
}

/** Ref-stable handle returned by `useTerminalSocket`. */
export interface TerminalSocket {
  status: SocketStatus;
  sendBinary: (data: Uint8Array) => void;
  sendControl: (msg: ClientControl) => void;
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
  const frameHandlerRef = useRef<BinaryHandler | null>(null);
  const controlHandlerRef = useRef<ControlHandler | null>(null);
  const pendingFrameRef = useRef<Uint8Array[]>([]);
  const pendingControlRef = useRef<string[]>([]);
  const [status, setStatus] = useState<SocketStatus>('connecting');
  const statusRef = useRef<SocketStatus>('connecting');
  statusRef.current = status;
  const apiRef = useRef<TerminalSocket | null>(null);

  const lastSeq = options?.lastSeq;

  useEffect(() => {
    if (!activityId) return;
    const url = vtTerminalWsUrl(windowId, paneId, activityId, lastSeq);
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;
    pendingFrameRef.current = [];
    pendingControlRef.current = [];
    setStatus('connecting');

    ws.onopen = () => setStatus('connected');
    ws.onmessage = (ev) => {
      if (typeof ev.data === 'string') {
        const handler = controlHandlerRef.current;
        if (handler) handler(ev.data);
        else pendingControlRef.current.push(ev.data);
        return;
      }
      if (ev.data instanceof ArrayBuffer) {
        const bytes = new Uint8Array(ev.data);
        const handler = frameHandlerRef.current;
        if (handler) handler(bytes);
        else pendingFrameRef.current.push(bytes);
      }
    };
    ws.onclose = () => setStatus((s) => (s === 'exited' ? s : 'disconnected'));
    ws.onerror = () => setStatus('disconnected');

    return () => {
      wsRef.current = null;
      frameHandlerRef.current = null;
      controlHandlerRef.current = null;
      pendingFrameRef.current = [];
      pendingControlRef.current = [];
      ws.close();
    };
  }, [windowId, paneId, activityId, lastSeq]);

  if (apiRef.current === null) {
    const api: TerminalSocket = {
      get status() {
        return statusRef.current;
      },
      sendBinary(data) {
        const ws = wsRef.current;
        // NOTE: slice the exact byteOffset/byteLength range. Passing
        // `data.buffer` would send the entire underlying ArrayBuffer (msgpackr
        // Packr uses an 8192-byte pooled buffer and returns a subarray view),
        // causing the server to re-decode the FIRST frame on every send.
        if (ws?.readyState === WebSocket.OPEN) {
          const slice = data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength);
          ws.send(slice as ArrayBuffer);
        }
      },
      sendControl(msg) {
        const ws = wsRef.current;
        if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
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
