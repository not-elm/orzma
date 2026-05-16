/// <reference lib="webworker" />
// Frame worker for the cef-backed BrowserActivity.
//
// Receives transferred OffscreenCanvas instances + WS msgpack binaries on the
// main thread. Constructs two FrameRenderer instances (main viewport and popup
// overlay). Dispatches each decoded Screencast frame to the appropriate renderer
// based on `is_popup`. Popup rect changes are forwarded to the main thread as a
// `popup_rect` message so the main thread can reposition the overlay canvas.
//
// `generation` is incremented by the main thread on every reset (activity
// switch / resize) so late-arriving messages from a stale renderer can be
// discarded.

import { decode } from 'msgpackr';
import { createRenderer, type FrameEnvelope, type FrameRenderer } from '../renderer/factory';

type InitMsg = {
  type: 'init';
  generation: number;
  mainCanvas: OffscreenCanvas;
  popupCanvas: OffscreenCanvas;
  width: number;
  height: number;
};

type WsBinaryMsg = {
  type: 'wsBinary';
  generation: number;
  buffer: ArrayBuffer;
};

type DisposeMsg = {
  type: 'dispose';
};

type IncomingMsg = InitMsg | WsBinaryMsg | DisposeMsg;

type WireRect = { x: number; y: number; w: number; h: number };

type ScreencastWire = {
  kind: 'screencast';
  session_id: bigint;
  epoch: number;
  frame_seq: bigint;
  captured_at_us: bigint;
  width: number;
  height: number;
  is_keyframe: boolean;
  damage_rects: WireRect[];
  is_popup: boolean;
  popup_rect: WireRect | null | undefined;
  bgra: Uint8Array;
};

const POPUP_CANVAS_WIDTH = 800;
const POPUP_CANVAS_HEIGHT = 600;

let currentGeneration = -1;
let mainRenderer: FrameRenderer | null = null;
let popupRenderer: FrameRenderer | null = null;

self.onmessage = async (e: MessageEvent<IncomingMsg>) => {
  const msg = e.data;

  if (msg.type === 'dispose') {
    await mainRenderer?.destroy();
    await popupRenderer?.destroy();
    mainRenderer = null;
    popupRenderer = null;
    return;
  }

  if (msg.type !== 'init' && msg.generation !== currentGeneration) return;

  switch (msg.type) {
    case 'init': {
      currentGeneration = msg.generation;
      await mainRenderer?.destroy();
      await popupRenderer?.destroy();
      mainRenderer = await createRenderer(msg.mainCanvas, msg.width, msg.height);
      popupRenderer = await createRenderer(
        msg.popupCanvas,
        POPUP_CANVAS_WIDTH,
        POPUP_CANVAS_HEIGHT,
      );
      if (!mainRenderer || !popupRenderer) {
        self.postMessage({ type: 'unsupported', generation: currentGeneration });
      }
      return;
    }
    case 'wsBinary': {
      const decoded = decode(new Uint8Array(msg.buffer)) as { kind?: string };
      if (decoded.kind === 'screencast') {
        const wire = decoded as ScreencastWire;
        const envelope: FrameEnvelope = {
          session_id: wire.session_id,
          epoch: wire.epoch,
          frame_seq: wire.frame_seq,
          captured_at_us: wire.captured_at_us,
          width: wire.width,
          height: wire.height,
          is_keyframe: wire.is_keyframe,
          damage_rects: wire.damage_rects,
          bgra: wire.bgra,
        };

        if (wire.is_popup) {
          popupRenderer?.renderFrame(envelope);
          // NOTE: forward popup_rect to the main thread so the overlay canvas
          // can be repositioned. Sent on every popup frame — cheap since popup
          // frames are rare (user interaction only).
          self.postMessage({
            type: 'popup_rect',
            rect: wire.popup_rect ?? null,
          });
        } else {
          mainRenderer?.renderFrame(envelope);
          // NOTE: paint-done is consumed by the KPI smoke harness (Task 30) to
          // measure wheel→paint latency. Cheap on the hot path — one postMessage
          // per frame, no allocations beyond the small literal object.
          self.postMessage({
            type: 'paint-done',
            generation: currentGeneration,
            frame_seq: wire.frame_seq,
          });
        }
      }
      return;
    }
  }
};
