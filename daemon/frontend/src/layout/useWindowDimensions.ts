import { useEffect, useRef } from 'react';
import { resizeWindow } from './resizeWindow';
import type { WindowId } from './types';

/** Options for `useWindowDimensions`. */
export interface UseWindowDimensionsOptions {
  /** From the font-metrics probe. */
  cellWidth: number;
  /** From the font-metrics probe. */
  cellHeight: number;
  /** Debounce window for non-initial measurements (default 50ms). */
  debounceMs?: number;
}

/**
 * Observes `container`'s size and PATCHes the server with cell dimensions.
 * The first measurement is sent immediately (no debounce); subsequent ones
 * are debounced by `debounceMs` (default 50ms). Repeated measurements that
 * resolve to the same (cols, rows) are skipped.
 */
export function useWindowDimensions(
  windowId: WindowId | null,
  container: HTMLElement | null,
  opts: UseWindowDimensionsOptions,
): void {
  const sentInitial = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const last = useRef<{ cols: number; rows: number } | null>(null);

  useEffect(() => {
    if (!windowId) return;
    if (!container) return;
    if (opts.cellWidth <= 0 || opts.cellHeight <= 0) return;
    const measure = () => {
      const cols = Math.max(1, Math.floor(container.clientWidth / opts.cellWidth));
      const rows = Math.max(1, Math.floor(container.clientHeight / opts.cellHeight));
      if (last.current?.cols === cols && last.current?.rows === rows) return;
      last.current = { cols, rows };

      if (!sentInitial.current) {
        sentInitial.current = true;
        void resizeWindow(windowId, cols, rows);
        return;
      }
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        void resizeWindow(windowId, cols, rows);
        timer.current = null;
      }, opts.debounceMs ?? 50);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    measure();
    return () => {
      ro.disconnect();
      if (timer.current !== null) {
        clearTimeout(timer.current);
        timer.current = null;
      }
    };
  }, [windowId, container, opts.cellWidth, opts.cellHeight, opts.debounceMs]);
}
