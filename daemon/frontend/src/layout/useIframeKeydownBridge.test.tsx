import { render } from '@testing-library/react';
import { useEffect, useRef } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as prefix from '../shortcuts/usePrefixMode';
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
});
