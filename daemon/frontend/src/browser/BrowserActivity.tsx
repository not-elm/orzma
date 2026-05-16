import { useEffect, useRef, useState } from 'react';
import { ContextMenu } from './ContextMenu';
import { attachComposition } from './input/composition';
import { attachKeyboard } from './input/keyboard';
import { attachMouse } from './input/mouse';
import { CanvasFrame } from './renderer/CanvasFrame';
import { Toolbar } from './Toolbar';
import { useBrowserSocket } from './useBrowserSocket';

interface Props {
  windowId: string;
  paneId: string;
  activityId: string;
  isActive: boolean;
}

/**
 * Top-level component for a Browser Activity. Renders a toolbar above a
 * stacked layout of canvas (screencast) + transparent overlay (mouse) +
 * hidden textarea (keyboard / IME). A right-click on the overlay shows a
 * frontend-drawn `ContextMenu`.
 */
export function BrowserActivity({ windowId, paneId, activityId, isActive }: Props) {
  const { send, lastFrame, nav, viewport } = useBrowserSocket(windowId, paneId, activityId);
  const overlayRef = useRef<HTMLDivElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const [ctx, setCtx] = useState<{ x: number; y: number } | null>(null);

  // Send a DPR-aware resize whenever the overlay's CSS pixel size changes.
  // The Chromium viewport and screencast bounds are updated to match.
  // Resize messages sent before the WS is open are buffered in useBrowserSocket
  // and flushed on connect, so ResizeObserver can fire at any time.
  useEffect(() => {
    const el = overlayRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      const width = Math.max(1, Math.round(r.width));
      const height = Math.max(1, Math.round(r.height));
      send({
        kind: 'resize',
        width,
        height,
        device_scale_factor: window.devicePixelRatio,
      });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [send]);

  // Mouse / wheel: scale coords against the CSS viewport dimensions reported
  // by the daemon, not the JPEG bitmap size (which may be viewport × DSF on
  // HiDPI displays). Fall back to 1 until the first Viewport message arrives.
  useEffect(() => {
    if (!overlayRef.current) return;
    return attachMouse(
      overlayRef.current,
      { width: Math.max(1, viewport.width), height: Math.max(1, viewport.height) },
      send,
    );
  }, [send, viewport.width, viewport.height]);

  // Keyboard + IME on the hidden textarea.
  useEffect(() => {
    const ta = taRef.current;
    if (!ta) return;
    const detachKey = attachKeyboard(ta, send);
    const detachIme = attachComposition(ta, send);
    return () => {
      detachKey();
      detachIme();
    };
  }, [send]);

  // Focus the textarea when the pane becomes active.
  useEffect(() => {
    if (isActive) taRef.current?.focus();
  }, [isActive]);

  return (
    <div className="flex h-full w-full flex-col">
      <Toolbar url={nav.url} send={send} />
      <div
        ref={overlayRef}
        // NOTE: role="application" marks this as an interactive widget zone;
        // required because onContextMenu would otherwise violate the
        // a11y/noStaticElementInteractions lint rule.
        role="application"
        aria-label="Browser viewport"
        className="relative flex-1 overflow-hidden bg-background"
        onContextMenu={(e) => {
          e.preventDefault();
          setCtx({ x: e.clientX, y: e.clientY });
        }}
      >
        <CanvasFrame
          jpeg={lastFrame?.jpeg ?? null}
          width={lastFrame?.width ?? 0}
          height={lastFrame?.height ?? 0}
          className="absolute inset-0 h-full w-full"
        />
        <textarea
          ref={taRef}
          className="absolute inset-0 resize-none border-0 bg-transparent text-transparent caret-transparent outline-none pointer-events-none"
        />
        {ctx && <ContextMenu x={ctx.x} y={ctx.y} send={send} onClose={() => setCtx(null)} />}
      </div>
    </div>
  );
}
