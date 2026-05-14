//! VT canvas terminal hook — wires the VT WebSocket to the renderer grid + canvas + input modules.

import { useEffect, useRef, useState } from 'react';
import { type CompositionState, setupComposition } from './input/composition';
import { encodeInputFrame } from './input/encode-input';
import { setupFocusEvents } from './input/focus';
import { handleKeyDown } from './input/keymap';
import { setupMouse } from './input/mouse';
import { setupPaste } from './input/paste';
import {
  type LinkHover,
  type SelectionRange,
  setupPointerOverlays,
} from './input/pointer-overlays';
import { setOverlayState } from './overlay-store';
import { decodeFrame } from './protocol/frame';
import { createCanvasRenderer, DEFAULT_BG } from './renderer/canvas';
import { measureFont } from './renderer/font';
import { applyFrame, createGrid } from './renderer/grid';
import type { SocketStatus, TerminalSocket } from './useTerminalSocket';
import { useTerminalSocket } from './useTerminalSocket';

/** Public API of the VT canvas terminal hook. */
export interface CanvasTerminal {
  canvasRef: React.RefObject<HTMLCanvasElement | null>;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  status: SocketStatus;
  focus: () => void;
  blur: () => void;
  socket: TerminalSocket;
  preedit: string;
  selection: SelectionRange | null;
  linkHover: LinkHover | null;
  hyperlinks: ReadonlyMap<number, string>;
}

const DEFAULT_FONT = '14px "JetBrains Mono", monospace';

/** Subscribes to the VT socket, decodes frames into the grid, schedules canvas
 *  redraws, and wires all Phase 3A input listeners (IME / paste / mouse / focus
 *  / keydown). */
