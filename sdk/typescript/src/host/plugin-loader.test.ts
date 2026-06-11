import { describe, expect, it } from 'vitest';
import { loadPlugin, mergeApis } from './plugin-loader.ts';

describe('mergeApis', () => {
  it('merges disjoint namespaces from multiple plugins', () => {
    const { api, warnings } = mergeApis([
      { name: 'a', api: { fs: { read: async () => 1 } } },
      { name: 'b', api: { net: { get: async () => 2 } } },
    ]);
    expect(Object.keys(api).sort()).toEqual(['fs', 'net']);
    expect(warnings).toEqual([]);
  });

  it('keeps the earlier plugin on namespace collision and warns for the later', () => {
    const first = { read: async () => 'first' };
    const { api, warnings } = mergeApis([
      { name: 'a', api: { fs: first } },
      { name: 'b', api: { fs: { read: async () => 'second' } } },
    ]);
    expect(api.fs).toBe(first);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('fs');
    expect(warnings[0]).toContain('b');
    expect(warnings[0]).toContain('a');
  });

  it('does not treat prototype-member names as pre-existing namespaces', () => {
    const { api, warnings } = mergeApis([
      { name: 'a', api: { toString: { run: async () => 1 }, constructor: { run: async () => 2 } } },
    ]);
    expect(Object.keys(api).sort()).toEqual(['constructor', 'toString']);
    expect(warnings).toEqual([]);
  });
});

describe('loadPlugin', () => {
  it('returns the default export as the plugin api', async () => {
    const importer = async () => ({ default: { fs: { read: async () => 'ok' } } });
    const p = await loadPlugin('memo', '/abs/memo/api.ts', importer);
    expect(p.name).toBe('memo');
    expect(Object.keys(p.api)).toEqual(['fs']);
  });

  it('rejects when the module has no object default export', async () => {
    const importer = async () => ({ default: 42 });
    await expect(loadPlugin('bad', '/abs/bad/api.ts', importer)).rejects.toThrow(
      /default-export an object/,
    );
  });

  it('rejects when the module has no default export', async () => {
    const importer = async () => ({});
    await expect(loadPlugin('bad', '/abs/bad/api.ts', importer)).rejects.toThrow(
      /default-export an object/,
    );
  });

  it('rejects when the default export is null', async () => {
    const importer = async () => ({ default: null });
    await expect(loadPlugin('bad', '/abs/bad/api.ts', importer)).rejects.toThrow(
      /default-export an object/,
    );
  });
});
