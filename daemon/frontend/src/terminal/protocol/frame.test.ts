import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { decodeFrame } from './frame';

const FIXTURE_DIR = join(__dirname, '../../../../terminal/tests/fixtures/wire_msgpack');

function readBin(name: string): Uint8Array {
  return new Uint8Array(readFileSync(join(FIXTURE_DIR, `${name}.bin`)));
}
function readJson(name: string): unknown {
  return JSON.parse(readFileSync(join(FIXTURE_DIR, `${name}.json`), 'utf8'));
}

describe('decodeFrame', () => {
  it('decodes snapshot_minimal.bin to match snapshot_minimal.json', () => {
    const decoded = decodeFrame(readBin('snapshot_minimal'));
    const expected = readJson('snapshot_minimal');
    assert.deepStrictEqual(decoded, expected);
  });

  it('decodes delta_minimal.bin to match delta_minimal.json', () => {
    const decoded = decodeFrame(readBin('delta_minimal'));
    const expected = readJson('delta_minimal');
    assert.deepStrictEqual(decoded, expected);
  });

  it('does not BigInt-coerce u32 seq values', () => {
    const decoded = decodeFrame(readBin('delta_minimal')) as { seq: unknown };
    expect(typeof decoded.seq).toBe('number');
  });
});
