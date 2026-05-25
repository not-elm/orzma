import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { makeShortcutContext } from './__test-helpers';
import { setRenamePromptOpen } from './renamePromptGate';
import { usePrefixMode } from './usePrefixMode';

const DEFAULT_PAYLOAD = {
  prefix: {
    key: 'b',
    modifiers: { ctrl: true, shift: false, alt: false, meta: false },
    timeout_ms: 2000,
  },
  bindings: [
    {
      key: 'x',
      modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      action: { type: 'close-pane' },
    },
    {
      key: 's',
      modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      action: { type: 'split-pane', direction: 'horizontal' },
    },
    {
      key: 'v',
      modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      action: { type: 'split-pane', direction: 'vertical' },
    },
  ],
};

const origFetch = globalThis.fetch;
let closeFetchMock: ReturnType<typeof vi.fn<typeof fetch>>;
let splitFetchMock: ReturnType<typeof vi.fn<typeof fetch>>;
let configFetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

function press(opts: KeyboardEventInit & { key: string }) {
  document.dispatchEvent(
    new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...opts }),
  );
}

function makeCtx() {
  return makeShortcutContext({
    activeWindow: () => 'wid-1',
    activePane: () => 'pid-1',
  });
}

beforeEach(() => {
  // shouldAdvanceTime lets waitFor's polling interval fire under fake timers.
  vi.useFakeTimers({ shouldAdvanceTime: true });
  closeFetchMock = vi.fn<typeof fetch>().mockResolvedValue({ ok: true, status: 204 } as Response);
  splitFetchMock = vi.fn<typeof fetch>().mockResolvedValue({ ok: true, status: 201 } as Response);
  configFetchMock = vi.fn<typeof fetch>().mockResolvedValue({
    ok: true,
    status: 200,
    json: async () => DEFAULT_PAYLOAD,
  } as Response);
  globalThis.fetch = ((url: RequestInfo | URL, init?: RequestInit) => {
    const path = typeof url === 'string' ? url : url.toString();
    if (path === '/configs/shortcuts') return configFetchMock(url, init);
    if (path.endsWith('/split')) return splitFetchMock(url, init);
    return closeFetchMock(url, init);
  }) as typeof globalThis.fetch;
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  globalThis.fetch = origFetch;
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('usePrefixMode', () => {
  it('starts in loading and transitions to ready after fetch', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    expect(result.current.status).toBe('loading');
    expect(result.current.prefix).toBeNull();
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect(result.current.prefix?.key).toBe('b');
  });

  it('Ctrl+B → x fires the close-pane binding', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'x' });
    });
    await waitFor(() =>
      expect(closeFetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1', {
        method: 'DELETE',
      }),
    );
  });

  it('Ctrl+B → Escape returns to idle without firing', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(true);
    act(() => {
      press({ key: 'Escape' });
    });
    expect(result.current.isArmed).toBe(false);
    expect(closeFetchMock).not.toHaveBeenCalled();
  });

  it('Ctrl+B → Ctrl+B disarms (idle)', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(false);
    expect(closeFetchMock).not.toHaveBeenCalled();
  });

  it('timeout (timeout_ms from config) returns to idle', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(true);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(result.current.isArmed).toBe(false);
  });

  it('armed + unbound key returns to idle and consumes the key', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    let preventedDuringArmed = false;
    document.addEventListener(
      'keydown',
      (e) => {
        if (e.defaultPrevented) preventedDuringArmed = true;
      },
      { capture: true, once: true },
    );
    act(() => {
      press({ key: 'q' });
    });
    expect(result.current.isArmed).toBe(false);
    expect(preventedDuringArmed).toBe(true);
    expect(closeFetchMock).not.toHaveBeenCalled();
  });

  it('armed + key press calls both preventDefault and stopPropagation', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });

    const ev = new KeyboardEvent('keydown', { key: 'q', bubbles: true, cancelable: true });
    const preventSpy = vi.spyOn(ev, 'preventDefault');
    const stopSpy = vi.spyOn(ev, 'stopPropagation');
    act(() => {
      document.dispatchEvent(ev);
    });
    expect(preventSpy).toHaveBeenCalled();
    expect(stopSpy).toHaveBeenCalled();
    expect(result.current.isArmed).toBe(false);
  });

  it('event.repeat does not arm', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true, repeat: true });
    });
    expect(result.current.isArmed).toBe(false);
  });

  it('event.isComposing keys are pass-through', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    let prevented = false;
    document.addEventListener(
      'keydown',
      (e) => {
        if (e.defaultPrevented) prevented = true;
      },
      { once: true },
    );
    act(() => {
      const ev = new KeyboardEvent('keydown', { key: 'b', ctrlKey: true, bubbles: true });
      Object.defineProperty(ev, 'isComposing', { get: () => true });
      document.dispatchEvent(ev);
    });
    expect(result.current.isArmed).toBe(false);
    expect(prevented).toBe(false);
  });

  it('Shift+X with modifier mismatch does NOT fire the unmodified "x" binding', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'X', shiftKey: true });
    });
    await Promise.resolve();
    expect(closeFetchMock).not.toHaveBeenCalled();

    expect(result.current.isArmed).toBe(false);
  });

  it.each([
    ['s', 'horizontal'],
    ['v', 'vertical'],
  ] as const)('Ctrl+B → %s fires the split-pane %s binding', async (key, orientation) => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key });
    });
    await waitFor(() =>
      expect(splitFetchMock).toHaveBeenCalledWith('/windows/wid-1/panes/pid-1/split', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ orientation }),
      }),
    );
  });

  it('Shift+S with modifier mismatch does NOT fire the unmodified "s" binding', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'S', shiftKey: true });
    });
    await Promise.resolve();
    expect(splitFetchMock).not.toHaveBeenCalled();
    expect(result.current.isArmed).toBe(false);
  });

  it('does not dispatch keydowns before fetch resolves (status === loading)', () => {
    configFetchMock.mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    expect(result.current.status).toBe('loading');
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(false);
    expect(closeFetchMock).not.toHaveBeenCalled();
  });

  it('settles into error when fetch rejects', async () => {
    configFetchMock.mockRejectedValue(new Error('boom'));
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('error'));
    expect(result.current.prefix).toBeNull();
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(false);
  });

  it('does not arm while the rename prompt gate is open', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    setRenamePromptOpen(true);
    try {
      act(() => {
        press({ key: 'b', ctrlKey: true });
      });
      expect(result.current.isArmed).toBe(false);
    } finally {
      setRenamePromptOpen(false);
    }
  });

  it('handles the new named-field schema by disabling dispatch', async () => {
    const newShapePayload = {
      bindings: {
        'close-pane': {
          key: 'd',
          modifiers: { ctrl: false, shift: true, alt: false, meta: true },
        },
        'focus-pane-left': {
          key: 'h',
          modifiers: { ctrl: false, shift: false, alt: false, meta: true },
        },
      },
    };
    configFetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => newShapePayload,
    } as Response);
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    expect(result.current.prefix).toBeNull();
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(false);
  });
});

