import { useEffect, useRef, useState } from 'react';
import { createKeyDispatcher } from './globalKeyDispatcher';

export type PrefixBindings = ReadonlyMap<string, () => void>;

export interface PrefixModeOptions {
  prefix?: { ctrl: boolean; key: string };
  timeoutMs?: number;
}

export interface PrefixModeState {
  isArmed: boolean;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta']);

export function usePrefixMode(
  bindings: PrefixBindings,
  options?: PrefixModeOptions,
): PrefixModeState {
  const [isArmed, setIsArmed] = useState(false);

  const bindingsRef = useRef(bindings);
  bindingsRef.current = bindings;

  const armedRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const prefix = options?.prefix ?? { ctrl: true, key: 'b' };
  const timeoutMs = options?.timeoutMs ?? 2000;
  const prefixKey = prefix.key.toLowerCase();
  const prefixCtrl = prefix.ctrl;

  useEffect(() => {
    const clearTimer = () => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
    const setArmed = (next: boolean) => {
      armedRef.current = next;
      setIsArmed(next);
      clearTimer();
      if (next) {
        timerRef.current = setTimeout(() => {
          armedRef.current = false;
          setIsArmed(false);
          timerRef.current = null;
        }, timeoutMs);
      }
    };

    const handler = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      const key = e.key.toLowerCase();

      if (!armedRef.current) {
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
          setArmed(true);
        }
        return;
      }

      // armed
      if (MODIFIER_KEYS.has(e.key)) {
        e.preventDefault();
        return;
      }
      if (e.repeat) {
        e.preventDefault();
        return;
      }

      e.preventDefault();

      // cancel via Escape or repeated prefix
      if (e.key === 'Escape' || (key === prefixKey && e.ctrlKey === prefixCtrl)) {
        setArmed(false);
        return;
      }

      const action = bindingsRef.current.get(key);
      if (action) {
        action();
      }
      setArmed(false);
    };

    const dispatcher = createKeyDispatcher(handler);
    dispatcher.attachTo(document);
    return () => {
      dispatcher.detachFrom(document);
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [prefixKey, prefixCtrl, timeoutMs]);

  return { isArmed };
}
