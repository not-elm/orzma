import { useEffect, useState } from 'react';
import { createKeyDispatcher, type KeyDispatcher } from './globalKeyDispatcher';

/**
 * **INVARIANT: at most one `usePrefixMode` may be mounted at a time.**
 *
 * The hook owns module-level `shared` state and a singleton dispatcher
 * attached to `document`. A second concurrent caller would clobber
 * `shared` (silently breaking the first caller's `setArmed` callback)
 * and unmounting either would `detachFrom(document)`, killing keyboard
 * handling for the surviving caller. The intended owner is `App.tsx`.
 */
export type PrefixBindings = ReadonlyMap<string, () => void>;

export interface PrefixModeOptions {
  prefix?: { ctrl: boolean; key: string };
  timeoutMs?: number;
}

export interface PrefixModeState {
  isArmed: boolean;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta']);

interface SharedState {
  bindings: PrefixBindings;
  prefixCtrl: boolean;
  prefixKeyLower: string;
  timeoutMs: number;
  armed: boolean;
  setArmed: (next: boolean) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

let shared: SharedState | null = null;
let dispatcher: KeyDispatcher | null = null;

function ensureDispatcher() {
  if (dispatcher) return dispatcher;
  const handler = (e: KeyboardEvent) => {
    if (!shared) return;
    if (e.isComposing) return;
    const key = e.key.toLowerCase();
    const { prefixCtrl, prefixKeyLower } = shared;

    if (!shared.armed) {
      const isPrefix =
        key === prefixKeyLower &&
        e.ctrlKey === prefixCtrl &&
        !e.shiftKey &&
        !e.altKey &&
        !e.metaKey &&
        !e.repeat;
      if (isPrefix) {
        e.preventDefault();
        e.stopPropagation();
        shared.setArmed(true);
      }
      return;
    }

    // Armed mode consumes every event so it cannot leak to xterm.js or iframes.
    e.preventDefault();
    e.stopPropagation();

    if (MODIFIER_KEYS.has(e.key) || e.repeat) return;

    if (e.key === 'Escape' || (key === prefixKeyLower && e.ctrlKey === prefixCtrl)) {
      shared.setArmed(false);
      return;
    }

    const action = shared.bindings.get(key);
    if (action) action();
    shared.setArmed(false);
  };
  dispatcher = createKeyDispatcher(handler);
  return dispatcher;
}

/** Attach an additional EventTarget (e.g. an iframe contentDocument) to the prefix dispatcher. */
export function attachKeydownTarget(target: EventTarget) {
  ensureDispatcher().attachTo(target);
}

/** Detach a previously attached target. */
export function detachKeydownTarget(target: EventTarget) {
  if (!dispatcher) return;
  dispatcher.detachFrom(target);
}

export function usePrefixMode(
  bindings: PrefixBindings,
  options?: PrefixModeOptions,
): PrefixModeState {
  const [isArmed, setIsArmed] = useState(false);

  // Use primitive deps so a default object literal does not cause re-subscribe
  // on every render.
  const prefixCtrl = options?.prefix?.ctrl ?? true;
  const prefixKey = options?.prefix?.key ?? 'b';
  const timeoutMs = options?.timeoutMs ?? 2000;

  // biome-ignore lint/correctness/useExhaustiveDependencies: bindings is kept fresh via the render-time write below; adding it would re-run the effect on every identity change and clobber shared state.
  useEffect(() => {
    const setArmed = (next: boolean) => {
      if (shared) {
        if (shared.timer !== null) {
          clearTimeout(shared.timer);
          shared.timer = null;
        }
        shared.armed = next;
        setIsArmed(next);
        if (next) {
          shared.timer = setTimeout(() => {
            if (shared) {
              shared.armed = false;
              shared.timer = null;
            }
            setIsArmed(false);
          }, timeoutMs);
        }
      }
    };
    shared = {
      bindings,
      prefixCtrl,
      prefixKeyLower: prefixKey.toLowerCase(),
      timeoutMs,
      armed: false,
      setArmed,
      timer: null,
    };
    ensureDispatcher().attachTo(document);
    return () => {
      if (shared && shared.timer !== null) {
        clearTimeout(shared.timer);
      }
      if (dispatcher) dispatcher.detachFrom(document);
      shared = null;
    };
  }, [prefixCtrl, prefixKey, timeoutMs]);

  // Bindings close over per-render values; sync each render so the listener
  // always sees the current closure rather than the one captured at mount.
  if (shared) shared.bindings = bindings;

  return { isArmed };
}

if (import.meta.hot) {
  // Vite HMR replaces this module without re-running consumers' effects;
  // detach the old listener so we don't accumulate duplicate keydown handlers.
  import.meta.hot.dispose(() => {
    if (dispatcher) dispatcher.detachFrom(document);
    shared = null;
    dispatcher = null;
  });
}
