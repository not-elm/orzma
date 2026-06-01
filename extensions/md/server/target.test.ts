import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { resolveTarget, statOrNull } from './target.ts';

const dir = await mkdtemp(path.join(tmpdir(), 'md-target-'));

afterAll(async () => {
  await import('node:fs/promises').then((fs) => fs.rm(dir, { recursive: true, force: true }));
});

describe('statOrNull', () => {
  it('returns null for a missing path', async () => {
    expect(await statOrNull(path.join(dir, 'nope'))).toBeNull();
  });
});

describe('resolveTarget', () => {
  it('resolves a relative path against cwd and accepts a regular file', async () => {
    const file = path.join(dir, 'a.md');
    await writeFile(file, '# hi');
    const r = await resolveTarget(dir, 'a.md');
    expect(r).toEqual({ ok: true, filePath: file });
  });

  it('rejects a missing file with code 1', async () => {
    const r = await resolveTarget(dir, 'missing.md');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe(1);
      expect(r.message).toContain('no such file');
    }
  });

  it('rejects a directory with code 1', async () => {
    const r = await resolveTarget(dir, '.');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.code).toBe(1);
      expect(r.message).toContain('not a regular file');
    }
  });
});
