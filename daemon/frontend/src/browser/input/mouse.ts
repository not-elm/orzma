import type { BrowserClientMsg, MouseButton, MouseKind } from '../protocol/wire';

const BUTTON: Record<number, MouseButton> = { 0: 'left', 1: 'middle', 2: 'right' };

/** CDP modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8 — same as Chrome DevTools Protocol. */
export function modBits(e: {
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}): number {
  return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
}

/**
 * Attach mouse listeners (down / up / move / wheel) to `surface`, scaling
 * client coordinates to the Chromium viewport's pixel dimensions and
 * sending each event over the WS via `send`. Returns a detach function.
 */
export function attachMouse(
  surface: HTMLElement,
  viewport: { width: number; height: number },
  send: (m: BrowserClientMsg) => void,
): () => void {
  const scale = (e: MouseEvent): { x: number; y: number } => {
    const rect = surface.getBoundingClientRect();
    return {
      x: ((e.clientX - rect.left) * viewport.width) / Math.max(rect.width, 1),
      y: ((e.clientY - rect.top) * viewport.height) / Math.max(rect.height, 1),
    };
  };
  const mouseAt = (kind: MouseKind) => (e: MouseEvent) => {
    const { x, y } = scale(e);
    send({
      kind: 'mouse',
      mouse_kind: kind,
      x,
      y,
      button: BUTTON[e.button] ?? 'none',
      modifiers: modBits(e),
    });
  };
  const wheelHandler = (e: WheelEvent) => {
    const { x, y } = scale(e);
    e.preventDefault();
    send({
      kind: 'wheel',
      x,
      y,
      dx: e.deltaX,
      dy: e.deltaY,
      modifiers: modBits(e),
    });
  };
  const onDown = mouseAt('down');
  const onUp = mouseAt('up');
  const onMove = mouseAt('move');
  surface.addEventListener('mousedown', onDown);
  surface.addEventListener('mouseup', onUp);
  surface.addEventListener('mousemove', onMove);
  surface.addEventListener('wheel', wheelHandler, { passive: false });
  return () => {
    surface.removeEventListener('mousedown', onDown);
    surface.removeEventListener('mouseup', onUp);
    surface.removeEventListener('mousemove', onMove);
    surface.removeEventListener('wheel', wheelHandler);
  };
}
