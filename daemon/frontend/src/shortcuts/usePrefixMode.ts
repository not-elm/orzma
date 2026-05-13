import { useEffect, useState } from 'react';
import { actionToHandler, type ShortcutContext } from './actionDispatch';
import { matchesChord } from './chord';
import { createKeyDispatcher, type KeyDispatcher } from './globalKeyDispatcher';
import { type KeyChord, type Prefix, parseShortcuts } from './wire';

/**
 * **INVARIANT: at most one `usePrefixMode` may be mounted at a time.**
 *
 * The hook owns module-level `shared` state and a singleton dispatcher
 * attached to `document`. A second concurrent caller would clobber
 * `shared` (silently breaking the first caller's `setArmed` callback)
 * and unmounting either would `detachFrom(document)`, killing keyboard
 * handling for the surviving caller. The intended owner is `App.tsx`.
 *
 * The hook also owns the fetch lifecycle for `GET /configs/shortcuts`.
 * Bindings are derived from the parsed `Shortcuts.bindings` and
 * `actionToHandler`. Until the fetch resolves, `shared` is `null` and
 * the dispatcher (if attached by `useIframeKeydownBridge`) early-returns.
 */
export interface PrefixModeState {
  isArmed: boolean;
  status: 'loading' | 'ready' | 'error';
  prefix: Prefix | null;
}

interface ChordBinding {
  chord: KeyChord;
  handler: () => void;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta']);

interface SharedState {
  bindings: ReadonlyArray<ChordBinding>;
  prefix: Prefix;
  armed: boolean;
  setArmed: (next: boolean) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

let shared: SharedState | null = null;
let dispatcher: KeyDispatcher | null = null;
let moduleDisposed = false;

function ensureDispatcher() {
  if (dispatcher) return dispatcher;
  const handler = (e: KeyboardEvent) => {
    if (!shared) return;
    if (e.isComposing) return;

    if (!shared.armed) {
      if (e.repeat) return;
      if (!matchesChord(e, shared.prefix)) return;
      e.preventDefault();
      e.stopPropagation();
      shared.setArmed(true);
      return;
    }

    // Armed mode consumes every event so it cannot leak to xterm.js or iframes.
    e.preventDefault();
    e.stopPropagation();

    if (MODIFIER_KEYS.has(e.key) || e.repeat) return;

    if (e.key === 'Escape' || matchesChord(e, shared.prefix)) {
      shared.setArmed(false);
      return;
    }

    const match = shared.bindings.find((b) => matchesChord(e, b.chord));
    if (match) match.handler();
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

export function usePrefixMode(ctx: ShortcutContext): PrefixModeState {
  const [isArmed, setIsArmed] = useState(false);
  const [state, setState] = useState<{
    status: 'loading' | 'ready' | 'error';
    prefix: Prefix | null;
  }>({ status: 'loading', prefix: null });

  // ctx members are read inside handlers at fire-time, not captured here.
  // biome-ignore lint/correctness/useExhaustiveDependencies: ctx callbacks are read at fire time; including ctx would re-run the effect and re-fetch on every render.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      let payload: unknown;
      try {
        const r = await fetch('/configs/shortcuts');
        if (!r.ok) throw new Error(`/configs/shortcuts: ${r.status} ${r.statusText}`);
        payload = await r.json();
      } catch (e) {
        console.warn('usePrefixMode: fetch failed', e);
        // NOTE: Network failures and schema failures both map to status:'error';
        // the root cause is logged at the point of failure (fetch catch / parseShortcuts).
        if (!cancelled && !moduleDisposed) setState({ status: 'error', prefix: null });
        return;
      }
      if (cancelled || moduleDisposed) return;
      const parsed = parseShortcuts(payload);
      if (!parsed) {
        if (!cancelled && !moduleDisposed) setState({ status: 'error', prefix: null });
        return;
      }
      if (cancelled || moduleDisposed) return;

      const bindings: ChordBinding[] = [];
      for (const b of parsed.bindings) {
        const handler = actionToHandler(b.action, ctx);
        if (handler) bindings.push({ chord: b.chord, handler });
      }

      const setArmed = (next: boolean) => {
        if (!shared) return;
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
          }, parsed.prefix.timeout_ms);
        }
      };

      shared = {
        bindings,
        prefix: parsed.prefix,
        armed: false,
        setArmed,
        timer: null,
      };
      ensureDispatcher().attachTo(document);
      setState({ status: 'ready', prefix: parsed.prefix });
    })();

    return () => {
      cancelled = true;
      if (shared && shared.timer !== null) {
        clearTimeout(shared.timer);
      }
      if (dispatcher) dispatcher.detachFrom(document);
      shared = null;
    };
  }, []);

  return { isArmed, status: state.status, prefix: state.prefix };
}

if (import.meta.hot) {
  // Vite HMR replaces this module without re-running consumers' effects;
  // detach the old listener so we don't accumulate duplicate keydown handlers.
  import.meta.hot.dispose(() => {
    moduleDisposed = true;
    if (dispatcher) dispatcher.detachFrom(document);
    shared = null;
    dispatcher = null;
  });
}
