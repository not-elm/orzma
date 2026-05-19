import { beforeEach, describe, expect, it, vi } from 'vitest';

beforeEach(() => {
  globalThis.__OZMUX_PERF = true;
  globalThis.__ozmuxPerfBuffer = undefined;
  vi.resetModules();
});

describe('generateReport', () => {
  it('returns empty report when buffer absent or empty', async () => {
    globalThis.__ozmuxPerfBuffer = undefined;
    const { generateReport } = await import('./report');
    const r = generateReport();
    expect(r.total_marks).toBe(0);
    expect(r.per_stage).toEqual({});
  });

  it('computes a percentile when both endpoints of a stage are recorded', async () => {
    const { markStageAt } = await import('./marks');
    const { generateReport } = await import('./report');
    markStageAt(1, 'ws_recv', 0);
    markStageAt(1, 'decode', 0.1);
    markStageAt(1, 'store_apply', 0.3);
    const r = generateReport();
    expect(r.per_stage.decode?.count).toBe(1);
    expect(r.per_stage.store_apply?.count).toBe(1);
  });

  it('handles wrap-around (writeIndex > cap)', async () => {
    const { markStageAt } = await import('./marks');
    const { generateReport } = await import('./report');
    for (let i = 0; i < 1500; i++) {
      markStageAt(i, 'ws_recv', i);
    }
    const r = generateReport();
    expect(r.wrapped).toBe(true);
    expect(r.total_marks).toBe(1500);
  });

  it('returns raw entries when includeRaw=true', async () => {
    const { markStageAt } = await import('./marks');
    const { generateReport } = await import('./report');
    markStageAt(7, 'ws_recv', 100);
    const r = generateReport({ includeRaw: true });
    expect(r.raw).toBeDefined();
    expect(r.raw?.[0]).toEqual({ seq: 7, stage: 'ws_recv', t: 100 });
  });

  it('computes server_to_ws_recv_us when produced_at_us is recorded', async () => {
    const { markStageAt, recordProducedAt } = await import('./marks');
    const { generateReport } = await import('./report');
    const seq = 42;
    markStageAt(seq, 'ws_recv', 100);
    recordProducedAt(seq, (performance.timeOrigin + 100) * 1000 - 50);
    const r = generateReport();
    expect(r.per_stage.server_to_ws_recv_us).toBeDefined();
    expect(r.per_stage.server_to_ws_recv_us?.count).toBe(1);
    expect(r.per_stage.server_to_ws_recv_us?.p50).toBeCloseTo(50, 5);
  });
});
