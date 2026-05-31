import { execFileSync } from 'node:child_process';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import { assertCommandName, shellSingleQuote, writeShim } from './shim-writer.ts';

describe('assertCommandName', () => {
  it('accepts simple lowercase names', () => {
    expect(() => assertCommandName('memo')).not.toThrow();
    expect(() => assertCommandName('foo-bar_2')).not.toThrow();
  });

  it('accepts @-prefixed names', () => {
    expect(() => assertCommandName('@memo')).not.toThrow();
    expect(() => assertCommandName('@browser')).not.toThrow();
    expect(() => assertCommandName(`@${'a'.repeat(64)}`)).not.toThrow();
  });

  it('rejects uppercase, slashes, dots, empty, and overlong names', () => {
    for (const bad of ['', 'Foo', 'foo/bar', '..', '9start', 'a'.repeat(65)]) {
      expect(() => assertCommandName(bad), `should reject ${JSON.stringify(bad)}`).toThrow();
    }
  });

  it('rejects malformed @-prefixed names', () => {
    for (const bad of ['@', '@-foo', '@_foo', '@9x', '@@memo', `@${'a'.repeat(65)}`]) {
      expect(() => assertCommandName(bad), `should reject ${JSON.stringify(bad)}`).toThrow();
    }
  });
});

describe('shellSingleQuote', () => {
  it('wraps plain strings in single quotes', () => {
    expect(shellSingleQuote('/usr/bin/node')).toBe("'/usr/bin/node'");
  });
  it("escapes embedded single quotes with '\\''", () => {
    expect(shellSingleQuote("ab'cd")).toBe("'ab'\\''cd'");
  });
  it('preserves spaces and newlines inside the quotes', () => {
    expect(shellSingleQuote('a b\nc')).toBe("'a b\nc'");
  });
});

describe('writeShim', () => {
  it('writes a 0500 sh script with single-quoted embedded values', async () => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ozmux-shim-test-'));
    try {
      const filePath = path.join(dir, 'memo');
      await writeShim({
        filePath,
        execPath: '/usr/local/bin/node',
        helperPath: '/path/with space/cmd-shim.js',
        socketPath: "/tmp/sock with 'quote.sock",
        commandName: 'memo',
      });

      const stat = await fs.stat(filePath);
      expect(stat.mode & 0o777).toBe(0o500);

      const text = await fs.readFile(filePath, 'utf8');
      expect(text.startsWith('#!/bin/sh\n')).toBe(true);
      expect(text).toContain("'/usr/local/bin/node'");
      expect(text).toContain("'/path/with space/cmd-shim.js'");
      expect(text).toContain("'/tmp/sock with '\\''quote.sock'");
      expect(text).toContain("'memo'");
      expect(text.trimEnd().endsWith('-- "$@"')).toBe(true);

      // Script must parse as POSIX sh.
      execFileSync('sh', ['-n', filePath]);
    } finally {
      await fs.rm(dir, { recursive: true, force: true });
    }
  });
});
