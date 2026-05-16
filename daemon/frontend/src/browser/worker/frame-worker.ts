/// <reference lib="webworker" />
// Frame worker for the cef-backed BrowserActivity.
//
// Receives transferred OffscreenCanvas + WS msgpack binaries on the main thread,
// constructs a WebGpuRenderer, and renders each Screencast frame off the main
// thread. `generation` is incremented by the main thread on every reset
// (activity switch / resize) so late-arriving messages from a stale renderer
// can be discarded.

import { decode } from 'msgpackr';
import { createRenderer, type FrameEnvelope, type FrameRenderer } from '../renderer/factory';

type InitMsg = {
  type: 'init';
  generation: number;
  canvas: OffscreenCanvas;
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

type ScreencastWire = {
  kind: 'screencast';
  session_id: bigint;
  epoch: number;
  frame_seq: bigint;
  captured_at_us: bigint;
  width: number;
  height: number;
  is_keyframe: boolean;
  damage_rects: { x: number; y: number; w: number; h: number }[];
  bgra: Uint8Array;
};

let currentGeneration = -1;
let renderer: FrameRenderer | null = null;

self.onmessage = async (e: MessageEvent<IncomingMsg>) => {
  const msg = e.data;

  if (msg.type === 'dispose') {
    await renderer?.destroy();
    renderer = null;
    return;
  }

  if (msg.type !== 'init' && msg.generation !== currentGeneration) return;

  switch (msg.type) {
    case 'init': {
      currentGeneration = msg.generation;
      await renderer?.destroy();
      renderer = await createRenderer(msg.canvas, msg.width, msg.height);
      if (!renderer) {
        self.postMessage({ type: 'unsupported', generation: currentGeneration });
      }
      return;
    }
    case 'wsBinary': {
      if (!renderer) return;
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
        renderer.renderFrame(envelope);
        // NOTE: paint-done is consumed by the KPI smoke harness (Task 30) to
        // measure wheel→paint latency. Cheap on the hot path — one postMessage
        // per frame, no allocations beyond the small literal object.
        self.postMessage({
          type: 'paint-done',
          generation: currentGeneration,
          frame_seq: wire.frame_seq,
        });
      }
      return;
    }
  }
};
