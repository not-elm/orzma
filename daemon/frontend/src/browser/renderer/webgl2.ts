// WebGl2Renderer — BGRA-as-RGBA upload with .bgr swizzle in the fragment
// shader. Persistent RGBA8 backing texture + fullscreen quad blit. Same
// session/epoch/seq state machine as WebGpuRenderer so keyframe and delta
// frames are gated identically across renderers.

import type { FrameEnvelope, FrameRenderer } from './factory';

const VS = `#version 300 es
out vec2 vUv;
void main() {
  vec2 p[6] = vec2[](
    vec2(-1.0, -1.0), vec2( 1.0, -1.0), vec2(-1.0,  1.0),
    vec2(-1.0,  1.0), vec2( 1.0, -1.0), vec2( 1.0,  1.0)
  );
  vec2 uvs[6] = vec2[](
    vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 0.0),
    vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(1.0, 0.0)
  );
  gl_Position = vec4(p[gl_VertexID], 0.0, 1.0);
  vUv = uvs[gl_VertexID];
}`;

const FS = `#version 300 es
precision mediump float;
in vec2 vUv;
uniform sampler2D uTex;
out vec4 oColor;
void main() {
  vec4 c = texture(uTex, vUv);
  oColor = vec4(c.bgr, 1.0);
}`;

export class WebGl2Renderer implements FrameRenderer {
  private gl!: WebGL2RenderingContext;
  private backingTex!: WebGLTexture;
  private program!: WebGLProgram;
  private size = { w: 0, h: 0 };
  private hasKeyframe = false;
  private currentSessionId = 0n;
  private currentEpoch = 0;
  private lastFrameSeq = 0n;

  async init(canvas: OffscreenCanvas, w: number, h: number): Promise<void> {
    const gl = canvas.getContext('webgl2', {
      alpha: false,
      antialias: false,
    }) as WebGL2RenderingContext | null;
    if (!gl) throw new Error('webgl2 unavailable');
    this.gl = gl;
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    const tex = gl.createTexture();
    if (!tex) throw new Error('createTexture failed');
    this.backingTex = tex;
    gl.bindTexture(gl.TEXTURE_2D, this.backingTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.program = compileProgram(gl, VS, FS);
    this.size = { w, h };
  }

  renderFrame(f: FrameEnvelope): void {
    const isReset =
      f.session_id !== this.currentSessionId ||
      f.epoch !== this.currentEpoch ||
      f.width !== this.size.w ||
      f.height !== this.size.h;

    if (isReset) {
      if (!f.is_keyframe) return;
      const gl = this.gl;
      // Resize the OffscreenCanvas drawing buffer to match the frame.
      gl.canvas.width = f.width;
      gl.canvas.height = f.height;
      gl.bindTexture(gl.TEXTURE_2D, this.backingTex);
      gl.texImage2D(
        gl.TEXTURE_2D,
        0,
        gl.RGBA8,
        f.width,
        f.height,
        0,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        null,
      );
      this.size = { w: f.width, h: f.height };
      this.currentSessionId = f.session_id;
      this.currentEpoch = f.epoch;
      this.lastFrameSeq = 0n;
      this.hasKeyframe = false;
    }

    if (!f.is_keyframe && !this.hasKeyframe) return;
    if (!f.is_keyframe && f.frame_seq <= this.lastFrameSeq) return;

    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.backingTex);
    if (f.is_keyframe) {
      gl.texSubImage2D(
        gl.TEXTURE_2D,
        0,
        0,
        0,
        f.width,
        f.height,
        gl.RGBA,
        gl.UNSIGNED_BYTE,
        f.bgra,
      );
      this.hasKeyframe = true;
    } else {
      let offset = 0;
      for (const r of f.damage_rects) {
        const view = new Uint8Array(f.bgra.buffer, f.bgra.byteOffset + offset, r.w * r.h * 4);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, r.x, r.y, r.w, r.h, gl.RGBA, gl.UNSIGNED_BYTE, view);
        offset += r.w * r.h * 4;
      }
    }
    this.lastFrameSeq = f.frame_seq;
    this.draw();
  }

  async destroy(): Promise<void> {
    const gl = this.gl;
    gl.deleteTexture(this.backingTex);
    gl.deleteProgram(this.program);
  }

  private draw(): void {
    const gl = this.gl;
    gl.viewport(0, 0, this.size.w, this.size.h);
    // biome-ignore lint/correctness/useHookAtTopLevel: gl.useProgram is a WebGL2 API, not a React hook
    gl.useProgram(this.program);
    gl.bindTexture(gl.TEXTURE_2D, this.backingTex);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
  }
}

function compileProgram(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const v = compileShader(gl, gl.VERTEX_SHADER, vs);
  const f = compileShader(gl, gl.FRAGMENT_SHADER, fs);
  const p = gl.createProgram();
  if (!p) throw new Error('createProgram failed');
  gl.attachShader(p, v);
  gl.attachShader(p, f);
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
    throw new Error(`program link failed: ${gl.getProgramInfoLog(p)}`);
  }
  return p;
}

function compileShader(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type);
  if (!sh) throw new Error('createShader failed');
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    throw new Error(`shader compile failed: ${gl.getShaderInfoLog(sh)}`);
  }
  return sh;
}
