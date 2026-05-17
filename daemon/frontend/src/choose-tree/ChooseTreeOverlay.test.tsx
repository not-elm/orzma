import { act, render, screen, waitFor } from '@testing-library/react';
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

  it('pressing l on a window row acts as confirm (I1)', async () => {
    const fetchMock = vi.fn().mockImplementation((url: string, init?: RequestInit) => {
      if (url === '/sessions/tree') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              sessions: [
                {
                  id: 'sid-a',
                  name: 'work',
                  active_window: 'wid-a0',
                  windows: [{ id: 'wid-a0', name: 'build', index: 0 }],
                },
              ],
            }),
            { status: 200 },
          ),
        );
      }
      if (typeof url === 'string' && url.startsWith('/windows/') && init?.method === 'POST') {
        return Promise.resolve(new Response(null, { status: 200 }));
      }
      return Promise.resolve(new Response(null, { status: 404 }));
    });
    globalThis.fetch = fetchMock as typeof globalThis.fetch;
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
    // Wait for the tree-reloaded state update so the cursor is on the window row.
    await waitFor(() =>
      expect(dialog).toHaveAttribute('aria-activedescendant', 'window:sid-a:wid-a0'),
    );
    act(() => {
      dialog.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'l', bubbles: true, cancelable: true }),
      );
    });
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(fetchMock).toHaveBeenCalledWith('/windows/wid-a0/select', { method: 'POST' });
  });

  it('aria-activedescendant on the dialog points at the cursor row (I2)', async () => {
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
    // NOTE: aria-activedescendant is set after the tree-reloaded dispatch resolves;
    // waitFor lets the effect flush before asserting.
    await waitFor(() =>
      expect(dialog).toHaveAttribute('aria-activedescendant', 'window:sid-a:wid-a0'),
    );
  });

  it('closes immediately when the tree is empty (M4)', async () => {
    globalThis.fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ sessions: [] }), { status: 200 }),
      ) as typeof globalThis.fetch;
    const onClose = vi.fn();
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    render(
      <ChooseTreeOverlay
        onClose={onClose}
        attachedSessionId={null}
        setAttachedSession={() => {}}
      />,
    );
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(warn).toHaveBeenCalledWith('choose-tree: no sessions available; closing picker');
  });
});
