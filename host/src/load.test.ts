import { describe, expect, it } from 'vitest';
import type { ExtensionDescriptor } from './descriptors.ts';
import type { ApiImporter } from './extension-loader.ts';
import { loadHostApi } from './load.ts';

function fakeImporter(modules: Record<string, unknown>): ApiImporter {
  return async (specifier: string) => {
    if (!(specifier in modules)) throw new Error(`no module ${specifier}`);
    return { default: modules[specifier] };
  };
}

const d = (name: string, apiPaths: string[]): ExtensionDescriptor => ({
  name,
  apiPaths,
  assetRoot: `/p/${name}`,
});

describe('loadHostApi', () => {
  it('merges multiple api files within one extension', async () => {
    const importer = fakeImporter({
      '/a/fs.ts': { fs: { read: async () => 'r' } },
      '/a/net.ts': { net: { get: async () => 'g' } },
    });
    const { api, warnings } = await loadHostApi([d('a', ['/a/fs.ts', '/a/net.ts'])], importer);
    expect(Object.keys(api).sort()).toEqual(['fs', 'net']);
    expect(warnings).toEqual([]);
  });

  it('keeps the first loader on namespace collision (user-first order) and warns', async () => {
    const userFs = { read: async () => 'user' };
    const importer = fakeImporter({
      '/user/fs.ts': { fs: userFs },
      '/bundled/fs.ts': { fs: { read: async () => 'bundled' } },
    });
    const { api, warnings } = await loadHostApi(
      [d('user', ['/user/fs.ts']), d('bundled', ['/bundled/fs.ts'])],
      importer,
    );
    expect(api.fs).toBe(userFs);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('fs');
  });

  it('is fail-soft: a broken api file is skipped with a warning, others still load', async () => {
    const importer = fakeImporter({
      '/a/bad.ts': 42, // non-object default → loadExtension throws
      '/a/ok.ts': { fs: { read: async () => 'r' } },
    });
    const { api, warnings } = await loadHostApi([d('a', ['/a/bad.ts', '/a/ok.ts'])], importer);
    expect(Object.keys(api)).toEqual(['fs']);
    expect(warnings.some((w) => w.includes('/a/bad.ts'))).toBe(true);
  });

  it('is fail-soft: a missing module is skipped, not fatal', async () => {
    const importer = fakeImporter({ '/a/ok.ts': { fs: { read: async () => 1 } } });
    const { api, warnings } = await loadHostApi([d('a', ['/a/missing.ts', '/a/ok.ts'])], importer);
    expect(Object.keys(api)).toEqual(['fs']);
    expect(warnings.some((w) => w.includes('/a/missing.ts'))).toBe(true);
  });
});
