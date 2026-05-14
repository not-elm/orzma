//! Mouse event encoding (modes 1000/1002/1003/1006) + cell-coord translation
//! + textarea-attached listener with RAF coalescing.

import type { FontMetrics } from '../renderer/font';

/** Button enum matching `PointerEvent.button` (0=left, 1=middle, 2=right). */
export type MouseButton = 0 | 1 | 2;

export interface MouseEventInput {
  kind: 'down' | 'up' | 'move' | 'wheel';
  button: 'left' | 'middle' | 'right' | 'wheelUp' | 'wheelDown' | 'none';
  col: number;
  row: number;
  shift: boolean;
  alt: boolean;
  ctrl: boolean;
  buttonHeld: boolean;
}

const BUTTON_CB: Record<MouseEventInput['button'], number> = {
  left: 0,
  middle: 1,
  right: 2,
  wheelUp: 64,
  wheelDown: 65,
  none: 3,
};

function baseCb(e: MouseEventInput): number {
  let cb = BUTTON_CB[e.button];
  if (e.kind === 'move') cb |= 32;
  if (e.shift) cb += 4;
  if (e.alt) cb += 8;
  if (e.ctrl) cb += 16;
  return cb;
}

function gatingDropsEvent(e: MouseEventInput, modes: ReadonlySet<string>): boolean {
  if (!modes.has('mouse-vt200') && !modes.has('mouse-btn-event') && !modes.has('mouse-any-event')) {
    return true;
  }
  if (e.kind === 'move') {
    if (modes.has('mouse-any-event')) return false;
    if (modes.has('mouse-btn-event') && e.buttonHeld) return false;
    return true;
  }
  return false;
}

const ENC = new TextEncoder();

/** Returns null when gating drops the event or DEFAULT-encoding coords overflow. */
export function encodeMouseEvent(
  e: MouseEventInput,
  modes: ReadonlySet<string>,
): Uint8Array | null {
  if (gatingDropsEvent(e, modes)) return null;

  const col1 = e.col + 1;
  const row1 = e.row + 1;
  const cb = baseCb(e);

  if (modes.has('mouse-sgr-1006')) {
    // SGR release uses 'm' and carries the original button (not the X10
    // "release=3" sentinel). baseCb returns 0/1/2 for left/middle/right so
    // `cb` already encodes the press-button — pass through.
    const suffix = e.kind === 'up' ? 'm' : 'M';
    return ENC.encode(`\x1b[<${cb};${col1};${row1}${suffix}`);
  }

  // DEFAULT encoding: \e[M + (Cb+32) + (col1+32) + (row1+32). Bytes must
  // fit in u8 (≤255). xterm.js's DEFAULT.restrict suppresses overflow.
  const b1 = cb + 32;
  const b2 = col1 + 32;
  const b3 = row1 + 32;
  if (b1 > 255 || b2 > 255 || b3 > 255) return null;

  return new Uint8Array([0x1b, 0x5b, 0x4d, b1, b2, b3]);
}

/** Translates clientX/Y on the canvas to 0-based cell coords. */
export function pointToCell(
  canvas: HTMLCanvasElement,
  ev: { clientX: number; clientY: number },
  fm: FontMetrics,
): { col: number; row: number } {
  const rect = canvas.getBoundingClientRect();
  const col = Math.floor((ev.clientX - rect.left) / fm.cellW);
  const row = Math.floor((ev.clientY - rect.top) / fm.cellH);
  return { col, row };
}

function buttonName(b: number): MouseEventInput['button'] {
  if (b === 0) return 'left';
  if (b === 1) return 'middle';
  if (b === 2) return 'right';
  return 'none';
}

/** Wires pointer + wheel + contextmenu listeners on `target` (the textarea),
 *  translates coordinates relative to `canvas`, and dispatches encoded bytes. */
