import { beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
  // Module-load state is constant-folded; we reset by resetting modules
  // and the globals between tests.
  globalThis.__ozmuxPerfBuffer = undefined;
  globalThis.__OZMUX_PERF = undefined;
  vi.resetModules();
});

describe('markStage', () => {
  it('is a no-op when __OZMUX_PERF is unset', async () => {
    const { markStage } = await import('./marks');
    markStage(0, 'decode');
    expect(globalThis.__ozmuxPerfBuffer).toBeUndefined();
  });

  it('records entries when __OZMUX_PERF is true', async () => {
    globalThis.__OZMUX_PERF = true;
    const { markStage } = await import('./marks');
    markStage(1, 'decode');
    markStage(2, 'paint_end');
    expect(globalThis.__ozmuxPerfBuffer?.writeIndex).toBe(2);
    expect(globalThis.__ozmuxPerfBuffer?.seqs[0]).toBe(1);
    expect(globalThis.__ozmuxPerfBuffer?.stages[0]).toBe(1); // decode = 1
    expect(globalThis.__ozmuxPerfBuffer?.seqs[1]).toBe(2);
    expect(globalThis.__ozmuxPerfBuffer?.stages[1]).toBe(4); // paint_end = 4
  });

  it('circular FIFO eviction at cap', async () => {
    globalThis.__OZMUX_PERF = true;
    const { markStage } = await import('./marks');
    for (let i = 0; i < 1500; i++) markStage(i, 'ws_recv');
    expect(globalThis.__ozmuxPerfBuffer?.writeIndex).toBe(1500);
    // Index 0 in the ring was overwritten 1500 times; last write at iteration 1000 (mod 1000 = 0).
    expect(globalThis.__ozmuxPerfBuffer?.seqs[0]).toBe(1000);
  });
});
