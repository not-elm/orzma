import { useEffect, useState } from 'react';
import { fetchJson } from '../fetchJson';
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
  repeatable: boolean;
}

const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta']);

interface SharedState {
  bindings: ReadonlyArray<ChordBinding>;
  prefix: Prefix;
  repeatTimeoutMs: number;
  armed: boolean;
  repeatMode: boolean;
  setArmed: (next: boolean) => void;
  prefixTimer: ReturnType<typeof setTimeout> | null;
  repeatTimer: ReturnType<typeof setTimeout> | null;
}

let shared: SharedState | null = null;
let dispatcher: KeyDispatcher | null = null;
let moduleDisposed = false;

function ensureDispatcher() {
  if (dispatcher) return dispatcher;
  const handler = (e: KeyboardEvent) => {
    if (!shared) return;
    if (e.isComposing) return;

    // Branch 1: not armed AND not in repeat mode.
    if (!shared.armed && !shared.repeatMode) {
      if (e.repeat) return;
      if (!matchesChord(e, shared.prefix)) return;
      e.preventDefault();
      e.stopPropagation();
      shared.setArmed(true);
      return;
    }

    // Branch 2: in repeat sub-mode — no prefix needed for repeatable
    // bindings; e.repeat is accepted.
    if (shared.repeatMode) {
      // Modifier-only keypress is ignored; don't consume.
      if (MODIFIER_KEYS.has(e.key)) return;
      const match = shared.bindings.find((b) => matchesChord(e, b.chord));
      if (match?.repeatable) {
        e.preventDefault();
        e.stopPropagation();
        match.handler();
        if (shared.repeatTimer !== null) clearTimeout(shared.repeatTimer);
        shared.repeatTimer = setTimeout(() => {
          if (shared) {
            shared.repeatMode = false;
            shared.repeatTimer = null;
          }
        }, shared.repeatTimeoutMs);
        return;
      }
      // Non-repeatable / no-match chord exits repeat mode and is NOT
      // consumed — falls through to the terminal as a normal key.
      shared.repeatMode = false;
      if (shared.repeatTimer !== null) {
        clearTimeout(shared.repeatTimer);
        shared.repeatTimer = null;
      }
      return;
    }

    // Branch 3: armed mode (prefix has just been pressed). Consume every
    // event so it cannot leak to xterm.js or iframes.
    e.preventDefault();
    e.stopPropagation();

    if (MODIFIER_KEYS.has(e.key) || e.repeat) return;

    if (e.key === 'Escape' || matchesChord(e, shared.prefix)) {
      shared.setArmed(false);
      return;
    }

    const match = shared.bindings.find((b) => matchesChord(e, b.chord));
    if (match) {
      match.handler();
      if (match.repeatable) {
        // Transition from armed → repeat: disarm visibly but stay
        // listening for further repeatable chords.
        shared.setArmed(false);
        shared.repeatMode = true;
        if (shared.repeatTimer !== null) clearTimeout(shared.repeatTimer);
        shared.repeatTimer = setTimeout(() => {
          if (shared) {
            shared.repeatMode = false;
            shared.repeatTimer = null;
          }
        }, shared.repeatTimeoutMs);
        return;
      }
    }
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
    // NOTE: Network failures and schema failures both map to status:'error';
    // the root cause is logged at the point of failure (fetchJson / parseShortcuts).
    const setIfAlive = (next: { status: 'loading' | 'ready' | 'error'; prefix: Prefix | null }) => {
      if (!cancelled && !moduleDisposed) setState(next);
    };
    (async () => {
      let payload: unknown;
      try {
        payload = await fetchJson('/configs/shortcuts');
      } catch (e) {
        console.warn('usePrefixMode: fetch failed', e);
        setIfAlive({ status: 'error', prefix: null });
        return;
      }
      if (cancelled || moduleDisposed) return;
      const parsed = parseShortcuts(payload);
      if (!parsed) {
        setIfAlive({ status: 'error', prefix: null });
        return;
      }

      const bindings: ChordBinding[] = [];
      for (const b of parsed.bindings) {
        const handler = actionToHandler(b.action, ctx);
        if (handler) bindings.push({ chord: b.chord, handler, repeatable: b.repeatable });
      }

      const setArmed = (next: boolean) => {
        if (!shared) return;
        if (shared.prefixTimer !== null) {
          clearTimeout(shared.prefixTimer);
          shared.prefixTimer = null;
        }
        shared.armed = next;
        setIsArmed(next);
        if (next) {
          shared.prefixTimer = setTimeout(() => {
            if (shared) {
              shared.armed = false;
              shared.prefixTimer = null;
            }
            setIsArmed(false);
          }, parsed.prefix.timeout_ms);
        }
      };

      shared = {
        bindings,
        prefix: parsed.prefix,
        repeatTimeoutMs: parsed.repeat_timeout_ms,
        armed: false,
        repeatMode: false,
        setArmed,
        prefixTimer: null,
        repeatTimer: null,
      };
      ensureDispatcher().attachTo(document);
      setIfAlive({ status: 'ready', prefix: parsed.prefix });
    })();

    return () => {
      cancelled = true;
      if (shared) {
        if (shared.prefixTimer !== null) clearTimeout(shared.prefixTimer);
        if (shared.repeatTimer !== null) clearTimeout(shared.repeatTimer);
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
    if (shared) {
      if (shared.prefixTimer !== null) clearTimeout(shared.prefixTimer);
      if (shared.repeatTimer !== null) clearTimeout(shared.repeatTimer);
    }
    if (dispatcher) dispatcher.detachFrom(document);
    shared = null;
    dispatcher = null;
  });
}
