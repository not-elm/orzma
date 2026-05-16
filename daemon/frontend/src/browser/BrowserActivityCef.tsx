// CEF-backed BrowserActivity (PoC). Owns a `<canvas>` whose control is
// transferred to a Web Worker on mount; the worker decodes msgpack
// BrowserServerMsg frames via WebGpuRenderer.
//
// PoC scope: no toolbar, no input forwarding, fixed 1280×800 viewport.
// The full toolbar / input / IME story lands in Plan 2 alongside the
// matching cef_host input plumbing.

import { useEffect, useRef, useState } from 'react';
import { useBrowserSocketCef } from './useBrowserSocketCef';

interface Props {
  windowId: string;
  paneId: string;
  activityId: string;
}

interface WorkerHandle {
  worker: Worker;
  generation: number;
}

const POC_WIDTH = 1280;
const POC_HEIGHT = 800;

export function BrowserActivityCef({ windowId, paneId, activityId }: Props) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [handle, setHandle] = useState<WorkerHandle | null>(null);
  const [unsupported, setUnsupported] = useState(false);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const offscreen = canvas.transferControlToOffscreen();
    const w = new Worker(new URL('./worker/frame-worker.ts', import.meta.url), {
      type: 'module',
    });
    const generation = 1;
    w.onmessage = (ev: MessageEvent<{ type: string }>) => {
      if (ev.data.type === 'unsupported') {
        setUnsupported(true);
      }
    };
    w.postMessage(
      {
        type: 'init',
        generation,
        canvas: offscreen,
        width: POC_WIDTH,
        height: POC_HEIGHT,
      },
      [offscreen],
    );
    setHandle({ worker: w, generation });
    return () => {
      w.postMessage({ type: 'dispose' });
      w.terminate();
    };
  }, []);

  useBrowserSocketCef(
    windowId,
    paneId,
    activityId,
    handle?.worker ?? null,
    handle?.generation ?? 0,
  );

  return (
    <div className="bg-background text-foreground flex h-full w-full items-center justify-center">
      {unsupported ? (
        <div className="text-destructive">WebGPU is not available in this browser.</div>
      ) : (
        <canvas
          ref={canvasRef}
          width={POC_WIDTH}
          height={POC_HEIGHT}
          className="block max-h-full max-w-full"
        />
      )}
    </div>
  );
}
