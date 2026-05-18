// Renderer factory for the cef-backed BrowserActivity screencast pipeline.
//
// Platform branch: Linux prefers WebGL2 (Mesa WebGPU is unstable per parent
// spec §8); macOS / Windows prefer WebGPU. Each path falls back to the other
// when init fails.

import { WebGl2Renderer } from './webgl2';
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

async function tryWebGpu(
  canvas: OffscreenCanvas,
  w: number,
  h: number,
): Promise<FrameRenderer | null> {
  const gpu = (globalThis.navigator as Navigator & { gpu?: GPU }).gpu;
  if (!gpu) return null;
  try {
    const ctx = canvas.getContext('webgpu') as GPUCanvasContext | null;
    if (!ctx) return null;
    const adapter = await gpu.requestAdapter({ powerPreference: 'high-performance' });
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    device.lost.then((info) => {
      console.warn('WebGPU device lost', info.reason, info.message);
    });
    const r = new WebGpuRenderer(device, ctx, gpu);
    await r.init(canvas, w, h);
    return r;
  } catch (e) {
    console.warn('WebGPU init failed', e);
    return null;
  }
}

async function tryWebGl2(
  canvas: OffscreenCanvas,
  w: number,
  h: number,
): Promise<FrameRenderer | null> {
  try {
    const r = new WebGl2Renderer();
    await r.init(canvas, w, h);
    return r;
  } catch (e) {
    console.warn('WebGL2 init failed', e);
    return null;
  }
}

export async function createRenderer(
  canvas: OffscreenCanvas,
  w: number,
  h: number,
): Promise<FrameRenderer | null> {
  const ua = (globalThis.navigator?.userAgent ?? '').toLowerCase();
  const isLinux = ua.includes('linux') && !ua.includes('android');
  if (isLinux) {
    return (await tryWebGl2(canvas, w, h)) ?? (await tryWebGpu(canvas, w, h));
  }
  return (await tryWebGpu(canvas, w, h)) ?? (await tryWebGl2(canvas, w, h));
}
