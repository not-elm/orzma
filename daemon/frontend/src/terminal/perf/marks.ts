/**
 * Performance-mark ring buffer for the WS → render pipeline.
 *
 * PR-A: push path only (no read API). PR-B adds `report.ts` with
 * `__OZMUX_PERF_REPORT()` for percentile read-out.
 *
 * markStage is a 1-branch no-op when `window.__OZMUX_PERF` is unset at
 * module load (constant-folded). When enabled, writes into a fixed-size
 * circular FIFO of typed arrays — zero per-call allocation, O(1) overwrite.
 */

const PERF_ENABLED = globalThis.__OZMUX_PERF === true;
const BUFFER_CAP = 1000;

const STAGE_IDS = {
  ws_recv: 0,
  decode: 1,
  store_apply: 2,
  commit_end: 3,
  paint_end: 4,
} as const;

/** Pipeline stage identifier for a perf mark entry. */
export type Stage = keyof typeof STAGE_IDS;

if (PERF_ENABLED && globalThis.__ozmuxPerfBuffer === undefined) {
  globalThis.__ozmuxPerfBuffer = {
    writeIndex: 0,
    seqs: new Uint32Array(BUFFER_CAP),
    stages: new Uint8Array(BUFFER_CAP),
    times: new Float64Array(BUFFER_CAP),
    cap: BUFFER_CAP,
  };
}

/** Records a perf event into the circular ring buffer. */
export function markStage(seq: number, stage: Stage): void {
  if (!PERF_ENABLED) return;
  // biome-ignore lint/style/noNonNullAssertion: buffer is guaranteed non-null by the module-init block that runs when PERF_ENABLED is true
  const buf = globalThis.__ozmuxPerfBuffer!;
  const i = buf.writeIndex % buf.cap;
  buf.seqs[i] = seq;
  buf.stages[i] = STAGE_IDS[stage];
  buf.times[i] = performance.now();
  buf.writeIndex += 1;
}

/**
 * Records a stage at an explicit prior timestamp. Used when the caller knows
 * a stage happened in the past (e.g. ws_recv before the seq was decoded).
 */
export function markStageAt(seq: number, stage: Stage, t: number): void {
  if (!PERF_ENABLED) return;
  // biome-ignore lint/style/noNonNullAssertion: buffer is guaranteed non-null by the module-init block that runs when PERF_ENABLED is true
  const buf = globalThis.__ozmuxPerfBuffer!;
  const i = buf.writeIndex % buf.cap;
  buf.seqs[i] = seq;
  buf.stages[i] = STAGE_IDS[stage];
  buf.times[i] = t;
  buf.writeIndex += 1;
}

/**
 * Returns the recorded timestamp for `(seq, stage)`, or `undefined` if no
 * such mark exists in the active buffer window.
 */
export function getMarkTime(seq: number, stage: Stage): number | undefined {
  if (!PERF_ENABLED) return undefined;
  // biome-ignore lint/style/noNonNullAssertion: buffer is guaranteed non-null by the module-init block that runs when PERF_ENABLED is true
  const buf = globalThis.__ozmuxPerfBuffer!;
  const stageId = STAGE_IDS[stage];
  const lo = Math.max(0, buf.writeIndex - buf.cap);
  for (let i = lo; i < buf.writeIndex; i++) {
    const slot = i % buf.cap;
    if (buf.seqs[slot] === seq && buf.stages[slot] === stageId) {
      return buf.times[slot];
    }
  }
  return undefined;
}

const producedAtBySeq = new Map<number, number>();

/**
 * Records the server-side `produced_at_us` for a given seq, used by
 * report.ts to compute end-to-end latency via the `server_to_ws_recv_us`
 * synth-stage.
 */
export function recordProducedAt(seq: number, producedAtUs: number): void {
  if (!PERF_ENABLED) return;
  producedAtBySeq.set(seq, producedAtUs);
  if (producedAtBySeq.size > 5000) {
    const first = producedAtBySeq.keys().next().value;
    if (first !== undefined) producedAtBySeq.delete(first);
  }
}

/**
 * Returns the recorded `produced_at_us` for `seq`, or undefined if not
 * captured (frame had no field, perf disabled, or evicted).
 */
export function getProducedAt(seq: number): number | undefined {
  return producedAtBySeq.get(seq);
}

/**
 * Resets the ring buffer write index to zero. Exposed for tests that need
 * to clear the buffer without reloading the module.
 */
export const __test_only_resetPerfBuffer = (): void => {
  if (globalThis.__ozmuxPerfBuffer !== undefined) {
    globalThis.__ozmuxPerfBuffer.writeIndex = 0;
  }
  producedAtBySeq.clear();
};
