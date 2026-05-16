// WebGpuRenderer — persistent BGRA backing texture + fullscreen-quad blit.
//
// Keyframes wholesale-write the backing texture; deltas re-write only the
// damage rects in order, treating the BGRA blob as a tight concatenation of
// rect pixel buffers. The fragment shader samples the backing texture into
// the swap chain each frame.

import type { FrameEnvelope, FrameRenderer } from './factory';

export class WebGpuRenderer implements FrameRenderer {
  private size = { w: 0, h: 0 };
  private backingTexture!: GPUTexture;
  private pipeline!: GPURenderPipeline;
  private bindGroup!: GPUBindGroup;
  private sampler!: GPUSampler;
  private canvasFormat!: GPUTextureFormat;
  private hasKeyframe = false;
  private currentSessionId = 0n;
  private currentEpoch = 0;
  private lastFrameSeq = 0n;

  private readonly device: GPUDevice;
  private readonly ctx: GPUCanvasContext;
  private readonly gpu: GPU;

  constructor(device: GPUDevice, ctx: GPUCanvasContext, gpu: GPU) {
    this.device = device;
    this.ctx = ctx;
    this.gpu = gpu;
  }

  async init(_canvas: OffscreenCanvas, w: number, h: number): Promise<void> {
    this.canvasFormat = this.gpu.getPreferredCanvasFormat();
    this.ctx.configure({
      device: this.device,
      format: this.canvasFormat,
      alphaMode: 'opaque',
    });
    this.recreateBackingTexture(w, h);
    this.buildPipeline();
  }

  renderFrame(f: FrameEnvelope): void {
    const isReset =
      f.session_id !== this.currentSessionId ||
      f.epoch !== this.currentEpoch ||
      f.width !== this.size.w ||
      f.height !== this.size.h;

    if (isReset) {
      if (!f.is_keyframe) return;
      this.recreateBackingTexture(f.width, f.height);
      this.buildPipeline();
      this.currentSessionId = f.session_id;
      this.currentEpoch = f.epoch;
      this.lastFrameSeq = 0n;
      this.hasKeyframe = false;
    }

    if (!f.is_keyframe && !this.hasKeyframe) return;
    if (!f.is_keyframe && f.frame_seq <= this.lastFrameSeq) return;

    if (f.is_keyframe) {
      this.device.queue.writeTexture(
        { texture: this.backingTexture },
        f.bgra,
        { bytesPerRow: f.width * 4, rowsPerImage: f.height },
        { width: f.width, height: f.height, depthOrArrayLayers: 1 },
      );
      this.hasKeyframe = true;
    } else {
      let offset = 0;
      for (const r of f.damage_rects) {
        const view = new Uint8Array(f.bgra.buffer, f.bgra.byteOffset + offset, r.w * r.h * 4);
        this.device.queue.writeTexture(
          { texture: this.backingTexture, origin: { x: r.x, y: r.y } },
          view,
          { bytesPerRow: r.w * 4, rowsPerImage: r.h },
          { width: r.w, height: r.h, depthOrArrayLayers: 1 },
        );
        offset += r.w * r.h * 4;
      }
    }
    this.lastFrameSeq = f.frame_seq;
    this.drawToCanvas();
  }

  async destroy(): Promise<void> {
    this.backingTexture?.destroy();
    this.device.destroy();
  }

  private recreateBackingTexture(w: number, h: number): void {
    this.backingTexture?.destroy();
    this.backingTexture = this.device.createTexture({
      size: { width: w, height: h, depthOrArrayLayers: 1 },
      format: 'bgra8unorm',
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST,
    });
    this.size = { w, h };
    this.sampler = this.device.createSampler({ minFilter: 'linear', magFilter: 'linear' });
  }

  private buildPipeline(): void {
    const wgsl = `
      @vertex
      fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
        let p = array<vec2f, 6>(
          vec2f(-1.0, -1.0), vec2f( 1.0, -1.0), vec2f(-1.0,  1.0),
          vec2f(-1.0,  1.0), vec2f( 1.0, -1.0), vec2f( 1.0,  1.0),
        );
        return vec4f(p[i], 0.0, 1.0);
      }
      @group(0) @binding(0) var samp: sampler;
      @group(0) @binding(1) var tex: texture_2d<f32>;
      @fragment
      fn fs(@builtin(position) fc: vec4f) -> @location(0) vec4f {
        let uv = vec2f(fc.x / ${this.size.w}.0, fc.y / ${this.size.h}.0);
        return textureSample(tex, samp, uv);
      }
    `;
    const mod = this.device.createShaderModule({ code: wgsl });
    this.pipeline = this.device.createRenderPipeline({
      layout: 'auto',
      vertex: { module: mod, entryPoint: 'vs' },
      fragment: {
        module: mod,
        entryPoint: 'fs',
        targets: [{ format: this.canvasFormat }],
      },
      primitive: { topology: 'triangle-list' },
    });
    this.bindGroup = this.device.createBindGroup({
      layout: this.pipeline.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: this.sampler },
        { binding: 1, resource: this.backingTexture.createView() },
      ],
    });
  }

  private drawToCanvas(): void {
    const tex = this.ctx.getCurrentTexture();
    const encoder = this.device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: tex.createView(),
          loadOp: 'clear',
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
          storeOp: 'store',
        },
      ],
    });
    pass.setPipeline(this.pipeline);
    pass.setBindGroup(0, this.bindGroup);
    pass.draw(6);
    pass.end();
    this.device.queue.submit([encoder.finish()]);
  }
}
