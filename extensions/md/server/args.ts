import { parseArgs } from 'node:util';

/** Pane split orientation accepted by `-s` / `--split`. */
export type Orientation = 'horizontal' | 'vertical';

/** Result of parsing `@md` argv: either the validated inputs or an exit code + message. */
export type ParsedMdArgs =
  | { ok: true; rawPath: string; split: Orientation | undefined }
  | { ok: false; code: number; message: string };

const USAGE = 'usage: @md [-s|--split <horizontal|vertical>] <file>';

/** Parses `@md` arguments (command name already stripped) into a validated result. */
export function parseMdArgs(argv: string[]): ParsedMdArgs {
  let values: { split?: string };
  let positionals: string[];
  try {
    ({ values, positionals } = parseArgs({
      args: argv,
      options: { split: { type: 'string', short: 's' } },
      allowPositionals: true,
    }) as { values: { split?: string }; positionals: string[] });
  } catch (err) {
    return { ok: false, code: 2, message: `${(err as Error).message}\n${USAGE}` };
  }

  if (positionals.length !== 1) {
    return { ok: false, code: 2, message: USAGE };
  }

  const split = values.split;
  if (split !== undefined && split !== 'horizontal' && split !== 'vertical') {
    return { ok: false, code: 2, message: `invalid orientation: ${split}\n${USAGE}` };
  }

  return { ok: true, rawPath: positionals[0], split };
}
