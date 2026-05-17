import { useEffect, useState } from 'react';
import { isRenamePromptOpen } from '../shortcuts/renamePromptGate';
import { setChooseTreeOpen } from './chooseTreeGate';

/** Closed/open state of the choose-tree picker. */
export type ChooseTreeState = { open: false } | { open: true };

/** Imperative API for the choose-tree picker overlay. */
export interface ChooseTreeApi {
  state: ChooseTreeState;
  open: () => void;
  close: () => void;
}

/**
 * Owns the choose-tree picker's open/close state. Mirrors
 * `useRenameWindowPrompt` — the module gate (`chooseTreeGate`) is kept
 * in sync via an effect so React state is the single source of truth.
 * `open()` refuses while the rename prompt is open (modal mutual
 * exclusion).
 */
export function useChooseTree(): ChooseTreeApi {
  const [state, setState] = useState<ChooseTreeState>({ open: false });

  // NOTE: the cleanup resets the gate so an unmount mid-picker cannot
  // leave the global shortcut dispatcher permanently gated.
  useEffect(() => {
    setChooseTreeOpen(state.open);
    return () => setChooseTreeOpen(false);
  }, [state.open]);

  const open = () => {
    if (isRenamePromptOpen()) return;
    setState({ open: true });
  };

  const close = () => {
    setState((prev) => (prev.open ? { open: false } : prev));
  };

  return { state, open, close };
}
