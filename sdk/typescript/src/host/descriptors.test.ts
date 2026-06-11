import { describe, expect, it } from 'vitest';
import { parseHostManifest } from './descriptors.ts';

describe('parseHostManifest', () => {
  it('parses a well-formed manifest', () => {
    const m = parseHostManifest(
      JSON.stringify({
        plugins: [{ name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'], assetRoot: '/abs/memo' }],
      }),
    );
    expect(m.plugins).toEqual([
      { name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'], assetRoot: '/abs/memo' },
    ]);
  });

  it('accepts an empty plugins array', () => {
    expect(parseHostManifest('{"plugins":[]}').plugins).toEqual([]);
  });

  it('throws on malformed JSON', () => {
    expect(() => parseHostManifest('{not json')).toThrow(/host manifest/i);
  });

  it('throws when plugins is missing or not an array', () => {
    expect(() => parseHostManifest('{}')).toThrow(/host manifest/i);
    expect(() => parseHostManifest('{"plugins":"x"}')).toThrow(/host manifest/i);
  });

  it('throws when a plugin entry has the wrong shape', () => {
    expect(() => parseHostManifest('{"plugins":[{"name":"x"}]}')).toThrow(/host manifest/i);
    expect(() =>
      parseHostManifest('{"plugins":[{"name":"x","apiPaths":"y","assetRoot":"z"}]}'),
    ).toThrow(/host manifest/i);
  });
});
