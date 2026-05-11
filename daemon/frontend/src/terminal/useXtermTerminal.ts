import '@xterm/xterm/css/xterm.css';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal as XTerm } from '@xterm/xterm';
import { type RefObject, useEffect, useRef } from 'react';
import type { TerminalSocket } from './useTerminalSocket';

const encoder = new TextEncoder();

export function useXtermTerminal(
  containerRef: RefObject<HTMLDivElement | null>,
  socket: TerminalSocket,
) {
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  // Effect: mount xterm exactly once via a ref guard.
  //
  // We deliberately do NOT dispose in cleanup. xterm.js (5.x) schedules an
  // internal setTimeout from Viewport's constructor inside `term.open()`; if
  // the term is disposed before that fires (which happens under React 19
  // StrictMode's mount-cleanup-mount cycle), the deferred callback throws
  // `Cannot read properties of undefined (reading 'dimensions')`.
  //
  // The ref guard ensures only one xterm instance is ever created per
  // component instance. Terminal is the application root so it never
  // unmounts in practice; the term lives for the page lifetime.
  // See https://github.com/xtermjs/xterm.js/issues/4757
  useEffect(() => {
    if (termRef.current) return;
    const container = containerRef.current;
    if (!container) return;

    const term = new XTerm();
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;
  }, [containerRef]);

  // Effect: observe the container for size changes and re-fit xterm.
  //
  // Lives in a separate effect from the mount above so it can be re-attached
  // safely after StrictMode's mount-cleanup-mount cycle. The mount effect
  // short-circuits on re-run (ref guard), which would otherwise leave the
  // container without an observer and prevent xterm from shrinking when its
  // pane is resized (e.g. after a split).
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let resizeTimer: number | null = null;
    const observer = new ResizeObserver(() => {
      const fit = fitRef.current;
      const term = termRef.current;
      if (!fit || !term) return;
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        fit.fit();
        socket.sendControl({
          type: 'resize',
          cols: term.cols,
          rows: term.rows,
        });
      }, 150);
    });
    observer.observe(container);

    return () => {
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      observer.disconnect();
    };
  }, [containerRef, socket.sendControl]);

  // Effect 2: bridge xterm <-> socket whenever the socket transitions
  // through "connected". On disconnect this effect cleans up the listeners
  // but the xterm instance and its on-screen content are preserved.
  // socket.setBinaryHandler buffers any frames that arrived before this
  // effect ran (e.g. the initial scrollback snapshot) and drains them
  // synchronously when the handler is registered.
  // note: snapshot リプレイ時に xterm が capability-query の応答を生成し、
  // それが PTY stdin に逆流する既知の問題がある (DA1/DECRQM/OSC 11)。
  // 根本対策は server 側 VT エミュレータ導入 (docs/plans/server-side-vt-emulator.md)。
  useEffect(() => {
    const term = termRef.current;
    if (!term || socket.status !== 'connected') return;

    const dataDisp = term.onData((data) => {
      socket.sendBinary(encoder.encode(data));
    });
    const binDisp = term.onBinary((data) => {
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i) & 0xff;
      socket.sendBinary(bytes);
    });

    socket.setBinaryHandler((bytes) => term.write(bytes));
    socket.sendControl({ type: 'resize', cols: term.cols, rows: term.rows });

    return () => {
      dataDisp.dispose();
      binDisp.dispose();
      socket.setBinaryHandler(null);
    };
  }, [socket.status, socket.sendBinary, socket.sendControl, socket.setBinaryHandler]);

  const focus = () => termRef.current?.focus();
  const blur = () => termRef.current?.blur();
  return { focus, blur };
}
