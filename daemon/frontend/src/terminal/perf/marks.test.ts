import { beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
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

describe('markStageAt + getMarkTime', () => {
  it('records and recalls explicit timestamps', async () => {
    globalThis.__OZMUX_PERF = true;
    const { markStageAt, getMarkTime } = await import('./marks');
    markStageAt(42, 'ws_recv', 100.5);
    expect(getMarkTime(42, 'ws_recv')).toBe(100.5);
  });

  it('returns undefined for unknown seq', async () => {
    globalThis.__OZMUX_PERF = true;
    const { getMarkTime } = await import('./marks');
    expect(getMarkTime(999, 'paint_end')).toBeUndefined();
  });

  it('is a no-op when __OZMUX_PERF is unset', async () => {
    const { markStageAt, getMarkTime } = await import('./marks');
    markStageAt(1, 'decode', 50.0);
    expect(getMarkTime(1, 'decode')).toBeUndefined();
  });

  it('__test_only_resetPerfBuffer clears the write index', async () => {
    globalThis.__OZMUX_PERF = true;
    const { markStageAt, getMarkTime, __test_only_resetPerfBuffer } = await import('./marks');
    markStageAt(7, 'decode', 77.7);
    expect(getMarkTime(7, 'decode')).toBe(77.7);
    __test_only_resetPerfBuffer();
    expect(getMarkTime(7, 'decode')).toBeUndefined();
  });
});
