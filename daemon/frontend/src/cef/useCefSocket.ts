//! CEF WebSocket hook shared by Browser and Extension activities.
//!
//! Connects to the supplied path (`browser/ws` for Browser activities,
//! `extension/cef/ws` for Extension activities — both endpoints speak the same
//! `BrowserClientMsg`/`BrowserServerMsg` wire protocol), sends Subscribe on
//! open, and forwards every incoming binary frame to the supplied frame worker
//! as a transferable ArrayBuffer.
//!
//! Peeks each incoming message's `kind` field on the main thread:
//! SubscribeReply / Nav / BrowserUnavailable / Cursor frames are consumed here
//! and dispatched to the supplied callbacks. Every other frame (screencast)
//! is forwarded to the worker as a transferable ArrayBuffer so decode stays
//! off the main thread.
//!
//! Returns `{ send }` so callers can push BrowserClientMsg frames on the same
//! socket the hook subscribed on (e.g. for input forwarding).

import { decode, encode } from 'msgpackr';
import { useEffect, useRef } from 'react';
import type { InputEvent } from '../browser/protocol/input';

/** Identifier for a single frame produced by cef_host. Echoed back to the
 *  daemon on resubscribe so it can replay deltas from a known base. */
export type FrameKey = {
  session_id: bigint;
  epoch: number;
  frame_seq: bigint;
};

/** Discriminated union of messages the frontend sends to the daemon over the
 *  cef WebSocket. Mirrors `BrowserClientMsg` in wire.rs (spec §5). */
export type BrowserClientMsg =
  | {
      kind: 'subscribe';
      session_id: bigint | null;
      last_key: FrameKey | null;
      has_base_keyframe: boolean;
    }
  | { kind: 'resize'; css_w: number; css_h: number; dpr: number }
  | { kind: 'input'; event: InputEvent }
  | { kind: 'navigate'; url: string }
  | { kind: 'navigate_history'; delta: number }
  | { kind: 'copy_request' }
  | { kind: 'paste'; text: string };

/** Snapshot of navigation state delivered by `BrowserServerMsg::Nav`. */
export interface NavSnapshot {
  url: string;
  title: string;
  can_back: boolean;
  can_forward: boolean;
}

/** Mirrors `BrowserUnavailableReason` in wire.rs (serde tag = "kind", snake_case). */
export type BrowserUnavailableReason =
  | { kind: 'retry_exhausted'; last_error: string }
  | { kind: 'extension_disconnected' };

/** Mirrors `CursorKind` in wire.rs (serde snake_case). */
export type CursorKind =
  | 'default'
  | 'pointer'
  | 'text'
  | 'crosshair'
  | 'wait'
  | 'progress'
  | 'help'
  | 'move'
  | 'not_allowed'
  | 'grab'
  | 'grabbing'
  | 'col_resize'
  | 'row_resize'
  | 'nesw_resize'
  | 'nwse_resize'
  | 'zoom_in'
  | 'zoom_out';

/** Options for {@link useCefSocket}. */
export interface UseCefSocketOpts {
  windowId: string;
  paneId: string;
  activityId: string;
  /** WebSocket sub-path under `/windows/{wid}/panes/{pid}/activities/{aid}/`.
   *  Use `browser/ws` for Browser activities and `extension/cef/ws` for
   *  Extension activities. */
  path: string;
  worker: Worker | null;
  generation: number;
  /** Last frame the renderer holds, or null to request a fresh keyframe. */
  lastKey: FrameKey | null;
  /** Called when the daemon emits a Nav message (URL / title / can_back /
   *  can_forward). Consumers typically hoist this into a state setter. */
  onNav?: (nav: NavSnapshot) => void;
  /** Called when the daemon replies with MustRestart. The reason is one of
   *  `session_mismatch | epoch_mismatch | last_key_evicted`. */
  onMustRestart: (reason: string) => void;
  /** Called when the daemon broadcasts BrowserUnavailable (cef_host died). */
  onUnavailable?: (reason: BrowserUnavailableReason) => void;
  /** Called when the embedded page's mouse cursor changes. */
  onCursor?: (cursor: CursorKind) => void;
  /** Called right after the socket opens and Subscribe is sent. Consumers use
   *  it to (re-)send the current viewport size, since a Resize emitted before
   *  the socket was open would have been dropped. */
  onOpen?: () => void;
}

