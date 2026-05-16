// CEF-backed BrowserActivity (PoC). Owns a `<canvas>` whose control is
// transferred to a Web Worker on mount; the worker decodes msgpack
// BrowserServerMsg frames via WebGpuRenderer.
//
// PoC scope: no toolbar, no input forwarding, fixed 1280×800 viewport.
// The full toolbar / input / IME story lands in Plan 2 alongside the
// matching cef_host input plumbing.
//
// Task A10: when the daemon answers with `SubscribeReply::MustRestart`,
// we bump `restartId` to remount the canvas + recreate the worker + force
// a fresh subscription (with `last_key=null`).

import { useCallback, useEffect, useRef, useState } from 'react';
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
  // Bumped on SubscribeReply::MustRestart so the canvas remounts (key change)
  // and the worker is recreated (effect re-runs).
  const [restartId, setRestartId] = useState(0);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let offscreen: OffscreenCanvas;
    try {
      offscreen = canvas.transferControlToOffscreen();
    } catch (e) {
      console.warn('transferControlToOffscreen failed; skipping worker init', e);
      return;
    }
    const w = new Worker(new URL('./worker/frame-worker.ts', import.meta.url), {
      type: 'module',
    });
    const generation = restartId + 1;
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
      setHandle(null);
    };
  }, [restartId]);

  const onMustRestart = useCallback((reason: string) => {
    console.warn('SubscribeReply::MustRestart', reason);
    setRestartId((id) => id + 1);
  }, []);

  useBrowserSocketCef({
    windowId,
    paneId,
    activityId,
    worker: handle?.worker ?? null,
    generation: handle?.generation ?? 0,
    // PoC: always re-subscribe fresh. Plan 2 wires persistence of the
    // most-recent FrameKey across reconnects so ResumeReplay can fire.
    lastKey: null,
    onMustRestart,
  });

  return (
    <div className="bg-background text-foreground flex h-full w-full items-center justify-center">
      {unsupported ? (
        <div className="text-destructive">WebGPU is not available in this browser.</div>
      ) : (
        <canvas
          key={restartId}
          ref={canvasRef}
          width={POC_WIDTH}
          height={POC_HEIGHT}
          className="block max-h-full max-w-full"
        />
      )}
    </div>
  );
}
