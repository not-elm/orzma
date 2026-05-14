//! VT canvas terminal hook — wires the VT WebSocket to the renderer grid + canvas.

import { useEffect, useRef } from 'react';
import { decodeFrame } from './protocol/frame';
import { createCanvasRenderer } from './renderer/canvas';
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
}

const DEFAULT_FONT = '14px "JetBrains Mono", monospace';

/** Subscribes to the VT socket, decodes frames into the grid, and schedules canvas redraws. */
export function useCanvasTerminal(
  windowId: string,
  paneId: string,
  activityId: string | null,
): CanvasTerminal {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gridRef = useRef(createGrid({ cols: 80, rows: 24 }));
  const rendererRef = useRef<ReturnType<typeof createCanvasRenderer> | null>(null);

  const socket = useTerminalSocket(windowId, paneId, activityId, { mode: 'vt' });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const fm = measureFont(canvas, DEFAULT_FONT);
    rendererRef.current = createCanvasRenderer(canvas, fm);

    let rafId: number | null = null;
    const scheduleRedraw = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        const r = rendererRef.current;
        if (r) r.paint(gridRef.current);
      });
    };

    let lastCols = 0;
    let lastRows = 0;
    const fitToContainer = () => {
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
        applyFrame(gridRef.current, frame);
        scheduleRedraw();
      } catch (e) {
        socket.reportDecodeError(String(e));
      }
    });

    socket.setControlHandler((text) => {
      try {
        const msg = JSON.parse(text) as { kind?: string };
        if (msg.kind === 'mode') {
          const mode = msg as { added?: string[]; removed?: string[] };
          for (const m of mode.added ?? []) gridRef.current.modes.add(m);
          for (const m of mode.removed ?? []) gridRef.current.modes.delete(m);
        }
        // NOTE: hello/error/clipboard text frames are ignored in Phase 2B.
      } catch (e) {
        socket.reportDecodeError(String(e));
      }
    });

    return () => {
      ro.disconnect();
      socket.setFrameHandler(null);
      socket.setControlHandler(null);
      if (rafId !== null) cancelAnimationFrame(rafId);
    };
  }, [socket]);

  return {
    canvasRef,
    textareaRef,
    status: socket.status,
    focus: () => textareaRef.current?.focus(),
    blur: () => textareaRef.current?.blur(),
    socket,
  };
}