type SubscribeReplyMessage = {
  kind: 'subscribe_reply';
  session_id: bigint;
  result:
    | { kind: 'fresh_snapshot' }
    | { kind: 'resume_replay' }
    | { kind: 'must_restart'; reason: string }
    | { kind: 'awaiting_keyframe' };
};

/** Return value of {@link useCefSocket}. The `send` function pushes a
 *  `BrowserClientMsg` on the live socket; it no-ops when the socket is not
 *  yet open or has closed. */
export interface UseCefSocketReturn {
  send: (msg: BrowserClientMsg) => void;
}

/** React hook that opens a CEF screencast WebSocket and pumps screencast
 *  frames into the supplied worker. */
export function useCefSocket(opts: UseCefSocketOpts): UseCefSocketReturn {
  const {
    windowId,
    paneId,
    activityId,
    path,
    worker,
    generation,
    lastKey,
    onMustRestart,
    onNav,
    onUnavailable,
    onOpen,
    onCursor,
  } = opts;
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!worker) return;
    const url = `ws://${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/${path}`;
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    wsRef.current = ws;

    const sendMsg = (msg: BrowserClientMsg) => {
      if (ws.readyState !== ws.OPEN) return;
      const payload = encode(msg);
      const buf = payload.buffer.slice(
        payload.byteOffset,
        payload.byteOffset + payload.byteLength,
      ) as ArrayBuffer;
      ws.send(buf);
    };

    ws.onopen = () => {
      sendMsg({
        kind: 'subscribe',
        session_id: lastKey?.session_id ?? null,
        last_key: lastKey,
        has_base_keyframe: lastKey !== null,
      });
      onOpen?.();
    };

    ws.onmessage = (ev: MessageEvent<ArrayBuffer>) => {
      // NOTE: peek up to the first 256 bytes to recognise SubscribeReply.
      // Screencast frames may carry large BGRA blobs; we don't want to
      // decode them on the main thread or copy them unnecessarily.
      const peekLen = Math.min(ev.data.byteLength, 256);
      const head = new Uint8Array(ev.data, 0, peekLen);
      let kind: string | undefined;
      try {
        kind = (decode(head) as { kind?: string }).kind;
      } catch {
        // NOTE: decode of a truncated head may fail; if so assume the message
        // is a screencast and let the worker handle it.
      }

      if (kind === 'subscribe_reply') {
        let reply: SubscribeReplyMessage | null = null;
        try {
          reply = decode(new Uint8Array(ev.data)) as SubscribeReplyMessage;
        } catch (e) {
          console.warn('subscribe_reply decode failed', e);
          return;
        }
        if (reply.result.kind === 'must_restart') {
          onMustRestart(reply.result.reason);
        }
        return;
      }
      if (kind === 'nav') {
        try {
          const nav = decode(new Uint8Array(ev.data)) as {
            kind: 'nav';
            url: string;
            title: string;
            can_back: boolean;
            can_forward: boolean;
          };
          onNav?.(nav);
        } catch (e) {
          console.warn('nav decode failed', e);
        }
        return;
      }
      if (kind === 'browser_unavailable') {
        try {
          const msg = decode(new Uint8Array(ev.data)) as {
            kind: 'browser_unavailable';
            reason: BrowserUnavailableReason;
          };
          onUnavailable?.(msg.reason);
        } catch (e) {
          console.warn('browser_unavailable decode failed', e);
        }
        return;
      }
      if (kind === 'cursor') {
        try {
          const msg = decode(new Uint8Array(ev.data)) as { kind: 'cursor'; cursor: CursorKind };
          onCursor?.(msg.cursor);
        } catch (e) {
          console.warn('cursor decode failed', e);
        }
        return;
      }
      worker.postMessage({ type: 'wsBinary', generation, buffer: ev.data }, [ev.data]);
    };

    ws.onerror = (ev) => {
      console.warn('cef ws error', ev);
    };

    return () => {
      wsRef.current = null;
      ws.close();
    };
  }, [
    windowId,
    paneId,
    activityId,
    path,
    worker,
    generation,
    lastKey,
    onMustRestart,
    onNav,
    onUnavailable,
    onOpen,
    onCursor,
  ]);

  const send = (msg: BrowserClientMsg) => {
    const ws = wsRef.current;
    if (!ws || ws.readyState !== ws.OPEN) return;
    const payload = encode(msg);
    const buf = payload.buffer.slice(
      payload.byteOffset,
      payload.byteOffset + payload.byteLength,
    ) as ArrayBuffer;
    ws.send(buf);
  };

  return { send };
}
