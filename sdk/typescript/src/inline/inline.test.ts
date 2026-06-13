import { describe, expect, it } from 'vitest';
import { mountInline, unmountInline } from './inline.ts';

const ESC = '\x1b';
const ST = `${ESC}\\`;

describe('mountInline', () => {
  it('emits the OSC 5379 mount-inline sequence followed by rows newlines', () => {
    const out = mountInline('memo.main', { rows: 3, cols: 20 });
    expect(out).toBe(`${ESC}]5379;mount-inline;memo.main;3;20${ST}\n\n\n`);
  });

  it('reserves exactly rows newlines', () => {
    expect(mountInline('v', { rows: 1, cols: 1 }).endsWith(`${ST}\n`)).toBe(true);
    expect([...mountInline('v', { rows: 5, cols: 2 })].filter((c) => c === '\n')).toHaveLength(5);
  });

  it('rejects out-of-range rows/cols', () => {
    expect(() => mountInline('v', { rows: 0, cols: 10 })).toThrow();
    expect(() => mountInline('v', { rows: 201, cols: 10 })).toThrow();
    expect(() => mountInline('v', { rows: 10, cols: 0 })).toThrow();
    expect(() => mountInline('v', { rows: 10, cols: 401 })).toThrow();
    expect(() => mountInline('v', { rows: 1.5, cols: 10 })).toThrow();
  });

  it('accepts the boundary values', () => {
    expect(() => mountInline('v', { rows: 1, cols: 1 })).not.toThrow();
    expect(() => mountInline('v', { rows: 200, cols: 400 })).not.toThrow();
  });

  it('rejects invalid view ids', () => {
    expect(() => mountInline('', { rows: 1, cols: 1 })).toThrow();
    expect(() => mountInline('a;b', { rows: 1, cols: 1 })).toThrow();
    expect(() => mountInline('../etc', { rows: 1, cols: 1 })).toThrow();
    expect(() => mountInline('a'.repeat(129), { rows: 1, cols: 1 })).toThrow();
  });

  it('accepts the full legal view-id charset', () => {
    expect(() => mountInline('a.b_c-D9', { rows: 1, cols: 1 })).not.toThrow();
  });

  it('appends the instance id as the 5th field when given', () => {
    const out = mountInline('memo.main', { rows: 3, cols: 20, instanceId: 'a' });
    expect(out).toBe(`${ESC}]5379;mount-inline;memo.main;3;20;a${ST}\n\n\n`);
  });

  it('omits the instance field when instanceId is undefined', () => {
    const out = mountInline('memo.main', { rows: 3, cols: 20 });
    expect(out).toBe(`${ESC}]5379;mount-inline;memo.main;3;20${ST}\n\n\n`);
  });

  it('rejects an invalid instance id', () => {
    expect(() => mountInline('memo.main', { rows: 1, cols: 1, instanceId: 'a;b' })).toThrow();
    expect(() => mountInline('memo.main', { rows: 1, cols: 1, instanceId: '' })).toThrow();
  });
});

describe('unmountInline', () => {
  it('emits a view-scoped unmount with the id', () => {
    expect(unmountInline('memo.main')).toBe(`${ESC}]5379;unmount-inline;memo.main${ST}`);
  });

  it('emits an unmount-all with NO trailing semicolon when no id is given', () => {
    expect(unmountInline()).toBe(`${ESC}]5379;unmount-inline${ST}`);
  });

  it('rejects an invalid view id', () => {
    expect(() => unmountInline('a;b')).toThrow();
  });

  it('emits an instance-scoped unmount with both ids', () => {
    expect(unmountInline('memo.main', 'a')).toBe(`${ESC}]5379;unmount-inline;memo.main;a${ST}`);
  });

  it('throws when an instanceId is given without a viewId', () => {
    expect(() => unmountInline(undefined, 'a')).toThrow();
  });

  it('rejects an invalid instance id', () => {
    expect(() => unmountInline('memo.main', 'a;b')).toThrow();
  });
});
