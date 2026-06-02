import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import type { CommandContext, SplitArgs, SurfaceSpecInput } from '@ozmux/sdk/server';
import { afterAll, describe, expect, it, vi } from 'vitest';
import { type MdDeps, mdCommand } from './command.ts';

const dir = await mkdtemp(path.join(tmpdir(), 'md-cmd-'));
afterAll(async () =>
  import('node:fs/promises').then((fs) => fs.rm(dir, { recursive: true, force: true })),
);

function fakeCtx(argv: string[]) {
  const errs: string[] = [];
  const activate = vi.fn(async () => {});
  const split = vi.fn((_args: SplitArgs) => Promise.resolve({}));
  const addSurface = vi.fn((_spec: SurfaceSpecInput) => Promise.resolve({ activate }));
  const ctx = {
    argv,
    cwd: dir,
    stderr: {
      write: (s: string) => {
        errs.push(s);
        return true;
      },
    },
    pane: { split, addSurface },
  } as unknown as CommandContext;
  return { ctx, errs, split, addSurface, activate };
}

async function deps(): Promise<MdDeps> {
  const distIndexPath = path.join(dir, 'dist-index.html');
  await writeFile(distIndexPath, '<!doctype html>');
  return {
    distIndexPath,
    makeChannel: () => async function* () {},
  };
}

describe('mdCommand', () => {
  it('returns 2 and prints usage when no file is given', async () => {
    const { ctx, errs } = fakeCtx([]);
    expect(await mdCommand(ctx, await deps())).toBe(2);
    expect(errs.join('')).toContain('usage:');
  });

  it('returns 1 when the file is missing', async () => {
    const { ctx, errs } = fakeCtx(['missing.md']);
    expect(await mdCommand(ctx, await deps())).toBe(1);
    expect(errs.join('')).toContain('no such file');
  });

  it('returns 1 with a build hint when dist/index.html is absent', async () => {
    await writeFile(path.join(dir, 'present.md'), '# x');
    const { ctx, errs } = fakeCtx(['present.md']);
    const badDeps: MdDeps = {
      distIndexPath: path.join(dir, 'nope.html'),
      makeChannel: () => async function* () {},
    };
    expect(await mdCommand(ctx, badDeps)).toBe(1);
    expect(errs.join('')).toContain('client not built');
  });

  it('splits the pane when -s is given', async () => {
    await writeFile(path.join(dir, 's.md'), '# x');
    const { ctx, split, addSurface } = fakeCtx(['-s', 'vertical', 's.md']);
    expect(await mdCommand(ctx, await deps())).toBe(0);
    expect(addSurface).not.toHaveBeenCalled();
    expect(split).toHaveBeenCalledTimes(1);
    // biome-ignore lint/style/noNonNullAssertion: guarded by the toHaveBeenCalledTimes(1) assertion above
    const arg = split.mock.calls[0]![0]!;
    expect(arg.orientation).toBe('vertical');
    expect(arg.side).toBe('after');
    expect(arg.surface.kind).toBe('extension');
    expect(arg.surface.name).toBe('s.md');
    if (arg.surface.kind === 'extension') {
      expect(typeof arg.surface.channels?.content).toBe('function');
    }
  });

  it('adds + activates an in-pane surface (with its content channel) when no flag is given', async () => {
    await writeFile(path.join(dir, 'inpane.md'), '# x');
    const { ctx, split, addSurface, activate } = fakeCtx(['inpane.md']);
    expect(await mdCommand(ctx, await deps())).toBe(0);
    expect(split).not.toHaveBeenCalled();
    expect(addSurface).toHaveBeenCalledTimes(1);
    expect(activate).toHaveBeenCalledTimes(1);
    // biome-ignore lint/style/noNonNullAssertion: guarded by the toHaveBeenCalledTimes(1) assertion above
    const spec = addSurface.mock.calls[0]![0]!;
    expect(spec.kind).toBe('extension');
    if (spec.kind === 'extension') {
      expect(typeof spec.channels?.content).toBe('function');
    }
  });
});
