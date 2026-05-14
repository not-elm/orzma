import { Unpackr } from 'msgpackr/index-no-eval';
import { describe, expect, it } from 'vitest';
import { encodeInputFrame } from './encode-input';

const unpackr = new Unpackr({ useRecords: false, mapsAsObjects: true, int64AsType: 'number' });

describe('encodeInputFrame', () => {
  it('wraps bytes in {kind: "input", data: bytes} msgpack', () => {
    const encoded = encodeInputFrame(new Uint8Array([97, 98, 99]));
    const decoded = unpackr.unpack(encoded) as { kind: string; data: Uint8Array };
    expect(decoded.kind).toBe('input');
    expect(Array.from(decoded.data)).toEqual([97, 98, 99]);
  });
});
