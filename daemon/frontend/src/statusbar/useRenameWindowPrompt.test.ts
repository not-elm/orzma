import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { isRenamePromptOpen, setRenamePromptOpen } from '../shortcuts/renamePromptGate';
import { useRenameWindowPrompt } from './useRenameWindowPrompt';

afterEach(() => {
  setRenamePromptOpen(false);
  document.body.innerHTML = '';
});

describe('useRenameWindowPrompt', () => {
  it('starts closed with the gate closed', () => {
    const { result } = renderHook(() => useRenameWindowPrompt());
    expect(result.current.promptState.open).toBe(false);
    expect(isRenamePromptOpen()).toBe(false);
  });

  it('openPrompt opens the prompt and the gate, capturing the focused element', () => {
    const prior = document.createElement('button');
    document.body.appendChild(prior);
    prior.focus();
    const { result } = renderHook(() => useRenameWindowPrompt());
    act(() => {
      result.current.openPrompt('wid-1', 'old-name');
    });
    const state = result.current.promptState;
    expect(state).toEqual({
      open: true,
      windowId: 'wid-1',
      initialName: 'old-name',
      returnFocus: prior,
    });
    expect(isRenamePromptOpen()).toBe(true);
  });

  it('closePrompt with restoreFocus refocuses the captured element and closes the gate', () => {
    const prior = document.createElement('button');
    document.body.appendChild(prior);
    prior.focus();
    const { result } = renderHook(() => useRenameWindowPrompt());
    act(() => {
      result.current.openPrompt('wid-1', 'old-name');
    });
    const other = document.createElement('input');
    document.body.appendChild(other);
    other.focus();
    act(() => {
      result.current.closePrompt({ restoreFocus: true });
    });
    expect(result.current.promptState.open).toBe(false);
    expect(isRenamePromptOpen()).toBe(false);
    expect(document.activeElement).toBe(prior);
  });

  it('closePrompt without restoreFocus leaves focus untouched', () => {
    const prior = document.createElement('button');
    document.body.appendChild(prior);
    prior.focus();
    const { result } = renderHook(() => useRenameWindowPrompt());
    act(() => {
      result.current.openPrompt('wid-1', 'old-name');
    });
    const other = document.createElement('input');
    document.body.appendChild(other);
    other.focus();
    act(() => {
      result.current.closePrompt({ restoreFocus: false });
    });
    expect(result.current.promptState.open).toBe(false);
    expect(document.activeElement).toBe(other);
  });

  it('closePrompt is idempotent — a second call does not refocus', () => {
    const prior = document.createElement('button');
    document.body.appendChild(prior);
    prior.focus();
    const { result } = renderHook(() => useRenameWindowPrompt());
    act(() => {
      result.current.openPrompt('wid-1', 'old-name');
    });
    act(() => {
      result.current.closePrompt({ restoreFocus: true });
    });
    const other = document.createElement('input');
    document.body.appendChild(other);
    other.focus();
    act(() => {
      result.current.closePrompt({ restoreFocus: true });
    });
    expect(document.activeElement).toBe(other);
  });

  it('resets the gate on unmount', () => {
    const { result, unmount } = renderHook(() => useRenameWindowPrompt());
    act(() => {
      result.current.openPrompt('wid-1', 'old-name');
    });
    expect(isRenamePromptOpen()).toBe(true);
    unmount();
    expect(isRenamePromptOpen()).toBe(false);
  });
});
