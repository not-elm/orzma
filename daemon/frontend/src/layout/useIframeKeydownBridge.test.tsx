import { render } from '@testing-library/react';
import { useEffect, useRef } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ShortcutContext } from '../shortcuts/actionDispatch';
import * as prefix from '../shortcuts/usePrefixMode';
import { usePrefixMode } from '../shortcuts/usePrefixMode';
import { useIframeKeydownBridge } from './useIframeKeydownBridge';

afterEach(() => {
  vi.restoreAllMocks();
});

function Probe() {
  const ref = useRef<HTMLIFrameElement>(null);
  useIframeKeydownBridge(ref);
  // Fire load synchronously after mount so the test can observe attach
  useEffect(() => {
    ref.current?.dispatchEvent(new Event('load'));
  }, []);
  return <iframe ref={ref} src="about:blank" title="probe" />;
}

describe('useIframeKeydownBridge', () => {
  it('attaches the prefix dispatcher to the iframe contentDocument on load', () => {
    const attachSpy = vi.spyOn(prefix, 'attachKeydownTarget');
    render(<Probe />);
    // attached at least once with a Document target
    // Note: jsdom iframes use a cross-realm Document so instanceof fails; check by constructor name.
    const calls = attachSpy.mock.calls.filter(
      ([t]) => t instanceof Document || (t as EventTarget).constructor?.name === 'Document',
    );
    expect(calls.length).toBeGreaterThanOrEqual(1);
  });

  it('detaches on unmount', () => {
    const detachSpy = vi.spyOn(prefix, 'detachKeydownTarget');
    const { unmount } = render(<Probe />);
    unmount();
    expect(detachSpy).toHaveBeenCalled();
  });

  it('does not dispatch armed-mode bindings when shortcuts have not loaded', async () => {
    const origFetch = globalThis.fetch;
    const closeFetchMock = vi.fn().mockResolvedValue({ ok: true, status: 204 } as Response);
    // /configs/shortcuts hangs forever so usePrefixMode's `shared` stays null.
    const configFetchMock = vi.fn<typeof fetch>(() => new Promise<Response>(() => {}));
    globalThis.fetch = ((url: RequestInfo | URL, init?: RequestInit) => {
      const path = typeof url === 'string' ? url : url.toString();
      if (path === '/configs/shortcuts') return configFetchMock(url, init);
      return closeFetchMock(url, init);
    }) as typeof globalThis.fetch;

    try {
      const ctx: ShortcutContext = {
        activeWindow: () => 'wid-1',
        activePane: () => 'pid-1',
      };
      const Holder = () => {
        usePrefixMode(ctx);
        const ref = useRef<HTMLIFrameElement>(null);
        useIframeKeydownBridge(ref);
        useEffect(() => {
          ref.current?.dispatchEvent(new Event('load'));
        }, []);
        return <iframe ref={ref} src="about:blank" title="probe-no-load" />;
      };

      const { container } = render(<Holder />);
      const iframe = container.querySelector('iframe');
      const doc = iframe?.contentDocument;
      expect(doc).not.toBeNull();
      doc?.dispatchEvent(new KeyboardEvent('keydown', { key: 'b', ctrlKey: true, bubbles: true }));
      // Microtask flush before asserting no fetch happened.
      await Promise.resolve();
      expect(closeFetchMock).not.toHaveBeenCalled();
    } finally {
      globalThis.fetch = origFetch;
    }
  });
});
