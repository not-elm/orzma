import "@xterm/xterm/css/xterm.css";

import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import { type RefObject, useEffect, useRef } from "react";
import type { TerminalSocket } from "./useTerminalSocket";

const encoder = new TextEncoder();

export function useXtermTerminal(
  containerRef: RefObject<HTMLDivElement | null>,
  socket: TerminalSocket,
) {
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  // Effect 1: mount xterm once. Independent of socket lifecycle so that
  // disconnect/reconnect does NOT destroy the on-screen scrollback.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new XTerm();
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    let resizeTimer: number | null = null;
    const observer = new ResizeObserver(() => {
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        fit.fit();
        socket.sendControl({
          type: "resize",
          cols: term.cols,
          rows: term.rows,
        });
      }, 150);
    });
    observer.observe(container);

    return () => {
      observer.disconnect();
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // socket は意図的に依存に入れない: xterm は mount-once
  }, [containerRef]);

  // Effect 2: bridge xterm <-> socket whenever the socket transitions
  // through "connected". On disconnect this effect cleans up the listeners
  // but the xterm instance and its on-screen content are preserved.
  // socket.setBinaryHandler buffers any frames that arrived before this
  // effect ran (e.g. the initial scrollback snapshot) and drains them
  // synchronously when the handler is registered.
  useEffect(() => {
    const term = termRef.current;
    if (!term || socket.status !== "connected") return;

    const dataDisp = term.onData((data) => {
      socket.sendBinary(encoder.encode(data));
    });
    const binDisp = term.onBinary((data) => {
      const bytes = new Uint8Array(data.length);
      for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i) & 0xff;
      socket.sendBinary(bytes);
    });

    socket.setBinaryHandler((bytes) => term.write(bytes));
    socket.sendControl({ type: "resize", cols: term.cols, rows: term.rows });

    return () => {
      dataDisp.dispose();
      binDisp.dispose();
      socket.setBinaryHandler(null);
    };
    // status の変化のみ effect 再実行のトリガにする (socket オブジェクト自体は
    // 毎レンダー作り直されるため依存に入れてはいけない。setBinaryHandler は
    // useCallback で安定参照)。
  }, [socket.status]);
}
