import { afterEach, describe, expect, it, vi } from 'vitest';
import { isOzmaAvailable, type OzmaApi, ozma } from './ozma.ts';

const g = globalThis as typeof globalThis & { ozma?: OzmaApi };

afterEach(() => {
  g.ozma = undefined;
});

describe('@ozma/web client', () => {
  it('delegates call/on/off to the injected bridge', async () => {
    const call = vi.fn(async () => 'pong');
    const on = vi.fn();
    const off = vi.fn();
    g.ozma = { call, on, off } as unknown as OzmaApi;

    const handler = (): void => {};
    await expect(ozma.call('ping', 'hi')).resolves.toBe('pong');
    ozma.on('tick', handler);
    ozma.off('tick', handler);

    expect(call).toHaveBeenCalledWith('ping', 'hi');
    expect(on).toHaveBeenCalledWith('tick', handler);
    expect(off).toHaveBeenCalledWith('tick', handler);
  });

  it('throws a descriptive error when the bridge is absent', () => {
    expect(() => ozma.call('ping')).toThrow(/window\.ozma is unavailable/);
    expect(() => ozma.on('tick', () => {})).toThrow(/window\.ozma is unavailable/);
    expect(() => ozma.off('tick', () => {})).toThrow(/window\.ozma is unavailable/);
  });

  it('reports bridge availability', () => {
    expect(isOzmaAvailable()).toBe(false);
    g.ozma = { call: vi.fn(), on: vi.fn(), off: vi.fn() } as unknown as OzmaApi;
    expect(isOzmaAvailable()).toBe(true);
  });
});
