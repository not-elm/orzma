import { describe, expect, it } from 'vitest';
import { defineApi } from './define-api.ts';

describe('defineApi', () => {
  it('returns the same object reference (identity helper)', () => {
    const api = { fs: { read: async (p: string) => p } };
    expect(defineApi(api)).toBe(api);
  });

  it('preserves nested methods callable form', async () => {
    const api = defineApi({ math: { add: async (a: number, b: number) => a + b } });
    expect(await api.math.add(2, 3)).toBe(5);
  });
});
