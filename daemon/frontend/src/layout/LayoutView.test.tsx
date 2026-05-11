import { render, waitFor } from '@testing-library/react';
import { Server } from 'mock-socket';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LayoutView } from './LayoutView';
import type { WindowView } from './types';

const WID = 'wid-test';
const SID = 'sid-test';
const URL = `ws://${location.host}/windows/${WID}/events`;

const origFetch = globalThis.fetch;
let server: Server;

beforeEach(() => {
  globalThis.fetch = vi.fn().mockImplementation((url: string) => {
    if (url === '/sessions')
      return Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ sessions: [{ id: SID }] }),
      } as Response);
    if (url === `/sessions/${SID}`)
      return Promise.resolve({
        ok: true,
        json: () => Promise.resolve({ windows: [WID], active_window: WID }),
      } as Response);
    return Promise.reject(new Error(`unexpected fetch ${url}`));
  }) as typeof globalThis.fetch;
  server = new Server(URL);
});

afterEach(() => {
  server.stop();
  globalThis.fetch = origFetch;
});

function fakeView(overrides: Partial<WindowView> = {}): WindowView {
  return {
    id: WID,
    name: 'main',
    root_cell: 'cid-root',
    active_pane: 'pid-1',
    panes: [
      { id: 'pid-1', active_activity: 'aid-1', activities: [{ id: 'aid-1', kind: 'terminal' }] },
    ],
    layout_schema_version: 1,
    layout: {
      type: 'root',
      cell_id: 'cid-root',
      child: { type: 'pane', cell_id: 'cid-pane-1', pane_id: 'pid-1' },
    },
    ...overrides,
  };
}

async function withView(view: WindowView): Promise<HTMLElement> {
  server.on('connection', (sock) => {
    sock.send(JSON.stringify(view));
  });
  const { container } = render(<LayoutView />);
  await waitFor(() => {
    expect(container.querySelector('[data-active]')).not.toBeNull();
  });
  return container;
}

