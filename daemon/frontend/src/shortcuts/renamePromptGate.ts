/**
 * @file Module-level gate flag indicating whether the rename-window
 * prompt is open. Read by the global shortcut dispatcher to suppress
 * shortcuts while the prompt has focus; written by `useRenameWindowPrompt`,
 * which keeps it in sync with React state.
 */

let promptOpen = false;

/** Records whether the rename-window prompt is currently open. */
export function setRenamePromptOpen(open: boolean): void {
  promptOpen = open;
}

/** Returns whether the rename-window prompt is currently open. */
export function isRenamePromptOpen(): boolean {
  return promptOpen;
}

if (import.meta.hot) {
  // NOTE: reset the gate on HMR dispose so a hot reload while the prompt
  // is open cannot leave the dispatcher permanently gated in dev.
  import.meta.hot.dispose(() => {
    promptOpen = false;
  });
}
