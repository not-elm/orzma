// DOM wheel → InputEvent::MouseWheel bridge for the cef-backed BrowserActivity.
//
// preventDefault on the wheel event so the host browser does not scroll its
// own page; the wheel is consumed entirely by the embedded cef_host. Each
// dispatch records `performance.mark('input-dispatch', { detail: input_id })`
// so the Plan 2 Phase C KPI harness can correlate the matching paint.

import type { InputEvent } from '../protocol/input';

const MOD_ALT = 1 << 3;
const MOD_CTRL = 1 << 2;
const MOD_SHIFT = 1 << 1;
const MOD_META = 1 << 7;

function modifiers(e: WheelEvent): number {
  return (
    (e.altKey ? MOD_ALT : 0) |
    (e.ctrlKey ? MOD_CTRL : 0) |
    (e.shiftKey ? MOD_SHIFT : 0) |
    (e.metaKey ? MOD_META : 0)
  );
}

let nextInputId = 1;

export interface WheelAttachOpts {
  /** Sink for every translated InputEvent. */
  send: (ev: InputEvent) => void;
  /** Element whose bounding box defines the viewport. */
  element: HTMLElement;
  /** Returns the current device pixel ratio. */
  dpr: () => number;
  /** Frame worker to notify of each dispatched input id for KPI correlation. */
  worker?: Worker | null;
}

/** Attaches a wheel listener and returns a detach closure. */
export function attachWheel({ send, element, dpr, worker }: WheelAttachOpts): () => void {
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const id = nextInputId++;
    const t = performance.now();
    performance.mark('input-dispatch', { detail: id });
    (
      window as unknown as { __poc_kpi_dispatches?: Array<{ id: number; t: number }> }
    ).__poc_kpi_dispatches ??= [];
    (
      window as unknown as { __poc_kpi_dispatches: Array<{ id: number; t: number }> }
    ).__poc_kpi_dispatches.push({ id, t });
    worker?.postMessage({ type: 'last_input_id', id });
    const r = element.getBoundingClientRect();
    const scale = dpr();
    send({
      kind: 'mouse_wheel',
      x: Math.round((e.clientX - r.left) * scale),
      y: Math.round((e.clientY - r.top) * scale),
      // NOTE: sign-flip — Chromium's mouse wheel delta is opposite of DOM's
      // WheelEvent.deltaX/Y convention.
      delta_x: -Math.round(e.deltaX),
      delta_y: -Math.round(e.deltaY),
      modifiers: modifiers(e),
    });
  };
  element.addEventListener('wheel', onWheel, { passive: false });
  return () => element.removeEventListener('wheel', onWheel);
}
