import { describe, expect, it, vi } from 'vitest';
import { type CompositionState, setupComposition } from './composition';

function freshState(): CompositionState {
  return { isSendingComposition: false, startValue: 0, pendingTimer: null };
}

function fireComposition(ta: HTMLTextAreaElement, type: string, data: string | null): void {
  const e = new Event(type) as CompositionEvent;
  Object.defineProperty(e, 'data', { value: data });
  ta.dispatchEvent(e);
}

describe('setupComposition', () => {
  it('reads finalized text from textarea.value after setTimeout(0)', async () => {
    vi.useFakeTimers();
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onPreedit = vi.fn();
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, onPreedit, onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'こんにちは';
    fireComposition(ta, 'compositionupdate', 'こんにちは');
    fireComposition(ta, 'compositionend', 'こんにちは');

    expect(state.isSendingComposition).toBe(true);
    expect(onSubmit).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(0);

    expect(onSubmit).toHaveBeenCalledWith('こんにちは');
    expect(ta.value).toBe('');
    expect(state.isSendingComposition).toBe(false);
    expect(state.pendingTimer).toBeNull();

    document.body.removeChild(ta);
    vi.useRealTimers();
  });

  it('captures startValue per composition so concurrent compositionstart does not overwrite', async () => {
    vi.useFakeTimers();
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    ta.value = 'prefix:';
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    expect(state.startValue).toBe(7);
    ta.value = 'prefix:あ';
    fireComposition(ta, 'compositionend', 'あ');

    fireComposition(ta, 'compositionstart', null);
    // Second compositionstart overwrites state.startValue (now 8 = 'prefix:あ'.length),
    // but the in-flight timer captured 7 via const, so its submit must still read from offset 7.
    expect(state.startValue).toBe(8);

    await vi.advanceTimersByTimeAsync(0);

    expect(onSubmit).toHaveBeenCalledWith('あ');

    document.body.removeChild(ta);
    vi.useRealTimers();
  });

  it('blur flushes a pending submit', () => {
    vi.useFakeTimers();
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'hi';
    fireComposition(ta, 'compositionend', 'hi');
    ta.dispatchEvent(new Event('blur'));

    expect(onSubmit).toHaveBeenCalledWith('hi');
    expect(state.pendingTimer).toBeNull();
    expect(state.isSendingComposition).toBe(false);

    document.body.removeChild(ta);
    vi.useRealTimers();
  });

  it('empty textarea-delta at submit time does not call onSubmit', async () => {
    vi.useFakeTimers();
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    fireComposition(ta, 'compositionend', '');
    await vi.advanceTimersByTimeAsync(0);

    expect(onSubmit).not.toHaveBeenCalled();
    expect(state.isSendingComposition).toBe(false);

    document.body.removeChild(ta);
    vi.useRealTimers();
  });

  it('cleanup resets isSendingComposition and clears pending timer', async () => {
    vi.useFakeTimers();
    const ta = document.createElement('textarea');
    document.body.appendChild(ta);
    const onSubmit = vi.fn();
    const state = freshState();
    const cleanup = setupComposition(ta, vi.fn(), onSubmit, state);

    fireComposition(ta, 'compositionstart', null);
    ta.value = 'queued';
    fireComposition(ta, 'compositionend', 'queued');
    expect(state.isSendingComposition).toBe(true);
    expect(state.pendingTimer).not.toBeNull();

    cleanup();
    expect(state.isSendingComposition).toBe(false);
    expect(state.pendingTimer).toBeNull();

    await vi.advanceTimersByTimeAsync(0);
    expect(onSubmit).not.toHaveBeenCalled();

    document.body.removeChild(ta);
    vi.useRealTimers();
  });
});
