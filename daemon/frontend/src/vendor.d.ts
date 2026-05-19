// NOTE: msgpackr/index-no-eval is a CSP-safe build of msgpackr with the same API but no `new
// Function`. The package does not ship a separate types entry for this subpath, so we re-export
// the root declaration here.
declare module 'msgpackr/index-no-eval' {
  export * from 'msgpackr';
}

// Performance instrumentation globals — written by inline <script> in
// index.html when ?record-perf=1 is present in the URL. Read by
// terminal/perf/marks.ts at module load time (constant-folded).
declare var __OZMUX_PERF: boolean | undefined;
declare var __ozmuxPerfBuffer:
  | {
      writeIndex: number;
      seqs: Uint32Array;
      stages: Uint8Array;
      times: Float64Array;
      cap: number;
    }
  | undefined;