export function useCanvasTerminal(
  windowId: string,
  paneId: string,
  activityId: string | null,
  isActive: boolean,
): CanvasTerminal {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gridRef = useRef(createGrid({ cols: 80, rows: 24 }));
  const rendererRef = useRef<ReturnType<typeof createCanvasRenderer> | null>(null);
  const isActiveRef = useRef(isActive);
  const scheduleRedrawRef = useRef<(() => void) | null>(null);
  const [preedit, setPreedit] = useState('');
  const [selection, setSelection] = useState<SelectionRange | null>(null);
  const [linkHover, setLinkHover] = useState<LinkHover | null>(null);
  const [hyperlinks, setHyperlinks] = useState<ReadonlyMap<number, string>>(new Map());
  const hyperlinksRef = useRef<ReadonlyMap<number, string>>(hyperlinks);
  hyperlinksRef.current = hyperlinks;
  const modesRef = useRef<ReadonlySet<string>>(gridRef.current.modes);
  const compositionState = useRef<CompositionState>({
    isSendingComposition: false,
    startValue: 0,
    pendingTimer: null,
  });

  const socket = useTerminalSocket(windowId, paneId, activityId, { mode: 'vt' });

  // biome-ignore lint/correctness/useExhaustiveDependencies: windowId/paneId/activityId are connection keys — the socket object is ref-stable across reconnects, so we must re-run this effect to re-attach handlers on the new WS when any of them changes.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const fm = measureFont(canvas, DEFAULT_FONT);
    rendererRef.current = createCanvasRenderer(canvas, fm);

    // disposed flag bound to this effect run — gates any deferred (setTimeout
    // / RAF) callback so it cannot send to a torn-down socket after pane switch.
    let disposed = false;
    const sendBytes = (b: Uint8Array): void => {
      if (disposed) return;
      socket.sendBinary(encodeInputFrame(b));
    };

    // grid.modes is mutated in place by applySnapshot (renderer/grid.ts),
    // so modesRef tracks the Set identity once. Reassign defensively in the
    // frame handler in case a future regression reintroduces Set replacement.
    modesRef.current = gridRef.current.modes;

    function resetEphemeralState(): void {
      setPreedit('');
      setSelection(null);
      setLinkHover(null);
    }

    let rafId: number | null = null;
    const scheduleRedraw = (): void => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        const r = rendererRef.current;
        if (r) r.paint(gridRef.current, { isActive: isActiveRef.current });
      });
    };
    scheduleRedrawRef.current = scheduleRedraw;

    let lastCols = 0;
    let lastRows = 0;
    const fitToContainer = (): void => {
      const container = canvas.parentElement;
      if (!container) return;
      const cssW = container.clientWidth;
      const cssH = container.clientHeight;
      if (cssW === 0 || cssH === 0) return;
      const cols = Math.max(1, Math.floor(cssW / fm.cellW));
      const rows = Math.max(1, Math.floor(cssH / fm.cellH));
      const dpr = window.devicePixelRatio || 1;
      const desiredW = Math.round(cols * fm.cellW * dpr);
      const desiredH = Math.round(rows * fm.cellH * dpr);
      if (canvas.width !== desiredW) canvas.width = desiredW;
      if (canvas.height !== desiredH) canvas.height = desiredH;
      canvas.style.width = `${cols * fm.cellW}px`;
      canvas.style.height = `${rows * fm.cellH}px`;
      const ctx = canvas.getContext('2d');
      if (ctx) {
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx.font = fm.fontCss;
        ctx.textBaseline = 'alphabetic';
        ctx.fillStyle = DEFAULT_BG;
        ctx.fillRect(0, 0, cols * fm.cellW, rows * fm.cellH);
      }
      if (cols !== lastCols || rows !== lastRows) {
        lastCols = cols;
        lastRows = rows;
        socket.sendControl({ kind: 'resize', cols, rows });
        for (let r = 0; r < gridRef.current.rows; r++) gridRef.current.dirtyRows.add(r);
        scheduleRedraw();
      }
    };

    const ro = new ResizeObserver(() => fitToContainer());
    if (canvas.parentElement) ro.observe(canvas.parentElement);
    fitToContainer();

    socket.setFrameHandler((bytes) => {
      try {
        const frame = decodeFrame(bytes);
        const wasAlt = gridRef.current.modes.has('alt-screen');
        applyFrame(gridRef.current, frame);
        modesRef.current = gridRef.current.modes;
        if (frame.kind === 'snapshot') {
          setHyperlinks(new Map(frame.hyperlinks.map((h) => [h.id, h.uri])));
        } else if (frame.hyperlinks.length > 0) {
          setHyperlinks((prev) => {
            const next = new Map(prev);
            for (const h of frame.hyperlinks) next.set(h.id, h.uri);
            return next;
          });
        }
        setOverlayState({
          cursor: gridRef.current.cursor,
          cols: gridRef.current.cols,
          rows: gridRef.current.rows,
          fm,
        });
        const isAlt = gridRef.current.modes.has('alt-screen');
        if (wasAlt !== isAlt) resetEphemeralState();
        scheduleRedraw();
      } catch (e) {
        socket.reportDecodeError(String(e));
      }
    });

    socket.setControlHandler((text) => {
      try {
        const msg = JSON.parse(text) as { kind?: string; added?: string[]; removed?: string[] };
        if (msg.kind === 'mode') {
          const altToggled =
            msg.added?.includes('alt-screen') === true ||
            msg.removed?.includes('alt-screen') === true;
          for (const m of msg.added ?? []) gridRef.current.modes.add(m);
          for (const m of msg.removed ?? []) gridRef.current.modes.delete(m);
          if (altToggled) resetEphemeralState();
        }
      } catch (e) {
        socket.reportDecodeError(String(e));
      }
    });

    const cleanups: Array<() => void> = [];
    const ta = textareaRef.current;
    if (ta) {
      const localFmRef = { current: fm };
      // String-typed composition submit → bytes via TextEncoder.
      const sendText = (text: string): void => sendBytes(new TextEncoder().encode(text));

      cleanups.push(setupComposition(ta, setPreedit, sendText, compositionState.current));
      cleanups.push(setupPaste(ta, modesRef, sendBytes));
      cleanups.push(setupMouse(ta, canvas, localFmRef, modesRef, sendBytes));
      cleanups.push(setupFocusEvents(ta, modesRef, sendBytes));
      cleanups.push(
        setupPointerOverlays(
          ta,
          canvas,
          localFmRef,
          modesRef,
          gridRef,
          hyperlinksRef,
          setSelection,
          setLinkHover,
        ),
      );

      const onKey = (e: KeyboardEvent): void => {
        if (compositionState.current.isSendingComposition) return;
        const bytes = handleKeyDown(e, gridRef.current.modes);
        if (!bytes) return;
        e.preventDefault();
        sendBytes(bytes);
      };
      ta.addEventListener('keydown', onKey);
      cleanups.push(() => ta.removeEventListener('keydown', onKey));
    }

    return () => {
      disposed = true;
      for (const c of cleanups) c();
      ro.disconnect();
      socket.setFrameHandler(null);
      socket.setControlHandler(null);
      scheduleRedrawRef.current = null;
      if (rafId !== null) cancelAnimationFrame(rafId);
    };
  }, [socket, windowId, paneId, activityId]);

  useEffect(() => {
    isActiveRef.current = isActive;
    const grid = gridRef.current;
    if (grid.cursor.visible) grid.dirtyRows.add(grid.cursor.y);
    scheduleRedrawRef.current?.();
  }, [isActive]);

  return {
    canvasRef,
    textareaRef,
    status: socket.status,
    focus: () => textareaRef.current?.focus(),
    blur: () => textareaRef.current?.blur(),
    socket,
    preedit,
    selection,
    linkHover,
    hyperlinks,
  };
}
