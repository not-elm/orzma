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
 * and owns applying the result. `gg` is detected by reading `state.lastGAt`; the
 * state is reset up front and only a first lone `g` re-arms it.
 *
 * `state.lastGAt` should start at `Number.NEGATIVE_INFINITY` (no prior `g`) so a
 * single `g` pressed soon after load — when `now` is small — does not satisfy the
 * window and jump to the top.
 */
export function handleKey(
  e: KeyLike,
  m: ScrollMetrics,
  state: KeyState,
  now: number,
  maxTop: number,
): ScrollTarget | null {
  const recentG = now - state.lastGAt < GG_WINDOW_MS;
  state.lastGAt = Number.NEGATIVE_INFINITY;

  if (e.ctrlKey && (e.key === 'd' || e.key === 'u')) {
    const half = Math.floor(m.clientHeight / 2);
    const top = e.key === 'd' ? m.scrollTop + half : m.scrollTop - half;
    return { top: clamp(top, 0, maxTop) };
  }

  switch (e.key) {
    case 'j':
      return { top: clamp(m.scrollTop + LINE_STEP, 0, maxTop) };
    case 'k':
      return { top: clamp(m.scrollTop - LINE_STEP, 0, maxTop) };
    case 'h':
      return { left: m.scrollLeft - COL_STEP };
    case 'l':
      return { left: m.scrollLeft + COL_STEP };
    case 'G':
      return { top: maxTop };
    case 'g':
      if (recentG) return { top: 0 };
      state.lastGAt = now;
      return null;
    default:
      return null;
  }
}

function clamp(value: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, value));
}
