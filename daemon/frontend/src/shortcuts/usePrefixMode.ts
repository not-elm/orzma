import { useEffect, useState } from 'react';
import { isChooseTreeOpen } from '../choose-tree/chooseTreeGate';
import { fetchJson } from '../fetchJson';
import { actionToHandler, type ShortcutContext } from './actionDispatch';
import { matchesChord } from './chord';
import { createKeyDispatcher, type KeyDispatcher } from './globalKeyDispatcher';
import { isRenamePromptOpen } from './renamePromptGate';
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
 * the dispatcher early-returns.
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

function armRepeatTimer(state: SharedState) {
  if (state.repeatTimer !== null) clearTimeout(state.repeatTimer);
  state.repeatTimer = setTimeout(() => {
    if (shared) {
      shared.repeatMode = false;
      shared.repeatTimer = null;
    }
  }, state.repeatTimeoutMs);
}

function ensureDispatcher() {
  if (dispatcher) return dispatcher;
  const handler = (e: KeyboardEvent) => {
    if (!shared) return;
    if (e.isComposing) return;
    if (isRenamePromptOpen() || isChooseTreeOpen()) return;

    if (!shared.armed && !shared.repeatMode) {
      if (e.repeat) return;
      if (!matchesChord(e, shared.prefix)) return;
      e.preventDefault();
      e.stopPropagation();
      shared.setArmed(true);
      return;
    }

    if (shared.repeatMode) {
      if (MODIFIER_KEYS.has(e.key)) return;
      const match = shared.bindings.find((b) => matchesChord(e, b.chord));
      if (match?.repeatable) {
        e.preventDefault();
        e.stopPropagation();
        match.handler();
        armRepeatTimer(shared);
        return;
      }
      // NOTE: no preventDefault here — the chord must reach the terminal.
      shared.repeatMode = false;
      if (shared.repeatTimer !== null) {
        clearTimeout(shared.repeatTimer);
        shared.repeatTimer = null;
      }
      return;
    }

    // NOTE: in armed mode consume every event so it cannot leak to the terminal.
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
        shared.setArmed(false);
        shared.repeatMode = true;
        armRepeatTimer(shared);
        return;
      }
    }
    shared.setArmed(false);
  };
  dispatcher = createKeyDispatcher(handler);
  return dispatcher;
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
      if (parsed.prefix === null) {
        // NOTE: new named-field shortcut backend; React frontend dispatcher
        // is intentionally disabled here (see D2 in the design spec).
        setIfAlive({ status: 'ready', prefix: null });
        return;
      }

      const resolvedPrefix = parsed.prefix;

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
          }, resolvedPrefix.timeout_ms);
        }
      };

      shared = {
        bindings,
        prefix: resolvedPrefix,
        repeatTimeoutMs: parsed.repeat_timeout_ms ?? 500,
        armed: false,
        repeatMode: false,
        setArmed,
        prefixTimer: null,
        repeatTimer: null,
      };
      ensureDispatcher().attachTo(document);
      setIfAlive({ status: 'ready', prefix: resolvedPrefix });
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
  // NOTE: detach the old listener on HMR dispose so we don't accumulate duplicate keydown handlers.
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
