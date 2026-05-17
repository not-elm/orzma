/**
 * @file Module-level gate flag indicating whether the choose-tree
 * picker overlay is open. Read by the global shortcut dispatcher to
 * suppress shortcuts (and by `useRenameWindowPrompt` to refuse a
 * simultaneous open); written by `useChooseTree`, which keeps it in
 * sync with React state.
 */

let pickerOpen = false;

/** Records whether the choose-tree picker is currently open. */
export function setChooseTreeOpen(open: boolean): void {
  pickerOpen = open;
}

/** Returns whether the choose-tree picker is currently open. */
export function isChooseTreeOpen(): boolean {
  return pickerOpen;
}

if (import.meta.hot) {
  // NOTE: reset the gate on HMR dispose so a hot reload while the
  // picker is open cannot leave the dispatcher permanently gated.
  import.meta.hot.dispose(() => {
    pickerOpen = false;
  });
}
