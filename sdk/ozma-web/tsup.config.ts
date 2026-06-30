import { defineConfig } from 'tsup'

/** Tsup build configuration for @ozma/web. */
export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm'],
  dts: {
    resolve: true,
    // NOTE: tsup's DTS bundler (rollup-plugin-dts) internally sets baseUrl, which
    // TypeScript 6.0 treats as a deprecation error. ignoreDeprecations suppresses it
    // so the DTS build can proceed; this does not affect type-checking via tsc.
    compilerOptions: { ignoreDeprecations: '6.0' },
  },
  clean: true,
})
