/** Per-axis scroll geometry the key handler reads (plain numbers; no DOM). */
export interface ScrollMetrics {
  scrollTop: number;
  scrollLeft: number;
  clientHeight: number;
}

/** Mutable state threaded across keystrokes to detect the `gg` sequence. */
export interface KeyState {
  lastGAt: number;
}

/** The scroll position to apply; absent axis means "leave unchanged". */
export interface ScrollTarget {
  top?: number;
  left?: number;
}

/** Minimal keyboard-event shape the handler needs. */
export interface KeyLike {
  key: string;
  ctrlKey: boolean;
}

/** Pixels scrolled per j/k line step. */
export const LINE_STEP = 40;
/** Pixels scrolled per h/l column step. */
export const COL_STEP = 40;
const GG_WINDOW_MS = 400;

/**
 * Maps a keydown to a scroll target, or `null` when the key is unhandled. Pure:
 * the caller supplies geometry, `now`, and `maxTop` (= scrollHeight - clientHeight),
 * and owns applying the result. Mutates `state.lastGAt` to track `gg`.
 */
export function handleKey(
  e: KeyLike,
  m: ScrollMetrics,
  state: KeyState,
  now: number,
  maxTop: number,
): ScrollTarget | null {
  if (e.ctrlKey && (e.key === 'd' || e.key === 'u')) {
    state.lastGAt = 0;
    const half = Math.floor(m.clientHeight / 2);
    const top = e.key === 'd' ? m.scrollTop + half : m.scrollTop - half;
    return { top: clamp(top, 0, maxTop) };
  }

  switch (e.key) {
    case 'j':
      state.lastGAt = 0;
      return { top: clamp(m.scrollTop + LINE_STEP, 0, maxTop) };
    case 'k':
      state.lastGAt = 0;
      return { top: clamp(m.scrollTop - LINE_STEP, 0, maxTop) };
    case 'h':
      state.lastGAt = 0;
      return { left: m.scrollLeft - COL_STEP };
    case 'l':
      state.lastGAt = 0;
      return { left: m.scrollLeft + COL_STEP };
    case 'G':
      state.lastGAt = 0;
      return { top: maxTop };
    case 'g':
      if (now - state.lastGAt < GG_WINDOW_MS) {
        state.lastGAt = 0;
        return { top: 0 };
      }
      state.lastGAt = now;
      return null;
    default:
      state.lastGAt = 0;
      return null;
  }
}

function clamp(value: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, value));
}
