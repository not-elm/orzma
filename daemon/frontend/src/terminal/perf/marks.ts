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
