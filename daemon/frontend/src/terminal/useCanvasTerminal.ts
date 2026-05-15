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
import { createOverlayStore, type OverlayStore } from './overlay-store';
import { decodeFrame } from './protocol/frame';
import { cellHeightOf, cellWidthOf, type FontMetrics } from './renderer/font';
import { applyFrame, createGrid, snapshotGrid } from './renderer/grid';
import { createGridStore, type GridStore } from './renderer/grid-store';
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
  gridStore: GridStore;
  overlayStore: OverlayStore;
}

const DEFAULT_FM: FontMetrics = {
  cellW: 8,
  cellH: 16,
  baseline: 12,
  fontCss: '14px monospace',
  dpr: 1,
};

function mapsEqual<K, V>(a: ReadonlyMap<K, V>, b: ReadonlyMap<K, V>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const [k, v] of a) {
    if (b.get(k) !== v) return false;
  }
  return true;
}

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
  const hyperlinksRef = useRef(hyperlinks);
  hyperlinksRef.current = hyperlinks;
  const [fm, setFm] = useState<FontMetrics>(DEFAULT_FM);
  const modesRef = useRef<ReadonlySet<string>>(gridRef.current.modes);
  const compositionState = useRef<CompositionState>({
    isSendingComposition: false,
    startValue: 0,
    pendingTimer: null,
  });

  const gridStoreRef = useRef<GridStore | null>(null);
  if (!gridStoreRef.current) gridStoreRef.current = createGridStore();
  const gridStore = gridStoreRef.current;

  const overlayStoreRef = useRef<OverlayStore | null>(null);
  if (!overlayStoreRef.current) overlayStoreRef.current = createOverlayStore();
  const overlayStore = overlayStoreRef.current;

  const socket = useTerminalSocket(windowId, paneId, activityId);

  // biome-ignore lint/correctness/useExhaustiveDependencies: socket is ref-stable; rebind on connection keys
  useEffect(() => {
    const pane = paneRef.current;
    if (!pane) return;
    injectTerminalPalette();

    // DOM probe: measure cell size in the .terminal-grid font environment.
    // Probes carry `font-mono leading-none` (see font.ts) so the measurements
    // match what Row.tsx + TerminalGrid.tsx will actually render.
    const cellW = cellWidthOf(pane);
    const cellH = cellHeightOf(pane) || DEFAULT_FM.cellH;
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

    // H1: RAF-batch incoming frames. Vim page-down / scroll bursts produce
    // several frames in the same animation frame; xterm.js and wterm both
    // coalesce these into a single render to avoid flicker. We buffer the
    // raw bytes here and flush all of them inside one requestAnimationFrame.
    const pendingFrames: Uint8Array[] = [];
    let rafScheduled = false;
    let latestHyperlinks: ReadonlyMap<number, string> = hyperlinksRef.current;

    const flushFrames = (): void => {
      rafScheduled = false;
      if (disposed) return;
      if (pendingFrames.length === 0) return;
      const wasAlt = gridRef.current.modes.has('alt-screen');
      const batch = pendingFrames.splice(0);
      let hyperlinksDirty = false;
      let nextHyperlinks = latestHyperlinks;
      let nextScrollOffset = gridStore.getScrollSnapshot().displayOffset;
      let nextHistorySize = gridStore.getScrollSnapshot().historySize;
      for (const bytes of batch) {
        let frame: ReturnType<typeof decodeFrame>;
        try {
          frame = decodeFrame(bytes);
        } catch (e) {
          socket.reportDecodeError(String(e));
          continue;
        }
        applyFrame(gridRef.current, frame);
        if (frame.kind === 'snapshot') {
          nextHyperlinks = new Map(frame.hyperlinks.map((h) => [h.id, h.uri]));
          hyperlinksDirty = true;
          nextScrollOffset = frame.display_offset ?? 0;
          nextHistorySize = frame.history_size ?? 0;
        } else {
          if (frame.hyperlinks.length > 0) {
            const merged = new Map(nextHyperlinks);
            for (const h of frame.hyperlinks) merged.set(h.id, h.uri);
            nextHyperlinks = merged;
            hyperlinksDirty = true;
          }
          nextScrollOffset = frame.display_offset ?? nextScrollOffset;
        }
      }
      modesRef.current = gridRef.current.modes;
      // H2': only call setState when the content actually changed —
      // otherwise the new Map reference would invalidate React.memo on every
      // <Row> even though the cells were untouched.
      if (hyperlinksDirty && !mapsEqual(latestHyperlinks, nextHyperlinks)) {
        latestHyperlinks = nextHyperlinks;
        setHyperlinks(nextHyperlinks);
      }
      gridStore.setGrid(snapshotGrid(gridRef.current));
      gridStore.setScrollState(nextScrollOffset, nextHistorySize);
      overlayStore.setOverlayState({
        cursor: gridRef.current.cursor,
        cols: gridRef.current.cols,
        rows: gridRef.current.rows,
        fm: measuredFm,
      });
      const isAlt = gridRef.current.modes.has('alt-screen');
      if (wasAlt !== isAlt) resetEphemeralState();
    };

    socket.setFrameHandler((bytes) => {
      pendingFrames.push(bytes);
      if (!rafScheduled) {
        rafScheduled = true;
        requestAnimationFrame(flushFrames);
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
        setupMouse(pane, { current: pane }, fmRefLocal, modesRef, sendBytes, textareaRef, socket.sendControl),
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
    gridStore,
    overlayStore,
  };
}
