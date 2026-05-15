import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseHello } from './hello';

const HELLO_BIN = join(__dirname, '../../../../terminal/tests/fixtures/wire_msgpack/hello.bin');

describe('parseHello', () => {
  it('parses a valid hello frame from the Phase 2A fixture', () => {
    const text = readFileSync(HELLO_BIN, 'utf8');
    const hello = parseHello(text);
    expect(hello.kind).toBe('hello');
    expect(hello.seq).toBe(0);
    expect(typeof hello.cols).toBe('number');
    expect(typeof hello.rows).toBe('number');
    expect(hello.cursor).toEqual({ x: 0, y: 0, shape: 'block', blinking: false, visible: true });
    expect(Array.isArray(hello.escape_caps)).toBe(true);
    expect(Array.isArray(hello.input_caps)).toBe(true);
  });

  it('throws on missing kind field', () => {
    expect(() => parseHello('{"seq":0,"cols":80}')).toThrow();
  });

  it('throws on wrong kind value', () => {
    expect(() =>
      parseHello(
        '{"kind":"mode","seq":0,"cols":80,"rows":24,"cursor":{"x":0,"y":0,"shape":"block","visible":true},"escape_caps":[],"input_caps":[]}',
      ),
    ).toThrow();
  });
});
