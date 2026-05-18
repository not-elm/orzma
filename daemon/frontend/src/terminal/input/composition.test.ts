import { describe, expect, it, vi } from 'vitest';
import { type CompositionState, setupComposition } from './composition';

function freshState(): CompositionState {
  return { isComposing: false };
}

function fireComposition(ta: HTMLTextAreaElement, type: string, data: string | null): void {
  const e = new Event(type) as CompositionEvent;
  Object.defineProperty(e, 'data', { value: data });
  ta.dispatchEvent(e);
}

function fireInput(ta: HTMLTextAreaElement, inputType: string, data: string | null): void {
  const e = new Event('input') as InputEvent;
  Object.defineProperty(e, 'inputType', { value: inputType });
  Object.defineProperty(e, 'data', { value: data });
  ta.dispatchEvent(e);
}

describe('setupComposition', () => {
  it('compositionend with non-empty data submits synchronously', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onPreedit = vi.fn();
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, onPreedit, onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    expect(state.isComposing).toBe(true);
    fireComposition(ta, 'compositionupdate', 'こんにちは');
    expect(onPreedit).toHaveBeenLastCalledWith('こんにちは');
    ta.value = 'こんにちは';
    fireComposition(ta, 'compositionend', 'こんにちは');

    expect(onSubmit).toHaveBeenCalledWith('こんにちは');
    expect(ta.value).toBe('');
    expect(state.isComposing).toBe(false);
    expect(onPreedit).toHaveBeenLastCalledWith('');

    document.body.removeChild(ta);
  });

  it('compositionend with empty data does not submit', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    fireComposition(ta, 'compositionend', '');

    expect(onSubmit).not.toHaveBeenCalled();
    expect(state.isComposing).toBe(false);

    document.body.removeChild(ta);
  });

  it('sequential compositions submit their own data without loss', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'a';
    fireComposition(ta, 'compositionend', 'a');
    expect(onSubmit).toHaveBeenNthCalledWith(1, 'a');
    expect(ta.value).toBe('');

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'b';
    fireComposition(ta, 'compositionend', 'b');
    expect(onSubmit).toHaveBeenNthCalledWith(2, 'b');
    expect(ta.value).toBe('');

    document.body.removeChild(ta);
  });

  it('input event with insertText commits when not composing', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    ta.value = 'x';
    fireInput(ta, 'insertText', 'x');

    expect(onSubmit).toHaveBeenCalledWith('x');
    expect(ta.value).toBe('');

    document.body.removeChild(ta);
  });

  it('input event during composition is ignored (composition path owns the commit)', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'あ';
    fireInput(ta, 'insertCompositionText', 'あ');
    expect(onSubmit).not.toHaveBeenCalled();

    fireComposition(ta, 'compositionend', 'あ');
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith('あ');

    document.body.removeChild(ta);
  });

  it('input event with non-insertText inputType does not submit', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireInput(ta, 'deleteContentBackward', null);
    fireInput(ta, 'insertCompositionText', 'partial');
    fireInput(ta, 'insertLineBreak', null);

    expect(onSubmit).not.toHaveBeenCalled();

    document.body.removeChild(ta);
  });

  it('blur during composition flushes whatever the textarea holds', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'hi';
    ta.dispatchEvent(new Event('blur'));

    expect(onSubmit).toHaveBeenCalledWith('hi');
    expect(state.isComposing).toBe(false);
    expect(ta.value).toBe('');

    document.body.removeChild(ta);
  });

  it('blur outside composition does not fire onSubmit', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    ta.dispatchEvent(new Event('blur'));
    expect(onSubmit).not.toHaveBeenCalled();

    document.body.removeChild(ta);
  });

  it('cleanup resets state and clears textarea', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    const cleanup = setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'queued';
    expect(state.isComposing).toBe(true);

    cleanup();
    expect(state.isComposing).toBe(false);
    expect(ta.value).toBe('');

    fireInput(ta, 'insertText', 'after-cleanup');
    expect(onSubmit).not.toHaveBeenCalled();

    document.body.removeChild(ta);
  });

  // Regression: macOS / Chromium with an IME selected (even ABC mode) fires
  // keydown with `isComposing=true` on ASCII printable keys without ever
  // starting a real composition. `keymap.handleKeyDown` returns null in that
  // case, `useTerminal`'s keydown handler skips `preventDefault`, and the
  // character lands in the textarea via the browser default. The `input`
  // listener is the only thing that can flush it; without this, the first
  // keystroke is silently swallowed and the next keystroke commits the
  // previous one (the bug this file was rewritten to fix).
  it('flushes printable text that bypassed the keymap (macOS IME-on-ASCII regression)', () => {
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    // Simulate `a` slipping past `keydown` with isComposing=true; the browser
    // appends it to the textarea, then dispatches `input` insertText.
    ta.value = 'a';
    fireInput(ta, 'insertText', 'a');
    expect(onSubmit).toHaveBeenNthCalledWith(1, 'a');
    expect(ta.value).toBe('');

    // Next keystroke: same thing — must not pick up the previous char.
    ta.value = 'b';
    fireInput(ta, 'insertText', 'b');
    expect(onSubmit).toHaveBeenNthCalledWith(2, 'b');
    expect(ta.value).toBe('');

    document.body.removeChild(ta);
  });
});
