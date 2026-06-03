import * as path from 'node:path';
import type { ChannelGenerator, CommandContext, SurfaceSpecInput } from '@ozmux/sdk/server';
import type { ContentEvent } from '../content-event.ts';
import { parseMdArgs } from './args.ts';
import { resolveTarget, statOrNull } from './target.ts';

/** Host-supplied bits the command needs: the built client entry, and the channel factory. */
export interface MdDeps {
  distIndexPath: string;
  makeChannel: (filePath: string) => ChannelGenerator<Record<string, never>, ContentEvent>;
}

/** Runs the `@md` command: parse → gate → build-guard → open the preview surface. */
export async function mdCommand(ctx: CommandContext, deps: MdDeps): Promise<number> {
  const parsed = parseMdArgs(ctx.argv);
  if (!parsed.ok) return fail(ctx, parsed);

  const target = await resolveTarget(ctx.cwd, parsed.rawPath);
  if (!target.ok) return fail(ctx, target);

  if (!(await statOrNull(deps.distIndexPath))?.isFile()) {
    ctx.stderr.write('@md: client not built — run `pnpm build` (missing dist/index.html)\n');
    return 1;
  }

  const surface: SurfaceSpecInput = {
    kind: 'extension',
    name: path.basename(target.filePath),
    cwd: ctx.cwd,
    html: deps.distIndexPath,
    channels: { content: deps.makeChannel(target.filePath) },
  };

  if (parsed.split) {
    await ctx.pane.split({ orientation: parsed.split, side: 'after', surface });
  } else {
    const created = await ctx.pane.addSurface(surface);
    await created.activate();
  }
  return 0;
}

/** Writes a gate failure's message to stderr and returns its exit code. */
function fail(ctx: CommandContext, result: { message: string; code: number }): number {
  ctx.stderr.write(`${result.message}\n`);
  return result.code;
}