describe('<LayoutView>', () => {
  it('marks the active pane wrapper with data-active=true', async () => {
    const container = await withView(
      fakeView({
        active_pane: 'pid-ext',
        panes: [
          {
            id: 'pid-ext',
            active_activity: 'aid-ext',
            activities: [{ id: 'aid-ext', kind: 'extension', iframe_url: '/x' }],
          },
        ],
        layout: {
          type: 'root',
          cell_id: 'cid-root',
          child: { type: 'pane', cell_id: 'cid-pane', pane_id: 'pid-ext' },
        },
      }),
    );
    const wrapper = container.querySelector('[data-active="true"]') as HTMLElement;
    expect(wrapper).not.toBeNull();
  });

  it('marks a non-active pane wrapper with data-active=false', async () => {
    const view: WindowView = {
      id: WID,
      name: 'main',
      root_cell: 'cid-root',
      active_pane: 'pid-a',
      layout_schema_version: 1,
      panes: [
        { id: 'pid-a', active_activity: 'aid-a', activities: [{ id: 'aid-a', kind: 'extension', iframe_url: '/a' }] },
        { id: 'pid-b', active_activity: 'aid-b', activities: [{ id: 'aid-b', kind: 'extension', iframe_url: '/b' }] },
      ],
      layout: {
        type: 'root',
        cell_id: 'cid-root',
        child: {
          type: 'split',
          cell_id: 'cid-s',
          orientation: 'horizontal',
          split_ratio: 0.5,
          lhs: { type: 'pane', cell_id: 'cid-a', pane_id: 'pid-a' },
          rhs: { type: 'pane', cell_id: 'cid-b', pane_id: 'pid-b' },
        },
      },
    };
    const container = await withView(view);
    const wrappers = Array.from(container.querySelectorAll('[data-active]')) as HTMLElement[];
    const inactive = wrappers.find((w) => w.getAttribute('data-active') === 'false');
    expect(inactive).toBeDefined();
  });

  it('renders an iframe for an active extension pane', async () => {
    const container = await withView(
      fakeView({
        active_pane: 'pid-ext',
        panes: [
          {
            id: 'pid-ext',
            active_activity: 'aid-ext',
            activities: [
              { id: 'aid-ext', kind: 'extension', iframe_url: '/activities/aid-ext/iframe/index.html' },
            ],
          },
        ],
        layout: {
          type: 'root',
          cell_id: 'cid-root',
          child: { type: 'pane', cell_id: 'cid-pane', pane_id: 'pid-ext' },
        },
      }),
    );
    const iframe = container.querySelector('iframe');
    expect(iframe).not.toBeNull();
    expect(iframe!.getAttribute('src')).toBe('/activities/aid-ext/iframe/index.html');
  });

  it('falls back to PanePlaceholder for an extension activity without iframe_url', async () => {
    const container = await withView(
      fakeView({
        active_pane: 'pid-ext',
        panes: [
          {
            id: 'pid-ext',
            active_activity: 'aid-ext',
            activities: [{ id: 'aid-ext', kind: 'extension' }],
          },
        ],
        layout: {
          type: 'root',
          cell_id: 'cid-root',
          child: { type: 'pane', cell_id: 'cid-pane', pane_id: 'pid-ext' },
        },
      }),
    );
    expect(container.querySelector('iframe')).toBeNull();
    expect(container.textContent).toContain('pid-ext');
  });

  it('renders UnknownLayoutNode for unrecognized layout type', async () => {
    const container = await withView({
      id: WID,
      name: 'main',
      root_cell: 'cid-root',
      active_pane: 'pid-1',
      layout_schema_version: 1,
      panes: [
        {
          id: 'pid-1',
          active_activity: 'aid-1',
          activities: [{ id: 'aid-1', kind: 'extension', iframe_url: '/x' }],
        },
      ],
      // The layout has an unknown node type next to a known pane so withView's
      // data-active probe still finds something and the unknown is rendered.
      layout: {
        type: 'root',
        cell_id: 'cid-root',
        child: {
          type: 'split',
          cell_id: 'cid-s',
          orientation: 'horizontal',
          split_ratio: 0.5,
          lhs: { type: 'pane', cell_id: 'cid-a', pane_id: 'pid-1' },
          rhs: { type: 'mystery', cell_id: 'cid-m' } as never,
        },
      },
    });
    expect(container.textContent).toMatch(/Unknown layout node type/);
  });

  it('renders absolute-positioned wrappers with percentage sizes', async () => {
    const view: WindowView = {
      id: WID,
      name: 'main',
      root_cell: 'cid-root',
      active_pane: 'pid-a',
      layout_schema_version: 1,
      panes: [
        { id: 'pid-a', active_activity: 'aid-a', activities: [{ id: 'aid-a', kind: 'extension', iframe_url: '/a' }] },
        { id: 'pid-b', active_activity: 'aid-b', activities: [{ id: 'aid-b', kind: 'extension', iframe_url: '/b' }] },
      ],
      layout: {
        type: 'root',
        cell_id: 'cid-root',
        child: {
          type: 'split',
          cell_id: 'cid-s',
          orientation: 'horizontal',
          split_ratio: 0.7,
          lhs: { type: 'pane', cell_id: 'cid-a', pane_id: 'pid-a' },
          rhs: { type: 'pane', cell_id: 'cid-b', pane_id: 'pid-b' },
        },
      },
    };
    const container = await withView(view);
    const wrappers = Array.from(container.querySelectorAll('[data-active]')) as HTMLElement[];
    expect(wrappers).toHaveLength(2);
    for (const w of wrappers) {
      expect(w.classList.contains('absolute')).toBe(true);
      expect(w.style.left.endsWith('%')).toBe(true);
      expect(w.style.width.endsWith('%')).toBe(true);
    }
  });
});
