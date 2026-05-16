// Renderer factory for the cef-backed BrowserActivity screencast pipeline.
// PoC: only the WebGPU path is implemented; a WebGL2 fallback lands in Plan 2.

import { WebGpuRenderer } from './webgpu';

export interface FrameEnvelope {
  session_id: bigint;
  epoch: number;
  frame_seq: bigint;
  captured_at_us: bigint;
  width: number;
  height: number;
  is_keyframe: boolean;
  damage_rects: { x: number; y: number; w: number; h: number }[];
  bgra: Uint8Array;
}

export interface FrameRenderer {
  init(canvas: OffscreenCanvas, width: number, height: number): Promise<void>;
  renderFrame(frame: FrameEnvelope): void;
  destroy(): Promise<void>;
}

export async function createRenderer(
  canvas: OffscreenCanvas,
  width: number,
  height: number,
): Promise<FrameRenderer | null> {
  const gpu = (globalThis.navigator as Navigator & { gpu?: GPU }).gpu;
  if (!gpu) return null;
  try {
    const ctx = canvas.getContext('webgpu') as GPUCanvasContext | null;
    if (!ctx) return null;
    const adapter = await gpu.requestAdapter();
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    device.lost.then((info) => {
      console.warn('WebGPU device lost', info.reason, info.message);
    });
    const renderer = new WebGpuRenderer(device, ctx, gpu);
    await renderer.init(canvas, width, height);
    return renderer;
  } catch (e) {
    console.warn('WebGPU init failed', e);
    return null;
  }
}
