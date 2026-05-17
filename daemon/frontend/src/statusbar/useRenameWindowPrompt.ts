import { useEffect, useRef, useState } from 'react';
import type { WindowId } from '../layout/types';
import { setRenamePromptOpen } from '../shortcuts/renamePromptGate';

/** Closed/open state of the rename-window prompt. */
export type RenamePromptState =
  | { open: false }
  | {
      open: true;
      windowId: WindowId;
      initialName: string;
    };

/** Options for {@link RenameWindowPromptApi.closePrompt}. */
export interface ClosePromptOptions {
  /**
   * When true, focus is returned to the element captured at open time.
   * Pass true for key-originated closes (`Enter`/`Esc`) and false for
   * `blur`-originated closes (the browser already moved focus).
   */
  restoreFocus: boolean;
}

/** Imperative API for opening and closing the rename-window prompt. */
interface RenameWindowPromptApi {
  promptState: RenamePromptState;
  openPrompt: (windowId: WindowId, currentName: string) => void;
  closePrompt: (options: ClosePromptOptions) => void;
}

/**
 * Owns the rename-window prompt's open/close state. `openPrompt`
 * captures `document.activeElement` so a key-originated `closePrompt`
 * can restore focus. The module gate (`renamePromptGate`) is kept in
 * sync via an effect, so React state is the single source of truth.
 */
export function useRenameWindowPrompt(): RenameWindowPromptApi {
  const [promptState, setPromptState] = useState<RenamePromptState>({ open: false });
  const returnFocusRef = useRef<HTMLElement | null>(null);

  // NOTE: the cleanup resets the gate so an unmount mid-prompt cannot
  // leave the global shortcut dispatcher permanently gated.
  useEffect(() => {
    setRenamePromptOpen(promptState.open);
    return () => setRenamePromptOpen(false);
  }, [promptState.open]);

  const openPrompt = (windowId: WindowId, currentName: string) => {
    // NOTE: capture the focused element before the prompt input mounts;
    // callers must invoke openPrompt before that, or returnFocus would
    // capture the prompt's own input.
    returnFocusRef.current = document.activeElement as HTMLElement | null;
    setPromptState({ open: true, windowId, initialName: currentName });
  };

  const closePrompt = (options: ClosePromptOptions) => {
    setPromptState((prev) => (prev.open ? { open: false } : prev));
    if (options.restoreFocus) returnFocusRef.current?.focus();
    returnFocusRef.current = null;
  };

  return { promptState, openPrompt, closePrompt };
}
