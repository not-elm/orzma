#!/usr/bin/env tsx
import { Unpackr } from 'msgpackr';
import { strict as assert } from 'node:assert';
import { readFileSync, readdirSync } from 'node:fs';
import { basename, join } from 'node:path';

const dir = process.argv[2];
if (!dir) {
  console.error('usage: tsx tools/verify-msgpack.ts <fixture-dir>');
  process.exit(2);
}

const unpackr = new Unpackr({
  useRecords: false,
  mapsAsObjects: true,
  int64AsType: 'number',
});

// JSON-text fixtures whose `.bin` contains JSON, not msgpack.
const TEXT_FIXTURES = new Set(['hello']);

let failed = 0;
const bins = readdirSync(dir).filter((f) => f.endsWith('.bin')).sort();

for (const f of bins) {
  const base = basename(f, '.bin');
  const expectedPath = join(dir, base + '.json');
  let expected: unknown;
  try {
    expected = JSON.parse(readFileSync(expectedPath, 'utf8'));
  } catch (e) {
    console.error(`✗ ${f}: cannot read ${expectedPath}: ${(e as Error).message}`);
    failed++;
    continue;
  }

  let actual: unknown;
  if (TEXT_FIXTURES.has(base)) {
    actual = JSON.parse(readFileSync(join(dir, f), 'utf8'));
  } else {
    actual = unpackr.unpack(readFileSync(join(dir, f)));
  }

  try {
    assert.deepStrictEqual(actual, expected);
    console.log(`✓ ${f}`);
  } catch (e) {
    console.error(`✗ ${f}:`);
    console.error((e as Error).message);
    failed++;
  }
}

if (failed === 0) {
  console.log(`\n${bins.length} fixtures pass`);
  process.exit(0);
} else {
  console.error(`\n${failed} of ${bins.length} fixtures failed`);
  process.exit(1);
}
