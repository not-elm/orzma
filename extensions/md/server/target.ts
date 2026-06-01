import type { Stats } from 'node:fs';
import { stat } from 'node:fs/promises';
import * as path from 'node:path';

/** Returns the file's `Stats`, or `null` if it cannot be stat'd (missing, no access). */
export async function statOrNull(filePath: string): Promise<Stats | null> {
  try {
    return await stat(filePath);
  } catch {
    return null;
  }
}

/** Resolved target file, or an exit code + message when the gate rejects it. */
export type ResolveResult =
  | { ok: true; filePath: string }
  | { ok: false; code: number; message: string };

/** Resolves `rawPath` against `cwd` and requires it to be an existing regular file. */
export async function resolveTarget(cwd: string, rawPath: string): Promise<ResolveResult> {
  const filePath = path.resolve(cwd, rawPath);
  const s = await statOrNull(filePath);
  if (!s) return { ok: false, code: 1, message: `@md: no such file: ${rawPath}` };
  if (!s.isFile()) return { ok: false, code: 1, message: `@md: not a regular file: ${rawPath}` };
  return { ok: true, filePath };
}
