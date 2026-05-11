import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { type PrefixBindings, usePrefixMode } from './usePrefixMode';

function press(opts: KeyboardEventInit & { key: string }) {
  document.dispatchEvent(
    new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...opts }),
  );
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('usePrefixMode', () => {
  it('Ctrl+B → x fires the binding once', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'x' });
    });
    expect(fire).toHaveBeenCalledTimes(1);
  });

  it('Ctrl+B → Escape returns to idle without firing', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(true);
    act(() => {
      press({ key: 'Escape' });
    });
    expect(result.current.isArmed).toBe(false);
    expect(fire).not.toHaveBeenCalled();
  });

  it('Ctrl+B → Ctrl+B cancels (idle)', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(false);
    expect(fire).not.toHaveBeenCalled();
  });

  it('2000ms timeout returns to idle', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(true);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(result.current.isArmed).toBe(false);
  });

  it('armed + unbound key returns to idle (and consumes the key)', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    let preventedDuringArmed = false;
    document.addEventListener(
      'keydown',
      (e) => {
        if (e.defaultPrevented) preventedDuringArmed = true;
      },
      { once: true },
    );
    act(() => {
      press({ key: 'q' });
    });
    expect(result.current.isArmed).toBe(false);
    expect(fire).not.toHaveBeenCalled();
    expect(preventedDuringArmed).toBe(true);
  });

  it('event.repeat does not arm', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true, repeat: true });
    });
    expect(result.current.isArmed).toBe(false);
  });

  it('event.isComposing keys are pass-through (no preventDefault, no state change)', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    const { result } = renderHook(() => usePrefixMode(bindings));
    let prevented = false;
    document.addEventListener(
      'keydown',
      (e) => {
        if (e.defaultPrevented) prevented = true;
      },
      { once: true },
    );
    act(() => {
      // jsdom KeyboardEvent doesn't accept isComposing in init; force via property
      const ev = new KeyboardEvent('keydown', { key: 'b', ctrlKey: true, bubbles: true });
      Object.defineProperty(ev, 'isComposing', { get: () => true });
      document.dispatchEvent(ev);
    });
    expect(result.current.isArmed).toBe(false);
    expect(prevented).toBe(false);
    expect(fire).not.toHaveBeenCalled();
  });

  it('case-insensitive: Shift+X while armed fires binding "x"', () => {
    const fire = vi.fn();
    const bindings: PrefixBindings = new Map([['x', fire]]);
    renderHook(() => usePrefixMode(bindings));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'X', shiftKey: true });
    });
    expect(fire).toHaveBeenCalledTimes(1);
  });

  it('passing a fresh Map every render does not re-register listeners', () => {
    const addSpy = vi.spyOn(document, 'addEventListener');
    const fire = vi.fn();
    const { rerender } = renderHook(
      ({ tick }: { tick: number }) => {
        const bindings: PrefixBindings = new Map([['x', () => fire(tick)]]);
        return usePrefixMode(bindings);
      },
      { initialProps: { tick: 0 } },
    );
    const initialAddCount = addSpy.mock.calls.filter((c) => c[0] === 'keydown').length;
    rerender({ tick: 1 });
    rerender({ tick: 2 });
    const afterAddCount = addSpy.mock.calls.filter((c) => c[0] === 'keydown').length;
    expect(afterAddCount).toBe(initialAddCount);
    // Still uses the latest binding closure
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'x' });
    });
    expect(fire).toHaveBeenCalledWith(2);
  });
});
