import { act, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ChooseTreeOverlay } from './ChooseTreeOverlay';

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
  vi.restoreAllMocks();
});

function mockTree() {
  globalThis.fetch = vi.fn().mockImplementation((url: string) => {
    if (url === '/sessions/tree') {
      return Promise.resolve(
        new Response(
          JSON.stringify({
            sessions: [
              {
                id: 'sid-a',
                name: 'work',
                active_window: 'wid-a0',
                windows: [
                  { id: 'wid-a0', name: 'build', index: 0 },
                  { id: 'wid-a1', name: 'main', index: 1 },
                ],
              },
            ],
          }),
          { status: 200 },
        ),
      );
    }
    return Promise.resolve(new Response(null, { status: 200 }));
  }) as typeof globalThis.fetch;
}

describe('ChooseTreeOverlay', () => {
  it('focuses the overlay root on mount', async () => {
    mockTree();
    render(
      <ChooseTreeOverlay
        onClose={() => {}}
        attachedSessionId="sid-a"
        setAttachedSession={() => {}}
      />,
    );
    await screen.findByRole('tree');
    const dialog = screen.getByRole('dialog');
    expect(document.activeElement).toBe(dialog);
  });

  it('Escape calls onClose', async () => {
    mockTree();
    const onClose = vi.fn();
    render(
      <ChooseTreeOverlay
        onClose={onClose}
        attachedSessionId="sid-a"
        setAttachedSession={() => {}}
      />,
    );
    await screen.findByRole('tree');
    const dialog = screen.getByRole('dialog');
    act(() => {
      dialog.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }),
      );
    });
    expect(onClose).toHaveBeenCalled();
  });

  it('does not preventDefault on keys it does not consume', async () => {
    mockTree();
    render(
      <ChooseTreeOverlay
        onClose={() => {}}
        attachedSessionId="sid-a"
        setAttachedSession={() => {}}
      />,
    );
    await screen.findByRole('tree');
    const dialog = screen.getByRole('dialog');
    const ev = new KeyboardEvent('keydown', { key: 'F12', bubbles: true, cancelable: true });
    let prevented = false;
    ev.preventDefault = () => {
      prevented = true;
    };
    act(() => {
      dialog.dispatchEvent(ev);
    });
    expect(prevented).toBe(false);
  });

  it('IME-composing Enter does not call onClose', async () => {
    mockTree();
    const onClose = vi.fn();
    render(
      <ChooseTreeOverlay
        onClose={onClose}
        attachedSessionId="sid-a"
        setAttachedSession={() => {}}
      />,
    );
    await screen.findByRole('tree');
    const dialog = screen.getByRole('dialog');
    const ev = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    Object.defineProperty(ev, 'isComposing', { value: true });
    act(() => {
      dialog.dispatchEvent(ev);
    });
    expect(onClose).not.toHaveBeenCalled();
  });
});