export function setupMouse(
  target: HTMLElement,
  canvas: HTMLCanvasElement,
  fmRef: { current: FontMetrics },
  modesRef: { current: ReadonlySet<string> },
  send: (bytes: Uint8Array) => void,
): () => void {
  // NOTE: keyed by PointerEvent.button (0/1/2), NOT pointerId. pointerId
  // identifies the device, not the button; a Set of pointerIds cannot
  // represent multiple buttons held on the same mouse.
  const heldButtons = new Set<MouseButton>();

  function anyMode(): boolean {
    const m = modesRef.current;
    return m.has('mouse-vt200') || m.has('mouse-btn-event') || m.has('mouse-any-event');
  }

  function lastButtonName(): MouseEventInput['button'] {
    // Set iteration order is insertion order in JS; the most recently added
    // entry is the last one in iteration. Matches xterm.js "last button down".
    const last = Array.from(heldButtons).at(-1);
    if (last === undefined) return 'none';
    return buttonName(last);
  }

  const onPointerDown = (e: PointerEvent): void => {
    if (e.button < 0 || e.button > 2) return;
    try {
      target.setPointerCapture(e.pointerId);
    } catch {
      // NOTE: setPointerCapture throws NotFoundError if the pointer is no
      // longer active; ignore and continue with bubbled events.
    }
    heldButtons.add(e.button as MouseButton);
    if (anyMode()) e.preventDefault();

    const { col, row } = pointToCell(canvas, e, fmRef.current);
    const bytes = encodeMouseEvent(
      {
        kind: 'down',
        button: buttonName(e.button),
        col,
        row,
        shift: e.shiftKey,
        alt: e.altKey,
        ctrl: e.ctrlKey,
        buttonHeld: true,
      },
      modesRef.current,
    );
    if (bytes) send(bytes);
  };

  let pendingRaf: number | null = null;
  let pendingEvent: PointerEvent | null = null;
  const onPointerMove = (e: PointerEvent): void => {
    pendingEvent = e;
    if (pendingRaf !== null) return;
    pendingRaf = requestAnimationFrame(() => {
      pendingRaf = null;
      const ev = pendingEvent;
      pendingEvent = null;
      if (!ev) return;
      const { col, row } = pointToCell(canvas, ev, fmRef.current);
      const bytes = encodeMouseEvent(
        {
          kind: 'move',
          button: lastButtonName(),
          col,
          row,
          shift: ev.shiftKey,
          alt: ev.altKey,
          ctrl: ev.ctrlKey,
          buttonHeld: heldButtons.size > 0,
        },
        modesRef.current,
      );
      if (bytes) send(bytes);
    });
  };

  const onPointerUp = (e: PointerEvent): void => {
    if (e.button < 0 || e.button > 2) return;
    heldButtons.delete(e.button as MouseButton);
    const { col, row } = pointToCell(canvas, e, fmRef.current);
    const bytes = encodeMouseEvent(
      {
        kind: 'up',
        button: buttonName(e.button),
        col,
        row,
        shift: e.shiftKey,
        alt: e.altKey,
        ctrl: e.ctrlKey,
        buttonHeld: heldButtons.size > 0,
      },
      modesRef.current,
    );
    if (bytes) send(bytes);
  };

  const onPointerCancel = (): void => {
    heldButtons.clear();
  };
  const onLostCapture = (): void => {
    heldButtons.clear();
  };

  const onWheel = (e: WheelEvent): void => {
    if (!anyMode()) return;
    e.preventDefault();
    const { col, row } = pointToCell(canvas, e, fmRef.current);
    const button: MouseEventInput['button'] = e.deltaY < 0 ? 'wheelUp' : 'wheelDown';
    const bytes = encodeMouseEvent(
      {
        kind: 'wheel',
        button,
        col,
        row,
        shift: e.shiftKey,
        alt: e.altKey,
        ctrl: e.ctrlKey,
        buttonHeld: heldButtons.size > 0,
      },
      modesRef.current,
    );
    if (bytes) send(bytes);
  };

  const onContextMenu = (e: Event): void => {
    if (anyMode()) e.preventDefault();
  };

  target.addEventListener('pointerdown', onPointerDown);
  target.addEventListener('pointermove', onPointerMove);
  target.addEventListener('pointerup', onPointerUp);
  target.addEventListener('pointercancel', onPointerCancel);
  target.addEventListener('lostpointercapture', onLostCapture);
  target.addEventListener('wheel', onWheel, { passive: false });
  target.addEventListener('contextmenu', onContextMenu);

  return () => {
    if (pendingRaf !== null) {
      cancelAnimationFrame(pendingRaf);
      pendingRaf = null;
    }
    heldButtons.clear();
    target.removeEventListener('pointerdown', onPointerDown);
    target.removeEventListener('pointermove', onPointerMove);
    target.removeEventListener('pointerup', onPointerUp);
    target.removeEventListener('pointercancel', onPointerCancel);
    target.removeEventListener('lostpointercapture', onLostCapture);
    target.removeEventListener('wheel', onWheel);
    target.removeEventListener('contextmenu', onContextMenu);
  };
}
