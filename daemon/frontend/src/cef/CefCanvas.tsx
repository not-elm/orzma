//! Shared CEF screencast surface used by both Browser and Extension activities.
//!
//! Owns the two `<canvas>` elements (main viewport + popup overlay) whose
//! control is transferred to a Web Worker on mount, the input-overlay div,
//! the hidden IME textarea, and the WebSocket subscription. The only thing it
//! does not own is the per-activity chrome: Browser activities supply a
//! `renderHeader` callback to mount a URL/back/forward Toolbar above the
//! viewport, while Extension activities omit the callback and render the
//! canvas full-bleed.
//!
//! Refactored out of `BrowserActivity.tsx`. The behaviour (StrictMode-safe
//! worker lifecycle, MustRestart re-subscription, popup overlay positioning,
//! IME composition) is preserved verbatim.

import { type ReactNode, useEffect, useRef, useState } from 'react';
import { attachIme } from '../browser/input/ime';
import { attachKeyboard } from '../browser/input/keyboard';
import { attachMouse } from '../browser/input/mouse';
import { attachWheel } from '../browser/input/wheel';
import FrameWorker from '../browser/worker/frame-worker.ts?worker&inline';
import type {
  BrowserClientMsg,
  BrowserUnavailableReason,
  CursorKind,
  NavSnapshot,
} from './useCefSocket';
import { useCefSocket } from './useCefSocket';

// CursorKind → Tailwind cursor utility. Full literal class strings so the
// Tailwind content scanner picks them up. `.claude/rules/styling.md`: these
// are standard semantic utilities, not arbitrary values.
const CURSOR_CLASS: Record<CursorKind, string> = {
  default: 'cursor-default',
  pointer: 'cursor-pointer',
  text: 'cursor-text',
  crosshair: 'cursor-crosshair',
  wait: 'cursor-wait',
  progress: 'cursor-progress',
  help: 'cursor-help',
  move: 'cursor-move',
  not_allowed: 'cursor-not-allowed',
  grab: 'cursor-grab',
  grabbing: 'cursor-grabbing',
  col_resize: 'cursor-col-resize',
  row_resize: 'cursor-row-resize',
  nesw_resize: 'cursor-nesw-resize',
  nwse_resize: 'cursor-nwse-resize',
  zoom_in: 'cursor-zoom-in',
  zoom_out: 'cursor-zoom-out',
};

const INITIAL_CANVAS_WIDTH = 1280;
const INITIAL_CANVAS_HEIGHT = 800;

// NOTE: popup canvas backing size matches POPUP_PAYLOAD_MAX in shm_writer.rs
// (800×600 BGRA). For PoC the backing buffer is fixed and CSS-scaled via
// the style width/height when the popup is larger than this.
const POPUP_CANVAS_WIDTH = 800;
const POPUP_CANVAS_HEIGHT = 600;

interface WorkerHandle {
  worker: Worker;
  generation: number;
}

interface PopupRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Helpers handed to `renderHeader` so callers can drive navigation
 *  (back/forward/reload/go) and reflect server-driven URL state. */
export interface CefHeaderHelpers {
  nav: NavSnapshot;
  send: (msg: BrowserClientMsg) => void;
}

/** Optional render prop for activity-specific chrome above the canvas (e.g.
 *  the Browser Toolbar). Receives the current Nav snapshot and a `send`
 *  function for issuing BrowserClientMsg back to the daemon.
 *
 *  Omitted by Extension activities — they render their viewport full-bleed. */
export type RenderCefHeader = (helpers: CefHeaderHelpers) => ReactNode;

/** Render prop for an in-canvas overlay (e.g. the right-click ContextMenu).
 *  Receives the right-click coordinates and a `send` function. */
export type RenderCefOverlay = (helpers: {
  x: number;
  y: number;
  nav: NavSnapshot;
  send: (msg: BrowserClientMsg) => void;
  close: () => void;
}) => ReactNode;

/** Props for {@link CefCanvas}. */
export interface CefCanvasProps {
  windowId: string;
  paneId: string;
  activityId: string;
  /** WebSocket sub-path identifying the activity's screencast endpoint —
   *  `browser/ws` for Browser activities, `extension/cef/ws` for Extension
   *  activities. Both endpoints speak the same wire protocol. */
  path: string;
  /** Optional chrome rendered above the canvas. Hidden when the daemon
   *  reports the CEF browser as unavailable. */
  renderHeader?: RenderCefHeader;
  /** Optional overlay rendered on right-click. Hidden when the daemon
   *  reports the CEF browser as unavailable. */
  renderContextMenu?: RenderCefOverlay;
}

function reasonLabel(reason: BrowserUnavailableReason): string {
  switch (reason.kind) {
    case 'retry_exhausted':
      return `cef_host crashed: ${reason.last_error}. Restart the daemon.`;
    case 'extension_disconnected':
      return 'Extension disconnected. Restart the extension to recover.';
    default: {
      // Unknown reason kind — surface the discriminant so debugging is possible.
      const r = reason as { kind: string };
      return `Browser unavailable (${r.kind}).`;
    }
  }
}

