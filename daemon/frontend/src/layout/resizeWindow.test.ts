import { afterEach, describe, expect, it, vi } from 'vitest';
import { resizeWindow } from './resizeWindow';
import type { WindowId } from './types';

afterEach(() => vi.restoreAllMocks());

describe('resizeWindow', () => {
  it('PATCHes cols and rows', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchMock);
    await resizeWindow('w1' as WindowId, 120, 40);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/windows/w1/dimensions');
    expect(init?.method).toBe('PATCH');
    expect(JSON.parse(init?.body as string)).toEqual({ cols: 120, rows: 40 });
  });
});
