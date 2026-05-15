//! IME composition wiring — xterm.js textarea.value pattern.

/** Composition state shared between the listener and the parent hook. */
export interface CompositionState {
  isSendingComposition: boolean;
  startValue: number;
  pendingTimer: number | null;
}

/** Wires IME composition events on the textarea.
 *
 * Reads finalized text from `ta.value.substring(startOffset)` (NOT from
 * `compositionend.data`, which is unreliable on Chromium per xterm.js'
 * `CompositionHelper.ts`). All paths funnel through `finishPending` so the
 * state machine cannot get stuck with `isSendingComposition=true` across a
 * component unmount — which would silently swallow every keystroke on the
 * next mount because the parent keydown handler short-circuits on that flag.
 */
export function setupComposition(
  ta: HTMLTextAreaElement,
  onPreedit: (text: string) => void,
  onSubmit: (text: string) => void,
  state: CompositionState,
): () => void {
  function finishPending(opts: { submit: false } | { submit: true; startOffset: number }): void {
    if (state.pendingTimer !== null) {
      clearTimeout(state.pendingTimer);
      state.pendingTimer = null;
    }
    if (opts.submit) {
      const text = ta.value.substring(opts.startOffset);
      if (text) onSubmit(text);
    }
    ta.value = '';
    onPreedit('');
    state.isSendingComposition = false;
  }

  const onCompositionStart = (): void => {
    state.startValue = ta.value.length;
    onPreedit('');
  };

  const onCompositionUpdate = (e: CompositionEvent): void => {
    onPreedit(e.data ?? '');
  };

  const onCompositionEnd = (): void => {
    state.isSendingComposition = true;
    const capturedStart = state.startValue;
    state.pendingTimer = window.setTimeout(
      () => finishPending({ submit: true, startOffset: capturedStart }),
      0,
    );
  };

  const onBlur = (): void => {
    // NOTE: route detached blur to no-submit because the parent effect may
    // already have torn down `send`.
    if (!ta.isConnected) {
      finishPending({ submit: false });
      return;
    }
    finishPending({ submit: true, startOffset: state.startValue });
  };

  ta.addEventListener('compositionstart', onCompositionStart);
  ta.addEventListener('compositionupdate', onCompositionUpdate);
  ta.addEventListener('compositionend', onCompositionEnd);
  ta.addEventListener('blur', onBlur);

  return () => {
    finishPending({ submit: false });
    ta.removeEventListener('compositionstart', onCompositionStart);
    ta.removeEventListener('compositionupdate', onCompositionUpdate);
    ta.removeEventListener('compositionend', onCompositionEnd);
    ta.removeEventListener('blur', onBlur);
  };
}
