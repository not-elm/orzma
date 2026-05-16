import { useEffect, useRef, useState } from 'react';
import { browserWsUrl } from './api';
import { decode, encode } from './protocol/frame';
import type { BrowserClientMsg, BrowserServerMsg } from './protocol/wire';

/** Latest frame received from the daemon (decoded JPEG bytes + dimensions). */
export interface LastFrame {
  jpeg: Uint8Array;
  width: number;
  height: number;
}

/** Latest navigation state. */
export interface NavState {
  url: string;
  title: string;
}

/** Snapshot of the live browser activity exposed by `useBrowserSocket`. */
export interface BrowserSocketState {
  send: (msg: BrowserClientMsg) => void;
  lastFrame: LastFrame | null;
  nav: NavState;
  viewport: { width: number; height: number };
}

const INITIAL_NAV: NavState = {
  url: '',
  title: '',
};

const INITIAL_VIEWPORT = { width: 0, height: 0 };

/**
 * React hook that maintains a WebSocket to the daemon's browser stream for
 * one Activity. Decodes msgpack server frames into `lastFrame` / `nav` /
 * `viewport` and exposes a `send` callback for client messages.
 *
 * Resize messages sent before the socket is open are buffered (latest-only)
 * and flushed on `onopen`. All other messages are dropped while the socket is
 * not in the OPEN state.
 */
export function useBrowserSocket(
  windowId: string,
  paneId: string,
  activityId: string,
): BrowserSocketState {
  const wsRef = useRef<WebSocket | null>(null);
  const pendingResize = useRef<BrowserClientMsg | null>(null);
  const [lastFrame, setLastFrame] = useState<LastFrame | null>(null);
  const [nav, setNav] = useState<NavState>(INITIAL_NAV);
  const [viewport, setViewport] = useState<{ width: number; height: number }>(INITIAL_VIEWPORT);

  useEffect(() => {
    const ws = new WebSocket(browserWsUrl(windowId, paneId, activityId));
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;

    ws.onopen = () => {
      if (pendingResize.current) {
        const encoded = encode(pendingResize.current);
        ws.send(
          encoded.buffer.slice(
            encoded.byteOffset,
            encoded.byteOffset + encoded.byteLength,
          ) as ArrayBuffer,
        );
        pendingResize.current = null;
      }
    };

    ws.onmessage = (e: MessageEvent<ArrayBuffer>) => {
      const msg: BrowserServerMsg = decode(e.data);
      switch (msg.kind) {
        case 'screencast':
          setLastFrame({ jpeg: msg.jpeg, width: msg.width, height: msg.height });
          break;
        case 'nav':
          setNav({ url: msg.url, title: msg.title });
          break;
        case 'viewport':
          setViewport({ width: msg.width, height: msg.height });
          break;
        case 'clipboard_write':
          navigator.clipboard.writeText(msg.text).catch(() => {});
          break;
        case 'page_error':
          // NOTE: future "sad tab" UI; ignore for now.
          break;
      }
    };
    return () => {
      ws.close();
      wsRef.current = null;
    };
  }, [windowId, paneId, activityId]);

  const send = (msg: BrowserClientMsg) => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      // NOTE: buffer the latest resize so it flushes on open; drop everything else.
      if (msg.kind === 'resize') {
        pendingResize.current = msg;
      }
      return;
    }
    const encoded = encode(msg);
    // NOTE: slice the exact byteOffset/byteLength range. msgpackr uses a pooled
    // buffer and returns a subarray view, so passing encoded.buffer directly
    // would transmit the entire 8192-byte pool rather than just the message.
    ws.send(
      encoded.buffer.slice(
        encoded.byteOffset,
        encoded.byteOffset + encoded.byteLength,
      ) as ArrayBuffer,
    );
  };
  return { send, lastFrame, nav, viewport };
}
