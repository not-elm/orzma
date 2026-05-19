//! IME composition + plain-text input wiring on the hidden textarea sink.
//!
//! Two commit paths run side-by-side:
//!
//! - `compositionend.data` is sent verbatim (matches `browser/input/ime.ts`).
//!   The legacy xterm.js workaround that read `textarea.value.substring(...)`
//!   inside a `setTimeout(0)` is gone: under fast typing it raced with the
//!   next `compositionstart` and either delayed or dropped a character,
//!   producing the "first key ignored, next key types the previous one"
//!   symptom on macOS/Chromium when any IME is selected (even ABC).
//! - `input` with `inputType === 'insertText'` catches printable text that
//!   landed in the textarea outside a composition. `keydown` for ordinary
//!   printable keys calls `preventDefault`, so this path normally stays
//!   silent — but on macOS/Chromium with an IME installed, `keydown` can
//!   arrive with `event.isComposing === true` for ASCII keys yet no
//!   `compositionstart` ever fires, so the keymap returns null and the char
//!   silently fills the textarea. The `input` handler is the only thing that
//!   can flush it.
//!
//! Both paths reset `textarea.value` synchronously so nothing accumulates
//! across event boundaries.

/** Composition state shared with the parent hook's keydown short-circuit. */
export interface CompositionState {
  isComposing: boolean;
}

/** Wires composition + input listeners on the textarea. */
export function setupComposition(
  ta: HTMLTextAreaElement,
  onPreedit: (text: string) => void,
  onSubmit: (text: string) => void,
  state: CompositionState,
): () => void {
  const onCompositionStart = (): void => {
    state.isComposing = true;
    onPreedit('');
  };

  const onCompositionUpdate = (e: CompositionEvent): void => {
    onPreedit(e.data ?? '');
  };

  const onCompositionEnd = (e: CompositionEvent): void => {
    state.isComposing = false;
    onPreedit('');
    const text = e.data ?? '';
    if (text) onSubmit(text);
    ta.value = '';
  };

  const onInput = (e: Event): void => {
    if (state.isComposing) return;
    const ie = e as InputEvent;
    if (ie.inputType !== 'insertText') return;
    const data = ie.data ?? '';
    if (data) onSubmit(data);
    ta.value = '';
  };

  const onBlur = (): void => {
    // NOTE: a focus shift mid-composition leaves the textarea in a
    // half-finished state; commit whatever the textarea holds so a refocus
    // does not replay a stale preedit fragment.
    if (state.isComposing) {
      state.isComposing = false;
      onPreedit('');
      if (ta.isConnected) {
        const text = ta.value;
        if (text) onSubmit(text);
      }
      ta.value = '';
    }
  };

  ta.addEventListener('compositionstart', onCompositionStart);
  ta.addEventListener('compositionupdate', onCompositionUpdate);
  ta.addEventListener('compositionend', onCompositionEnd);
  ta.addEventListener('input', onInput);
  ta.addEventListener('blur', onBlur);

  return () => {
    state.isComposing = false;
    onPreedit('');
    ta.value = '';
    ta.removeEventListener('compositionstart', onCompositionStart);
    ta.removeEventListener('compositionupdate', onCompositionUpdate);
    ta.removeEventListener('compositionend', onCompositionEnd);
    ta.removeEventListener('input', onInput);
    ta.removeEventListener('blur', onBlur);
  };
}
