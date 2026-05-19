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
      cursor: { x: 5, y: 7, shape: 'block', blinking: false, visible: true },
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
      hyperlinks: [],
    };
    const bytes = packr.pack(original);
    const decoded = decodeFrame(bytes);
    expect(decoded).toEqual(original);
  });
});

describe('produced_at_us fixture roundtrips', () => {
  it('snapshot_with_produced_at decodes with produced_at_us', () => {
    const frame = decodeFrame(readBin('snapshot_with_produced_at'));
    expect(frame.kind).toBe('snapshot');
    if (frame.kind === 'snapshot') {
      expect(frame.produced_at_us).toBe(1_700_000_000_000_000);
    }
  });

  it('delta_with_produced_at decodes with produced_at_us', () => {
    const frame = decodeFrame(readBin('delta_with_produced_at'));
    expect(frame.kind).toBe('delta');
    if (frame.kind === 'delta') {
      expect(frame.produced_at_us).toBe(1_700_000_000_000_001);
    }
  });
});

describe('Phase 3B wire types', () => {
  it('snapshot_with_hyperlinks decodes with hyperlinks field', () => {
    const frame = decodeFrame(readBin('snapshot_with_hyperlinks'));
    expect(frame.kind).toBe('snapshot');
    if (frame.kind === 'snapshot') {
      // NOTE: golden was regenerated in PR-0 (#43) — emit_fixture produces
      // id=1, uri="https://example.com/". This test's prior assertion was
      // stale (expected id=0, "https://ozmux.example") and only surfaced
      // when PR-A added the testing module + ran frontend tests fresh.
      expect(frame.hyperlinks).toEqual([{ id: 1, uri: 'https://example.com/' }]);
    }
  });

  it('delta_cursor_shape decodes with steady bar cursor and empty hyperlinks', () => {
    const frame = decodeFrame(readBin('delta_cursor_shape'));
    expect(frame.kind).toBe('delta');
    if (frame.kind === 'delta') {
      expect(frame.cursor.shape).toBe('bar');
      expect(frame.cursor.blinking).toBe(false);
      expect(frame.hyperlinks).toEqual([]);
    }
  });
});
