// NOTE: msgpackr/index-no-eval is a CSP-safe build of msgpackr with the same API but no `new
// Function`. The package does not ship a separate types entry for this subpath, so we re-export
// the root declaration here.
declare module 'msgpackr/index-no-eval' {
  export * from 'msgpackr';
}
