// Cef-path WebSocket hook. Connects to /browser_cef/ws, sends Subscribe on
// open, and forwards every incoming binary frame to the supplied frame
// worker as a transferable ArrayBuffer.

import { encode } from 'msgpackr';
import { useEffect } from 'react';

export function useBrowserSocketCef(
  windowId: string,
  paneId: string,
  activityId: string,
  worker: Worker | null,
  generation: number,
): void {
  useEffect(() => {
    if (!worker) return;
    const url = `ws://${location.host}/windows/${windowId}/panes/${paneId}/activities/${activityId}/browser_cef/ws`;
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      const payload = encode({
        kind: 'subscribe',
        session_id: null,
        last_key: null,
        has_base_keyframe: false,
      });
      const buf = payload.buffer.slice(
        payload.byteOffset,
        payload.byteOffset + payload.byteLength,
      ) as ArrayBuffer;
      ws.send(buf);
    };

    ws.onmessage = (ev: MessageEvent<ArrayBuffer>) => {
      worker.postMessage({ type: 'wsBinary', generation, buffer: ev.data }, [ev.data]);
    };

    ws.onerror = (ev) => {
      console.warn('cef browser ws error', ev);
    };

    return () => {
      ws.close();
    };
  }, [windowId, paneId, activityId, worker, generation]);
}
