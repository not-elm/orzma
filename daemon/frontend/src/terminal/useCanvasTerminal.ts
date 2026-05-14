//! VT terminal hook (DOM renderer): subscribes to the WebSocket, decodes
//! frames into the grid, pushes the grid into grid-store, and wires all
//! Phase 3A input listeners (IME / paste / mouse / focus / keydown) plus
//! Phase 3.5 native clipboard copy.

import { useEffect, useRef, useState } from 'react';
import { type CompositionState, setupComposition } from './input/composition';
import { setupCopy } from './input/copy';
import { encodeInputFrame } from './input/encode-input';
import { setupFocusEvents } from './input/focus';
import { handleKeyDown } from './input/keymap';
import { setupMouse } from './input/mouse';
import { setupPaste } from './input/paste';
import { setOverlayState } from './overlay-store';
import { decodeFrame } from './protocol/frame';
import { cellWidthOf, type FontMetrics } from './renderer/font';
import { applyFrame, createGrid } from './renderer/grid';
import { setGrid } from './renderer/grid-store';
import { injectTerminalPalette } from './renderer/palette';
import type { SocketStatus, TerminalSocket } from './useTerminalSocket';
import { useTerminalSocket } from './useTerminalSocket';

/** Public API of the VT terminal hook (DOM renderer). */
export interface CanvasTerminal {
  paneRef: React.RefObject<HTMLDivElement | null>;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  status: SocketStatus;
  focus: () => void;
  blur: () => void;
  socket: TerminalSocket;
  preedit: string;
  hyperlinks: ReadonlyMap<number, string>;
  fm: FontMetrics;
}

const DEFAULT_FM: FontMetrics = {
  cellW: 8,
  cellH: 16,
  baseline: 12,
  fontCss: '14px monospace',
  dpr: 1,
};

/** Subscribes to the VT socket, decodes frames into the grid, pushes the grid
 *  into grid-store, and wires all Phase 3A input listeners (IME / paste / mouse
 *  / focus / keydown) plus Phase 3.5 native clipboard copy. */
export function useCanvasTerminal(
  windowId: string,
  paneId: string,
  activityId: string | null,
  isActive: boolean,
): CanvasTerminal {
  const paneRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gridRef = useRef(createGrid({ cols: 80, rows: 24 }));
  const isActiveRef = useRef(isActive);
  const [preedit, setPreedit] = useState('');
  const [hyperlinks, setHyperlinks] = useState<ReadonlyMap<number, string>>(new Map());
  const [fm, setFm] = useState<FontMetrics>(DEFAULT_FM);
  const modesRef = useRef<ReadonlySet<string>>(gridRef.current.modes);
  const compositionState = useRef<CompositionState>({
    isSendingComposition: false,
    startValue: 0,
    pendingTimer: null,
  });

  const socket = useTerminalSocket(windowId, paneId, activityId);

  // biome-ignore lint/correctness/useExhaustiveDependencies: socket is ref-stable; rebind on connection keys
  useEffect(() => {
    const pane = paneRef.current;
    if (!pane) return;
    injectTerminalPalette();

    // DOM probe: measure cell size from the actual rendered font.
    const cellW = cellWidthOf(pane);
    const heightProbe = document.createElement('span');
    heightProbe.style.visibility = 'hidden';
    heightProbe.style.position = 'absolute';
    heightProbe.className = 'font-mono';
    heightProbe.textContent = 'W';
    pane.appendChild(heightProbe);
    const cellH = heightProbe.getBoundingClientRect().height || DEFAULT_FM.cellH;
    pane.removeChild(heightProbe);
    const measuredFm: FontMetrics = {
      cellW,
      cellH,
      baseline: Math.round(cellH * 0.8),
      fontCss: getComputedStyle(pane).font,
      dpr: typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1,
    };
    setFm(measuredFm);

    let disposed = false;
    const sendBytes = (b: Uint8Array): void => {
      if (disposed) return;
      socket.sendBinary(encodeInputFrame(b));
    };

    modesRef.current = gridRef.current.modes;

    function resetEphemeralState(): void {
      setPreedit('');
      document.getSelection()?.removeAllRanges();
    }

    let lastCols = 0;
    let lastRows = 0;
    const fitToContainer = (): void => {
      const cssW = pane.clientWidth;
      const cssH = pane.clientHeight;
      if (cssW === 0 || cssH === 0) return;
      const cols = Math.max(1, Math.floor(cssW / measuredFm.cellW));
      const rows = Math.max(1, Math.floor(cssH / measuredFm.cellH));
      if (cols !== lastCols || rows !== lastRows) {
        lastCols = cols;
        lastRows = rows;
        socket.sendControl({ kind: 'resize', cols, rows });
      }
    };

    const ro = new ResizeObserver(() => fitToContainer());
    ro.observe(pane);
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
        setGrid({ ...gridRef.current });
        setOverlayState({
          cursor: gridRef.current.cursor,
          cols: gridRef.current.cols,
          rows: gridRef.current.rows,
          fm: measuredFm,
        });
        const isAlt = gridRef.current.modes.has('alt-screen');
        if (wasAlt !== isAlt) resetEphemeralState();
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
      const fmRefLocal = { current: measuredFm };
      const sendText = (text: string): void => sendBytes(new TextEncoder().encode(text));

      cleanups.push(setupComposition(ta, setPreedit, sendText, compositionState.current));
      cleanups.push(setupPaste(ta, modesRef, sendBytes));
      cleanups.push(
        setupMouse(pane, { current: pane }, fmRefLocal, modesRef, sendBytes, textareaRef),
      );
      cleanups.push(setupFocusEvents(ta, modesRef, sendBytes));
      cleanups.push(setupCopy(ta));

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
    };
  }, [socket, windowId, paneId, activityId]);

  useEffect(() => {
    isActiveRef.current = isActive;
  }, [isActive]);

  return {
    paneRef,
    textareaRef,
    status: socket.status,
    focus: () => textareaRef.current?.focus(),
    blur: () => textareaRef.current?.blur(),
    socket,
    preedit,
    hyperlinks,
    fm,
  };
}
