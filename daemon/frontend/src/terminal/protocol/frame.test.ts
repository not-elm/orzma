import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Packr } from 'msgpackr/index-no-eval';
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

describe('decodeFrame round-trip (Packr → decodeFrame)', () => {
  it('preserves a fabricated FrameDelta through Packr → decodeFrame', () => {
    const packr = new Packr({ useRecords: false, mapsAsObjects: true, int64AsType: 'number' });
    const original = {
      kind: 'delta',
      seq: 42,
      cursor: { x: 5, y: 7, shape: 'block', visible: true },
      dirty_rows: [
        {
          row: 7,
          runs: [
            {
              cols: 5,
              fg: [255, 0, 0],
              bg: null,
              style: 1,
              text: 'hello',
              hyperlink_id: null,
            },
          ],
        },
      ],
    };
    const bytes = packr.pack(original);
    const decoded = decodeFrame(bytes);
    expect(decoded).toEqual(original);
  });
});
