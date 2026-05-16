// CEF-backed BrowserActivity. Owns a `<canvas>` whose control is
// transferred to a Web Worker on mount; the worker decodes msgpack
// BrowserServerMsg frames via WebGpuRenderer.
//
// Task A10: when the daemon answers with `SubscribeReply::MustRestart`,
// we bump `restartId` to remount the canvas + recreate the worker + force
// a fresh subscription (with `last_key=null`).
//
// Task B7: an invisible overlay <div> absorbs mouse + keyboard events;
// a hidden <textarea> collects IME composition. Both route through the
// `send` function returned by useBrowserSocketCef.

import { useCallback, useEffect, useRef, useState } from 'react';
import { ContextMenu } from './ContextMenu';
import { attachIme } from './input-cef/ime';
import { attachKeyboard } from './input-cef/keyboard';
import { attachMouse } from './input-cef/mouse';
import { attachWheel } from './input-cef/wheel';
import { Toolbar } from './Toolbar';
import type { BrowserClientMsg, NavSnapshot } from './useBrowserSocketCef';
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
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [handle, setHandle] = useState<WorkerHandle | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  // Bumped on SubscribeReply::MustRestart so the canvas remounts (key change)
  // and the worker is recreated (effect re-runs).
  const [restartId, setRestartId] = useState(0);
  const [nav, setNav] = useState<NavSnapshot>({
    url: '',
    title: '',
    can_back: false,
    can_forward: false,
  });
  const [ctx, setCtx] = useState<{ x: number; y: number } | null>(null);

  // Stable ref to the send function so the input-attach effect does not
  // re-fire every render when `send` identity changes.
  const sendRef = useRef<((msg: BrowserClientMsg) => void) | null>(null);

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
      } else if (ev.data.type === 'paint-done') {
        // NOTE: exposed for the Playwright e2e in browser-cef-poc.spec.ts
        // (Task A16). Counts every successful render the worker reports.
        const w = window as unknown as { __poc_paint_done_count?: number };
        w.__poc_paint_done_count = (w.__poc_paint_done_count ?? 0) + 1;
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

  const onNav = useCallback((next: NavSnapshot) => {
    setNav(next);
  }, []);

  const { send } = useBrowserSocketCef({
    windowId,
    paneId,
    activityId,
    worker: handle?.worker ?? null,
    generation: handle?.generation ?? 0,
    // PoC: always re-subscribe fresh. Plan 2 wires persistence of the
    // most-recent FrameKey across reconnects so ResumeReplay can fire.
    lastKey: null,
    onMustRestart,
    onNav,
  });

  // Keep sendRef in sync without triggering the attach effect.
  sendRef.current = send;

  // Observe overlay size and report Resize to cef_host whenever it changes.
  useEffect(() => {
    const overlay = overlayRef.current;
    if (!overlay) return;
    const ro = new ResizeObserver(() => {
      const r = overlay.getBoundingClientRect();
      sendRef.current?.({
        kind: 'resize',
        css_w: Math.max(1, Math.round(r.width)),
        css_h: Math.max(1, Math.round(r.height)),
        dpr: window.devicePixelRatio,
      });
    });
    ro.observe(overlay);
    return () => ro.disconnect();
  }, []);

  // Wire input attach helpers when the worker is live.
  useEffect(() => {
    if (!handle?.worker) return;
    const overlay = overlayRef.current;
    const textarea = textareaRef.current;
    if (!overlay || !textarea) return;

    const inputSink = (ev: import('./protocol/input').InputEvent) => {
      sendRef.current?.({ kind: 'input', event: ev });
    };

    const detachMouse = attachMouse({
      send: inputSink,
      element: overlay,
      dpr: () => window.devicePixelRatio,
    });
    const detachWheel = attachWheel({
      send: inputSink,
      element: overlay,
      dpr: () => window.devicePixelRatio,
    });
    const detachKeyboard = attachKeyboard({
      send: inputSink,
      element: overlay,
      focusOnEditable: () => document.activeElement === textarea,
    });
    const detachIme = attachIme({ send: inputSink, textarea });

    overlay.focus();

    return () => {
      detachMouse();
      detachWheel();
      detachKeyboard();
      detachIme();
    };
  }, [handle?.worker]);

  return (
    <div className="bg-background text-foreground flex h-full w-full flex-col">
      {unsupported ? (
        <div className="text-destructive flex flex-1 items-center justify-center">
          WebGPU is not available in this browser.
        </div>
      ) : (
        <>
          <Toolbar
            url={nav.url}
            canBack={nav.can_back}
            canForward={nav.can_forward}
            onBack={() => send({ kind: 'navigate_history', delta: -1 })}
            onForward={() => send({ kind: 'navigate_history', delta: 1 })}
            onReload={() => send({ kind: 'navigate', url: nav.url })}
            onGo={(url) => send({ kind: 'navigate', url })}
          />
          <div className="relative flex-1 flex items-center justify-center">
            <canvas
              key={restartId}
              ref={canvasRef}
              width={POC_WIDTH}
              height={POC_HEIGHT}
              className="block max-h-full max-w-full"
            />
            {/* Overlay div absorbs mouse and keyboard events for the embedded browser. */}
            <div
              ref={overlayRef}
              // NOTE: role="application" marks this as an interactive widget zone;
              // required because onContextMenu would otherwise violate the
              // a11y/noStaticElementInteractions lint rule.
              role="application"
              aria-label="Browser viewport"
              // biome-ignore lint/a11y/noNoninteractiveTabindex: overlay must be focusable to receive keyboard events
              tabIndex={0}
              className="absolute inset-0 outline-none"
              onContextMenu={(e) => {
                e.preventDefault();
                setCtx({ x: e.clientX, y: e.clientY });
              }}
            />
            {/* Hidden textarea captures IME composition events. */}
            <textarea
              ref={textareaRef}
              tabIndex={-1}
              className="absolute inset-0 opacity-0 pointer-events-none"
              aria-hidden="true"
              readOnly={false}
            />
            {ctx && (
              <ContextMenu
                x={ctx.x}
                y={ctx.y}
                onClose={() => setCtx(null)}
                onBack={() => send({ kind: 'navigate_history', delta: -1 })}
                onForward={() => send({ kind: 'navigate_history', delta: 1 })}
                onReload={() => send({ kind: 'navigate', url: nav.url })}
                // TODO: Plan 3 wires the full clipboard round-trip for the CEF path.
                onCopy={() => send({ kind: 'copy_request' })}
                onPaste={() => {
                  navigator.clipboard.readText().then(
                    (t) => send({ kind: 'paste', text: t }),
                    () => {
                      // NOTE: clipboard read may be denied (permissions, focus) — ignore.
                    },
                  );
                }}
              />
            )}
          </div>
        </>
      )}
    </div>
  );
}
