import { describe, expect, it } from 'vitest';
import { parseHostManifest } from './descriptors.ts';

describe('parseHostManifest', () => {
  it('parses a well-formed manifest', () => {
    const m = parseHostManifest(
      JSON.stringify({
        extensions: [{ name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'] }],
      }),
    );
    expect(m.extensions).toEqual([{ name: 'memo', apiPaths: ['/abs/memo/api/fs.ts'] }]);
  });

  it('accepts an empty extensions array', () => {
    expect(parseHostManifest('{"extensions":[]}').extensions).toEqual([]);
  });

  it('throws on malformed JSON', () => {
    expect(() => parseHostManifest('{not json')).toThrow(/host manifest/i);
  });

  it('throws when extensions is missing or not an array', () => {
    expect(() => parseHostManifest('{}')).toThrow(/host manifest/i);
    expect(() => parseHostManifest('{"extensions":"x"}')).toThrow(/host manifest/i);
  });

  it('throws when a extension entry has the wrong shape', () => {
    expect(() => parseHostManifest('{"extensions":[{"name":"x"}]}')).toThrow(/host manifest/i);
    expect(() => parseHostManifest('{"extensions":[{"name":"x","apiPaths":"y"}]}')).toThrow(
      /host manifest/i,
    );
  });
});
