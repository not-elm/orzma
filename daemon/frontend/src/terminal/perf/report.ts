/**
 * `__OZMUX_PERF_REPORT()` implementation. Walks the typed-array ring in
 * chronological order, joins marks by seq, and computes per-stage
 * transition percentiles. When `produced_at_us` is available on the
 * frame, an extra synthetic stage `server_to_ws_recv_us` is computed.
 */

import type { Stage } from './marks';
import { getProducedAt } from './marks';

/** Percentile statistics for a single pipeline stage transition. */
export interface StageStats {
  count: number;
  min: number;
  max: number;
  p50: number;
  p95: number;
  p99: number;
}

/** All possible stage keys in a perf report, including the synth stage. */
export type ExtendedStage = Stage | 'server_to_ws_recv_us';

/** Full perf report returned by `window.__OZMUX_PERF_REPORT()`. */
export interface PerfReport {
  total_marks: number;
  buffer_capacity: number;
  wrapped: boolean;
  per_stage: Partial<Record<ExtendedStage, StageStats>>;
  raw?: ReadonlyArray<{ seq: number; stage: Stage; t: number }>;
}

const STAGE_NAMES: ReadonlyArray<Stage> = [
  'ws_recv',
  'decode',
  'store_apply',
  'commit_end',
  'paint_end',
];

function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = (sorted.length - 1) * p;
  const lo = Math.floor(idx);
  const hi = Math.ceil(idx);
  if (lo === hi) return sorted[lo];
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (idx - lo);
}

function statsFromSamples(samples: number[]): StageStats {
  const sorted = [...samples].sort((a, b) => a - b);
  return {
    count: sorted.length,
    min: sorted[0] ?? 0,
    max: sorted[sorted.length - 1] ?? 0,
    p50: percentile(sorted, 0.5),
    p95: percentile(sorted, 0.95),
    p99: percentile(sorted, 0.99),
  };
}

/**
 * Generates a perf report from the in-memory ring. Walks the ring in
 * chronological order, joins marks by seq, and computes per-stage
 * transition percentiles (each stage = ms from the previous stage to
 * this one). When `produced_at_us` has been recorded for a frame, also
 * includes a `server_to_ws_recv_us` synthetic stage measuring end-to-end
 * server-to-client latency.
 */
export function generateReport(opts: { includeRaw?: boolean } = {}): PerfReport {
  const buf = globalThis.__ozmuxPerfBuffer;
  if (!buf) {
    return {
      total_marks: 0,
      buffer_capacity: 0,
      wrapped: false,
      per_stage: {},
    };
  }
  const cap = buf.cap;
  const total = buf.writeIndex;
  const wrapped = total > cap;
  const lo = Math.max(0, total - cap);

  const raw: { seq: number; stage: Stage; t: number }[] = [];
  for (let i = lo; i < total; i++) {
    const slot = i % cap;
    raw.push({
      seq: buf.seqs[slot],
      stage: STAGE_NAMES[buf.stages[slot]],
      t: buf.times[slot],
    });
  }

  const bySeqStage = new Map<number, Partial<Record<Stage, number>>>();
  for (const m of raw) {
    let entry = bySeqStage.get(m.seq);
    if (!entry) {
      entry = {};
      bySeqStage.set(m.seq, entry);
    }
    entry[m.stage] = m.t;
  }

  const samples: Partial<Record<Stage, number[]>> = {};
  for (let i = 1; i < STAGE_NAMES.length; i++) {
    const prev = STAGE_NAMES[i - 1];
    const cur = STAGE_NAMES[i];
    const arr: number[] = [];
    for (const stages of bySeqStage.values()) {
      const a = stages[prev];
      const b = stages[cur];
      if (typeof a === 'number' && typeof b === 'number') {
        arr.push(b - a);
      }
    }
    if (arr.length > 0) samples[cur] = arr;
  }

  const per_stage: Partial<Record<ExtendedStage, StageStats>> = {};
  for (const [stage, arr] of Object.entries(samples)) {
    if (arr) per_stage[stage as Stage] = statsFromSamples(arr);
  }

  const synthSamples: number[] = [];
  for (const [seq, stages] of bySeqStage.entries()) {
    const wsRecv = stages.ws_recv;
    const producedAtUs = getProducedAt(seq);
    if (typeof wsRecv === 'number' && producedAtUs !== undefined) {
      const wsRecvWallUs = (performance.timeOrigin + wsRecv) * 1000;
      synthSamples.push(wsRecvWallUs - producedAtUs);
    }
  }
  if (synthSamples.length > 0) {
    per_stage.server_to_ws_recv_us = statsFromSamples(synthSamples);
  }

  const report: PerfReport = {
    total_marks: total,
    buffer_capacity: cap,
    wrapped,
    per_stage,
  };
  if (opts.includeRaw) report.raw = raw;
  return report;
}

/**
 * Installs `window.__OZMUX_PERF_REPORT` if `globalThis.__OZMUX_PERF` is
 * true. Called once at app boot in `main.tsx`; subsequent calls are no-ops.
 */
export function installPerfReport(): void {
  if (globalThis.__OZMUX_PERF !== true) return;
  (window as unknown as { __OZMUX_PERF_REPORT: typeof generateReport }).__OZMUX_PERF_REPORT =
    generateReport;
}
