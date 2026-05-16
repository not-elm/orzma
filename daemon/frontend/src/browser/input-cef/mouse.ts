// DOM mouse → InputEvent::Mouse* bridge for the cef-backed BrowserActivity.
//
// Coordinates are translated from CSS-pixel viewport space to the device-pixel
// space cef_host expects (via the supplied `dpr()` callback). Click count is
// tracked across a 250 ms double-click window so the wire `count` field
// reflects single/double/triple clicks the way `CefBrowserHost::SendMouseClickEvent`
// wants.

import type { InputEvent } from '../protocol/input';

const MOD_ALT = 1 << 3;
const MOD_CTRL = 1 << 2;
const MOD_SHIFT = 1 << 1;
const MOD_META = 1 << 7;

function modifiers(ev: MouseEvent): number {
  return (
    (ev.altKey ? MOD_ALT : 0) |
    (ev.ctrlKey ? MOD_CTRL : 0) |
    (ev.shiftKey ? MOD_SHIFT : 0) |
    (ev.metaKey ? MOD_META : 0)
  );
}

function buttonName(b: number): 'left' | 'middle' | 'right' {
  if (b === 1) return 'middle';
  if (b === 2) return 'right';
  return 'left';
}

export interface MouseAttachOpts {
  /** Sink for every translated InputEvent. */
  send: (ev: InputEvent) => void;
  /** Element whose bounding box defines the viewport (clientX/Y origin). */
  element: HTMLElement;
  /** Returns the current device pixel ratio. */
  dpr: () => number;
}

/** Attaches mouse listeners and returns a detach closure. */
export function attachMouse({ send, element, dpr }: MouseAttachOpts): () => void {
  let lastButton = -1;
  let lastTime = 0;
  let lastCount = 1;

  const local = (e: MouseEvent): { x: number; y: number } => {
    const r = element.getBoundingClientRect();
    const scale = dpr();
    return {
      x: Math.max(0, Math.round((e.clientX - r.left) * scale)),
      y: Math.max(0, Math.round((e.clientY - r.top) * scale)),
    };
  };

  const onMove = (e: MouseEvent) => {
    const { x, y } = local(e);
    send({ kind: 'mouse_move', x, y, modifiers: modifiers(e) });
  };

  const onDown = (e: MouseEvent) => {
    const { x, y } = local(e);
    const now = performance.now();
    if (e.button === lastButton && now - lastTime < 250) {
      lastCount = Math.min(3, lastCount + 1);
    } else {
      lastCount = 1;
    }
    lastButton = e.button;
    lastTime = now;
    send({
      kind: 'mouse_click',
      x,
      y,
      button: buttonName(e.button),
      count: lastCount,
      mouse_up: false,
      modifiers: modifiers(e),
    });
  };

  const onUp = (e: MouseEvent) => {
    const { x, y } = local(e);
    send({
      kind: 'mouse_click',
      x,
      y,
      button: buttonName(e.button),
      count: lastCount,
      mouse_up: true,
      modifiers: modifiers(e),
    });
  };

  element.addEventListener('mousemove', onMove);
  element.addEventListener('mousedown', onDown);
  element.addEventListener('mouseup', onUp);
  return () => {
    element.removeEventListener('mousemove', onMove);
    element.removeEventListener('mousedown', onDown);
    element.removeEventListener('mouseup', onUp);
  };
}
