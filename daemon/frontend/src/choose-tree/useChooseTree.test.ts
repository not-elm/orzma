import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { isRenamePromptOpen, setRenamePromptOpen } from '../shortcuts/renamePromptGate';
import { isChooseTreeOpen, setChooseTreeOpen } from './chooseTreeGate';
import { useChooseTree } from './useChooseTree';

afterEach(() => {
  setChooseTreeOpen(false);
  setRenamePromptOpen(false);
});

describe('useChooseTree', () => {
  it('open() sets the gate and exposes open=true', () => {
    const { result } = renderHook(() => useChooseTree());
    expect(result.current.state.open).toBe(false);
    act(() => result.current.open());
    expect(result.current.state.open).toBe(true);
    expect(isChooseTreeOpen()).toBe(true);
  });

  it('close() clears the gate', () => {
    const { result } = renderHook(() => useChooseTree());
    act(() => result.current.open());
    act(() => result.current.close());
    expect(result.current.state.open).toBe(false);
    expect(isChooseTreeOpen()).toBe(false);
  });

  it('open() is a no-op while the rename prompt is open', () => {
    setRenamePromptOpen(true);
    const { result } = renderHook(() => useChooseTree());
    act(() => result.current.open());
    expect(result.current.state.open).toBe(false);
  });

  it('unmount while open clears the gate', () => {
    const { result, unmount } = renderHook(() => useChooseTree());
    act(() => result.current.open());
    unmount();
    expect(isChooseTreeOpen()).toBe(false);
  });
});
