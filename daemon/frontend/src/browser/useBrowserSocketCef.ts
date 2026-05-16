// Cef-path WebSocket hook. Connects to /browser_cef/ws, sends Subscribe on
// open, and forwards every incoming binary frame to the supplied frame
// worker as a transferable ArrayBuffer.
//
// Peeks each incoming message's `kind` field on the main thread:
// SubscribeReply frames are consumed here (and dispatch MustRestart to the
// caller); all other frames are forwarded to the worker as transferable
// ArrayBuffers so decode stays off the main thread.

import { decode, encode } from 'msgpackr';
import { useEffect } from 'react';

export type FrameKey = {
  session_id: bigint;
  epoch: number;
  frame_seq: bigint;
};

export interface UseBrowserSocketCefOpts {
  windowId: string;
  paneId: string;
  activityId: string;
  worker: Worker | null;
  generation: number;
  /** Last frame the renderer holds, or null to request a fresh keyframe. */
  lastKey: FrameKey | null;
  /** Called when the daemon replies with MustRestart. The reason is one of
   *  `session_mismatch | epoch_mismatch | last_key_evicted`. */
  onMustRestart: (reason: string) => void;
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

export function useBrowserSocketCef(opts: UseBrowserSocketCefOpts): void {
  const { windowId, paneId, activityId, worker, generation, lastKey, onMustRestart } = opts;
  useEffect(() => {
    if (!worker) return;
    const url = `ws://${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/browser_cef/ws`;
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      const payload = encode({
        kind: 'subscribe',
        session_id: lastKey?.session_id ?? null,
        last_key: lastKey,
        has_base_keyframe: lastKey !== null,
      });
      const buf = payload.buffer.slice(
        payload.byteOffset,
        payload.byteOffset + payload.byteLength,
      ) as ArrayBuffer;
      ws.send(buf);
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
      worker.postMessage({ type: 'wsBinary', generation, buffer: ev.data }, [ev.data]);
    };

    ws.onerror = (ev) => {
      console.warn('cef browser ws error', ev);
    };

    return () => {
      ws.close();
    };
  }, [windowId, paneId, activityId, worker, generation, lastKey, onMustRestart]);
}
