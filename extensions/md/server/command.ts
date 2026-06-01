import * as path from 'node:path';
import type { ActivitySpecInput, ChannelGenerator, CommandContext } from '@ozmux/sdk/server';
import { parseMdArgs } from './args.ts';
import { resolveTarget, statOrNull } from './target.ts';

/** Host-supplied bits the command needs: the built client entry, and the channel factory. */
export interface MdDeps {
  distIndexPath: string;
  // biome-ignore lint/suspicious/noExplicitAny: channel map values are typed as ChannelGenerator<any,any>; using any here avoids a variance error when callers supply a narrower generator
  makeChannel: (filePath: string) => ChannelGenerator<any, any>;
}

/** Runs the `@md` command: parse → gate → build-guard → open the preview activity. */
export async function mdCommand(ctx: CommandContext, deps: MdDeps): Promise<number> {
  const parsed = parseMdArgs(ctx.argv);
  if (!parsed.ok) {
    ctx.stderr.write(`${parsed.message}\n`);
    return parsed.code;
  }

  const target = await resolveTarget(ctx.cwd, parsed.rawPath);
  if (!target.ok) {
    ctx.stderr.write(`${target.message}\n`);
    return target.code;
  }

  if (!(await statOrNull(deps.distIndexPath))?.isFile()) {
    ctx.stderr.write('@md: client not built — run `pnpm build` (missing dist/index.html)\n');
    return 1;
  }

  const activity: ActivitySpecInput = {
    kind: 'extension',
    name: path.basename(target.filePath),
    html: deps.distIndexPath,
    channels: { content: deps.makeChannel(target.filePath) },
  };

  if (parsed.split) {
    await ctx.pane.split({ orientation: parsed.split, side: 'after', activity });
  } else {
    const created = await ctx.pane.addActivity(activity);
    await created.activate();
  }
  return 0;
}
