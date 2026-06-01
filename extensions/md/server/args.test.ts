import { describe, expect, it } from 'vitest';
import { parseMdArgs } from './args.ts';

describe('parseMdArgs', () => {
  it('parses a bare file path', () => {
    expect(parseMdArgs(['foo.md'])).toEqual({ ok: true, rawPath: 'foo.md', split: undefined });
  });

  it('parses -s vertical with a path', () => {
    expect(parseMdArgs(['-s', 'vertical', 'foo.md'])).toEqual({
      ok: true,
      rawPath: 'foo.md',
      split: 'vertical',
    });
  });

  it('parses --split=horizontal with a path', () => {
    expect(parseMdArgs(['--split=horizontal', 'a.md'])).toEqual({
      ok: true,
      rawPath: 'a.md',
      split: 'horizontal',
    });
  });

  it('rejects zero positionals with code 2', () => {
    const r = parseMdArgs([]);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe(2);
  });

  it('rejects more than one positional with code 2', () => {
    const r = parseMdArgs(['a.md', 'b.md']);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe(2);
  });

  it('rejects an invalid orientation with code 2', () => {
    const r = parseMdArgs(['-s', 'sideways', 'a.md']);
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe(2);
      expect(r.message).toContain('sideways');
    }
  });

  it('rejects an unknown flag with code 2', () => {
    const r = parseMdArgs(['--nope', 'a.md']);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.code).toBe(2);
  });
});
