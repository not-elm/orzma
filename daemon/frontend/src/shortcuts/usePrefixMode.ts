import { useEffect, useRef, useState } from 'react';
import { createKeyDispatcher, type KeyDispatcher } from './globalKeyDispatcher';

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
  prefix: { ctrl: boolean; key: string };
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
    const prefixKey = shared.prefix.key.toLowerCase();
    const prefixCtrl = shared.prefix.ctrl;

    if (!shared.armed) {
      const isPrefix =
        key === prefixKey &&
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

    if (MODIFIER_KEYS.has(e.key)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (e.repeat) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }

    e.preventDefault();
    e.stopPropagation();

    if (e.key === 'Escape' || (key === prefixKey && e.ctrlKey === prefixCtrl)) {
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

  const bindingsRef = useRef(bindings);
  bindingsRef.current = bindings;

  // Use primitive deps so a default object literal does not cause re-subscribe
  // on every render.
  const prefixCtrl = options?.prefix?.ctrl ?? true;
  const prefixKey = options?.prefix?.key ?? 'b';
  const timeoutMs = options?.timeoutMs ?? 2000;

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
      bindings: bindingsRef.current,
      prefix: { ctrl: prefixCtrl, key: prefixKey },
      timeoutMs,
      armed: false,
      setArmed,
      timer: null,
    };
    ensureDispatcher().attachTo(document);
    return () => {
      if (shared?.timer !== null && shared) {
        clearTimeout(shared.timer);
      }
      if (dispatcher) dispatcher.detachFrom(document);
      shared = null;
    };
  }, [prefixCtrl, prefixKey, timeoutMs]);

  // Keep shared.bindings up to date every render via the ref.
  if (shared) shared.bindings = bindings;

  return { isArmed };
}