const REPEAT_PAYLOAD = {
  prefix: {
    key: 'b',
    modifiers: { ctrl: true, shift: false, alt: false, meta: false },
    timeout_ms: 2000,
  },
  bindings: [
    {
      key: 'x',
      modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      action: { type: 'close-pane' },
      repeatable: false,
    },
    {
      key: 'ArrowRight',
      modifiers: { ctrl: true, shift: false, alt: false, meta: false },
      action: { type: 'resize-pane', direction: 'right' },
      repeatable: true,
    },
  ],
  repeat_timeout_ms: 500,
};

describe('usePrefixMode repeat sub-mode', () => {
  let resizeFetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

  beforeEach(() => {
    // Override the configFetchMock so the test uses REPEAT_PAYLOAD.
    configFetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => REPEAT_PAYLOAD,
    } as Response);
    resizeFetchMock = vi.fn<typeof fetch>().mockResolvedValue({
      ok: true,
      status: 204,
    } as Response);
    const previousFetch = globalThis.fetch;
    globalThis.fetch = ((url: RequestInfo | URL, init?: RequestInit) => {
      const path = typeof url === 'string' ? url : url.toString();
      if (path.endsWith('/resize')) return resizeFetchMock(url, init);
      return previousFetch(url, init);
    }) as typeof globalThis.fetch;
  });

  it('fires a repeatable binding and stays listening within repeat_timeout_ms', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
    });
    expect(result.current.isArmed).toBe(true);

    act(() => {
      press({ key: 'ArrowRight', ctrlKey: true });
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(1);

    // Second press WITHOUT re-arming, within the 500ms repeat window.
    act(() => {
      press({ key: 'ArrowRight', ctrlKey: true, repeat: true });
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(2);
  });

  it('exits repeat mode when a non-repeatable chord arrives and forwards it to the terminal', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));

    // Enter repeat mode via prefix + repeatable chord.
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'ArrowRight', ctrlKey: true });
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(1);

    // Press 'x' (close-pane, NOT repeatable). It must NOT increment
    // the repeatable handler AND must NOT be consumed (defaultPrevented
    // stays false so the terminal receives it).
    const xEvent = new KeyboardEvent('keydown', {
      key: 'x',
      bubbles: true,
      cancelable: true,
    });
    act(() => {
      document.dispatchEvent(xEvent);
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(1);
    expect(xEvent.defaultPrevented).toBe(false);
  });

  it('disarms after repeat_timeout_ms with no further keypresses', async () => {
    const { result } = renderHook(() => usePrefixMode(makeCtx()));
    await waitFor(() => expect(result.current.status).toBe('ready'));
    act(() => {
      press({ key: 'b', ctrlKey: true });
      press({ key: 'ArrowRight', ctrlKey: true });
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(1);

    // Advance fake timers past 500ms.
    act(() => {
      vi.advanceTimersByTime(600);
    });

    // A second repeatable press now requires re-arming via prefix.
    act(() => {
      press({ key: 'ArrowRight', ctrlKey: true });
    });
    await Promise.resolve();
    expect(resizeFetchMock).toHaveBeenCalledTimes(1);
  });
});
