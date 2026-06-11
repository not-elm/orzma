import { describe, expect, it } from 'vitest';
import { decodeHostValue, encodeHostValue, isBinaryEnvelope } from './binary-codec.ts';

describe('binary-codec', () => {
  it('wraps a top-level Uint8Array as a base64 envelope', () => {
    const enc = encodeHostValue(new Uint8Array([1, 2, 3]));
    expect(isBinaryEnvelope(enc)).toBe(true);
    expect((enc as { __u8: string }).__u8).toBe(Buffer.from([1, 2, 3]).toString('base64'));
  });

  it('wraps a Node Buffer (Uint8Array subclass)', () => {
    const enc = encodeHostValue(Buffer.from('hi', 'utf8'));
    expect(isBinaryEnvelope(enc)).toBe(true);
  });

  it('passes plain JSON values through unchanged', () => {
    expect(encodeHostValue({ a: 1 })).toEqual({ a: 1 });
    expect(encodeHostValue('x')).toBe('x');
    expect(encodeHostValue(42)).toBe(42);
    expect(encodeHostValue(null)).toBe(null);
  });

  it('round-trips through decode back to a Uint8Array', () => {
    const decoded = decodeHostValue(encodeHostValue(new Uint8Array([9, 8, 7])));
    expect(decoded).toBeInstanceOf(Uint8Array);
    expect(decoded).toEqual(new Uint8Array([9, 8, 7]));
  });

  it('decode passes non-envelope values through', () => {
    expect(decodeHostValue({ a: 1 })).toEqual({ a: 1 });
    expect(decodeHostValue('x')).toBe('x');
  });

  it('does NOT deep-encode a nested Uint8Array (boundary-tagged only)', () => {
    const enc = encodeHostValue({ buf: new Uint8Array([1]) }) as { buf: unknown };
    expect(isBinaryEnvelope(enc.buf)).toBe(false);
  });
});
