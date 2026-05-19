#!/usr/bin/env tsx
/**
 * Compare two perf-baseline bundles and produce a markdown diff.
 * Usage: tsx scripts/perf-compare.ts <baseline-dir> <target-dir>
 */
import * as fs from 'node:fs';

const [, , baseline, target] = process.argv;
if (!baseline || !target) {
  console.error('usage: tsx scripts/perf-compare.ts <baseline-dir> <target-dir>');
  process.exit(2);
}

interface StageStats {
  count: number;
  min: number;
  max: number;
  p50: number;
  p95: number;
  p99: number;
}

interface PerfReport {
  per_stage: Record<string, StageStats>;
}

const a: PerfReport = JSON.parse(fs.readFileSync(`${baseline}/perf-report.json`, 'utf8'));
const b: PerfReport = JSON.parse(fs.readFileSync(`${target}/perf-report.json`, 'utf8'));

console.log(`# perf compare: ${baseline} vs ${target}\n`);
console.log('| Stage | p50 base | p50 target | Δ p50 | p95 base | p95 target | Δ p95 |');
console.log('|---|---|---|---|---|---|---|');

for (const stage of Object.keys(a.per_stage)) {
  const aS = a.per_stage[stage];
  const bS = b.per_stage[stage];
  if (!bS) continue;
  const d50 = (bS.p50 - aS.p50).toFixed(3);
  const d95 = (bS.p95 - aS.p95).toFixed(3);
  console.log(
    `| ${stage} | ${aS.p50.toFixed(3)} | ${bS.p50.toFixed(3)} | ${d50} | ` +
      `${aS.p95.toFixed(3)} | ${bS.p95.toFixed(3)} | ${d95} |`,
  );
}
