import { afterEach, beforeEach, describe, expect, expectTypeOf, it, vi } from 'vitest';
import { isOrzmaAvailable, type OrzmaApi, orzma } from './orzma.ts';

const g = globalThis as typeof globalThis & { orzma?: OrzmaApi };

afterEach(() => {
  g.orzma = undefined;
});

describe('@orzma/web client', () => {
  it('delegates call/on/off to the injected bridge', async () => {
    const call = vi.fn(async () => 'pong');
    const on = vi.fn();
    const off = vi.fn();
    const emit = vi.fn();
    g.orzma = { call, on, off, emit } as unknown as OrzmaApi;

    const handler = (): void => {};
    await expect(orzma.call('ping', 'hi')).resolves.toBe('pong');
    orzma.on('tick', handler);
    orzma.off('tick', handler);
    orzma.emit('hello', { message: 'hi' });

    expect(call).toHaveBeenCalledWith('ping', 'hi');
    expect(on).toHaveBeenCalledWith('tick', handler);
    expect(off).toHaveBeenCalledWith('tick', handler);
    expect(emit).toHaveBeenCalledWith('hello', { message: 'hi' });
  });

  it('throws a descriptive error when the bridge is absent', () => {
    expect(() => orzma.call('ping')).toThrow(/window\.orzma is unavailable/);
    expect(() => orzma.on('tick', () => {})).toThrow(/window\.orzma is unavailable/);
    expect(() => orzma.off('tick', () => {})).toThrow(/window\.orzma is unavailable/);
    expect(() => orzma.emit('hello', {})).toThrow(/window\.orzma is unavailable/);
  });

  it('reports bridge availability', () => {
    expect(isOrzmaAvailable()).toBe(false);
    g.orzma = { call: vi.fn(), on: vi.fn(), off: vi.fn(), emit: vi.fn() } as unknown as OrzmaApi;
    expect(isOrzmaAvailable()).toBe(true);
  });
});

describe('@orzma/web types (compile-time)', () => {
  beforeEach(() => {
    g.orzma = { call: vi.fn(), on: vi.fn(), off: vi.fn(), emit: vi.fn() } as unknown as OrzmaApi;
  });

  it('infers the on() handler payload from a parameter annotation', () => {
    orzma.on('content', (p: { markdown: string }) => {
      expectTypeOf(p).toEqualTypeOf<{ markdown: string }>();
    });
  });

  it('accepts an explicit payload generic on on()', () => {
    orzma.on<{ x: number }>('e', (p) => {
      expectTypeOf(p).toEqualTypeOf<{ x: number }>();
    });
  });

  it('defaults the on() payload to unknown without a type', () => {
    orzma.on('content', (p) => {
      expectTypeOf(p).toBeUnknown();
    });
  });

  it('types emit payloads and call params/results', () => {
    orzma.emit('scrollState', { ratio: 0.5 });
    orzma.emit<{ ratio: number }>('scrollState', { ratio: 0.5 });
    void orzma.call<string, { path: string }>('save', { path: '/tmp' });
    expectTypeOf(orzma.call<string>('ready')).resolves.toEqualTypeOf<string>();
  });

  it('rejects a payload that mismatches an explicit generic', () => {
    // @ts-expect-error payload must satisfy the declared <{ ratio: number }> generic
    orzma.emit<{ ratio: number }>('scrollState', { ratio: 'not-a-number' });
  });
});
