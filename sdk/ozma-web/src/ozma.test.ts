import { afterEach, beforeEach, describe, expect, expectTypeOf, it, vi } from 'vitest';
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
    const emit = vi.fn();
    g.ozma = { call, on, off, emit } as unknown as OzmaApi;

    const handler = (): void => {};
    await expect(ozma.call('ping', 'hi')).resolves.toBe('pong');
    ozma.on('tick', handler);
    ozma.off('tick', handler);
    ozma.emit('hello', { message: 'hi' });

    expect(call).toHaveBeenCalledWith('ping', 'hi');
    expect(on).toHaveBeenCalledWith('tick', handler);
    expect(off).toHaveBeenCalledWith('tick', handler);
    expect(emit).toHaveBeenCalledWith('hello', { message: 'hi' });
  });

  it('throws a descriptive error when the bridge is absent', () => {
    expect(() => ozma.call('ping')).toThrow(/window\.ozma is unavailable/);
    expect(() => ozma.on('tick', () => {})).toThrow(/window\.ozma is unavailable/);
    expect(() => ozma.off('tick', () => {})).toThrow(/window\.ozma is unavailable/);
    expect(() => ozma.emit('hello', {})).toThrow(/window\.ozma is unavailable/);
  });

  it('reports bridge availability', () => {
    expect(isOzmaAvailable()).toBe(false);
    g.ozma = { call: vi.fn(), on: vi.fn(), off: vi.fn(), emit: vi.fn() } as unknown as OzmaApi;
    expect(isOzmaAvailable()).toBe(true);
  });
});

describe('@ozma/web types (compile-time)', () => {
  beforeEach(() => {
    g.ozma = { call: vi.fn(), on: vi.fn(), off: vi.fn(), emit: vi.fn() } as unknown as OzmaApi;
  });

  it('infers the on() handler payload from a parameter annotation', () => {
    ozma.on('content', (p: { markdown: string }) => {
      expectTypeOf(p).toEqualTypeOf<{ markdown: string }>();
    });
  });

  it('accepts an explicit payload generic on on()', () => {
    ozma.on<{ x: number }>('e', (p) => {
      expectTypeOf(p).toEqualTypeOf<{ x: number }>();
    });
  });

  it('defaults the on() payload to unknown without a type', () => {
    ozma.on('content', (p) => {
      expectTypeOf(p).toBeUnknown();
    });
  });

  it('types emit payloads and call params/results', () => {
    ozma.emit('scrollState', { ratio: 0.5 });
    ozma.emit<{ ratio: number }>('scrollState', { ratio: 0.5 });
    void ozma.call<string, { path: string }>('save', { path: '/tmp' });
    expectTypeOf(ozma.call<string>('ready')).resolves.toEqualTypeOf<string>();
  });

  it('rejects a payload that mismatches an explicit generic', () => {
    // @ts-expect-error payload must satisfy the declared <{ ratio: number }> generic
    ozma.emit<{ ratio: number }>('scrollState', { ratio: 'not-a-number' });
  });
});