/**
 * CEF-backed screencast canvas with worker, input handlers, popup overlay,
 * and WebSocket subscription. Shared between Browser and Extension activities;
 * the per-activity chrome is supplied via `renderHeader` / `renderContextMenu`.
 */
export function CefCanvas({
  windowId,
  paneId,
  activityId,
  path,
  renderHeader,
  renderContextMenu,
}: CefCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const popupCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const [handle, setHandle] = useState<WorkerHandle | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [unavailable, setUnavailable] = useState<BrowserUnavailableReason | null>(null);
  // Tailwind cursor utility for the overlay, driven by the embedded page.
  const [cursorClass, setCursorClass] = useState('cursor-default');
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
  // Popup overlay rect, delivered by the worker when popup frames arrive.
  // null means the popup is not visible — the canvas is hidden.
  const [popupRect, setPopupRect] = useState<PopupRect | null>(null);

  // Stable ref to the send function so the input-attach effect does not
  // re-fire every render when `send` identity changes.
  const sendRef = useRef<((msg: BrowserClientMsg) => void) | null>(null);

  // Worker session, keyed by the canvas DOM element it transferred control
  // from. transferControlToOffscreen is one-shot per element, so the session
  // must survive React StrictMode's setup→cleanup→setup double-invoke (same
  // DOM node) and is only rebuilt when restartId bumps (fresh canvas node).
  const sessionRef = useRef<{
    canvas: HTMLCanvasElement;
    worker: Worker;
    generation: number;
  } | null>(null);
  const teardownTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const popupCanvas = popupCanvasRef.current;
    if (!canvas || !popupCanvas) return;

    // A StrictMode re-setup (or a restartId change) runs synchronously after
    // the previous cleanup — cancel its deferred teardown so the worker lives.
    if (teardownTimerRef.current !== null) {
      clearTimeout(teardownTimerRef.current);
      teardownTimerRef.current = null;
    }

    // A session bound to a different canvas element means restartId bumped and
    // the old canvas DOM node was retired — tear that worker down for real.
    let session = sessionRef.current;
    if (session && session.canvas !== canvas) {
      session.worker.postMessage({ type: 'dispose' });
      session.worker.terminate();
      session = null;
      sessionRef.current = null;
    }

    if (!session) {
      let mainOffscreen: OffscreenCanvas;
      let popupOffscreen: OffscreenCanvas;
      try {
        mainOffscreen = canvas.transferControlToOffscreen();
        popupOffscreen = popupCanvas.transferControlToOffscreen();
      } catch (e) {
        console.warn('transferControlToOffscreen failed; skipping worker init', e);
        return;
      }
      const w = new FrameWorker();
      const generation = restartId + 1;
      w.onmessage = (ev: MessageEvent<{ type: string }>) => {
        if (ev.data.type === 'unsupported') {
          setUnsupported(true);
        } else if (ev.data.type === 'paint-done') {
          // NOTE: exposed for the Playwright e2e in browser-cef-poc.spec.ts —
          // counts every successful render the worker reports.
          const win = window as unknown as { __poc_paint_done_count?: number };
          win.__poc_paint_done_count = (win.__poc_paint_done_count ?? 0) + 1;
          // Append KPI entry when the paint is correlated to a wheel dispatch.
          const paintMsg = ev.data as { type: string; correlate_to: number | null; t: number };
          if (paintMsg.correlate_to != null) {
            const kpiWindow = window as unknown as {
              __poc_kpi?: Array<{ input_id: number; t_paint: number }>;
            };
            kpiWindow.__poc_kpi ??= [];
            kpiWindow.__poc_kpi.push({ input_id: paintMsg.correlate_to, t_paint: paintMsg.t });
          }
        } else if (ev.data.type === 'popup_rect') {
          const msg = ev.data as { type: 'popup_rect'; rect: PopupRect | null };
          setPopupRect(msg.rect);
        }
      };
      w.postMessage(
        {
          type: 'init',
          generation,
          mainCanvas: mainOffscreen,
          popupCanvas: popupOffscreen,
          width: INITIAL_CANVAS_WIDTH,
          height: INITIAL_CANVAS_HEIGHT,
        },
        [mainOffscreen, popupOffscreen],
      );
      session = { canvas, worker: w, generation };
      sessionRef.current = session;
    }

    setHandle({ worker: session.worker, generation: session.generation });

    return () => {
      // Defer teardown by one macrotask. A StrictMode re-setup (or a restartId
      // change) runs synchronously next and cancels this timer, keeping the
      // worker alive. A real unmount has no follow-up setup, so it fires.
      teardownTimerRef.current = setTimeout(() => {
        const s = sessionRef.current;
        if (s) {
          s.worker.postMessage({ type: 'dispose' });
          s.worker.terminate();
          sessionRef.current = null;
        }
        teardownTimerRef.current = null;
        setHandle(null);
        setPopupRect(null);
      }, 0);
    };
  }, [restartId]);

  // Reports the overlay's current CSS size to cef_host. Called by the
  // ResizeObserver on every size change and once on socket open (a Resize
  // emitted before the socket opened would have been dropped).
  const emitResize = () => {
    const overlay = overlayRef.current;
    if (!overlay) return;
    const r = overlay.getBoundingClientRect();
    sendRef.current?.({
      kind: 'resize',
      css_w: Math.max(1, Math.round(r.width)),
      css_h: Math.max(1, Math.round(r.height)),
      dpr: window.devicePixelRatio,
    });
  };

  const onMustRestart = (reason: string) => {
    console.warn('SubscribeReply::MustRestart', reason);
    setRestartId((id) => id + 1);
  };

  const onNav = (next: NavSnapshot) => {
    setNav(next);
  };

  const onUnavailable = (reason: BrowserUnavailableReason) => {
    setUnavailable(reason);
  };

  const onCursor = (cursor: CursorKind) => {
    setCursorClass(CURSOR_CLASS[cursor] ?? 'cursor-default');
  };

  const { send } = useCefSocket({
    windowId,
    paneId,
    activityId,
    path,
    worker: handle?.worker ?? null,
    generation: handle?.generation ?? 0,
    // TODO: persist the most-recent FrameKey across reconnects so the
    // daemon can hand back ResumeReplay instead of a fresh snapshot.
    lastKey: null,
    onMustRestart,
    onNav,
    onUnavailable,
    onOpen: emitResize,
    onCursor,
  });

  // Keep sendRef in sync without triggering the attach effect.
  sendRef.current = send;

  // Observe overlay size and report Resize to cef_host whenever it changes.
  // NOTE: the body only touches refs, so empty deps + the stale closure are
  // intentional; emitResize is not used here to keep deps lint-clean.
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

    const inputSink = (ev: import('../browser/protocol/input').InputEvent) => {
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
      worker: handle.worker,
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

  if (unavailable) {
    return (
      <div className="bg-background text-foreground flex h-full w-full flex-col">
        <div className="text-destructive flex flex-1 items-center justify-center p-4 text-center">
          {reasonLabel(unavailable)}
        </div>
      </div>
    );
  }
  if (unsupported) {
    return (
      <div className="bg-background text-foreground flex h-full w-full flex-col">
        <div className="text-destructive flex flex-1 items-center justify-center">
          WebGPU is not available in this browser.
        </div>
      </div>
    );
  }

  return (
    <div className="bg-background text-foreground flex h-full w-full flex-col">
      {renderHeader?.({ nav, send })}
      {/* `min-h-0 min-w-0` lets this flex child shrink below the canvas's
          intrinsic 1280×800 size — without it the replaced-element
          min-content height overflows the column and hides the Toolbar. */}
      <div className="relative flex-1 min-h-0 min-w-0">
        {/* The backing buffer starts at INITIAL_CANVAS_WIDTH×INITIAL_CANVAS_HEIGHT and the
            renderer resizes it (via the OffscreenCanvas) to each frame's
            device-pixel size — which tracks the pane because the daemon
            clamps the cef viewport to css×dpr. `h-full w-full` fills the
            pane; aspect matches since the cef viewport matches the pane. */}
        <canvas
          key={`main-${restartId}`}
          ref={canvasRef}
          width={INITIAL_CANVAS_WIDTH}
          height={INITIAL_CANVAS_HEIGHT}
          className="block h-full w-full"
        />
        {/* Popup overlay canvas — keyed on restartId so a MustRestart
            produces a fresh DOM node; transferControlToOffscreen is
            one-shot per element. Hidden via `hidden` utility when no popup
            is active; positioned via inline style (runtime-computed CEF
            rect) when visible. */}
        <canvas
          key={`popup-${restartId}`}
          ref={popupCanvasRef}
          width={POPUP_CANVAS_WIDTH}
          height={POPUP_CANVAS_HEIGHT}
          className={popupRect ? 'absolute block' : 'absolute hidden'}
          // biome-ignore lint/plugin: popup overlay anchored to runtime-computed CEF rect — cannot use Tailwind utilities for arbitrary x/y/w/h
          style={
            popupRect
              ? {
                  left: popupRect.x,
                  top: popupRect.y,
                  width: popupRect.w,
                  height: popupRect.h,
                }
              : undefined
          }
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
          className={`absolute inset-0 outline-none ${cursorClass}`}
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
        {ctx &&
          renderContextMenu?.({
            x: ctx.x,
            y: ctx.y,
            nav,
            send,
            close: () => setCtx(null),
          })}
      </div>
    </div>
  );
}
