import { watch } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import * as path from 'node:path';
import type { ChannelGenerator } from '@ozmux/sdk/server';
import type { ContentEvent } from '../content-event.ts';
import { statOrNull } from './target.ts';

/** Maximum file size streamed to the preview; larger files report `too-large`. */
export const MAX_BYTES = 2 * 1024 * 1024;

/** Reads the file as UTF-8, or reports `missing` / `too-large` without throwing. */
export async function readContent(filePath: string): Promise<ContentEvent> {
  const s = await statOrNull(filePath);
  if (!s) return { kind: 'missing' };
  if (s.size > MAX_BYTES) return { kind: 'too-large', bytes: s.size };
  try {
    return { kind: 'content', markdown: await readFile(filePath, 'utf8') };
  } catch {
    return { kind: 'missing' };
  }
}

/** A `${mtimeMs}:${size}` fingerprint, or `''` if the file is gone. Used to skip no-op re-renders. */
export async function signatureOf(filePath: string): Promise<string> {
  try {
    const s = await stat(filePath);
    return `${s.mtimeMs}:${s.size}`;
  } catch {
    return '';
  }
}

/** Emits a tick whenever `basename` in `dir` changes; debounced and abort-aware. */
export type ChangeSource = (
  dir: string,
  basename: string,
  opts: { signal: AbortSignal; debounceMs: number },
) => AsyncGenerator<void, void, undefined>;

/** Default {@link ChangeSource}: watches the parent dir so editor temp+rename saves are caught. */
export async function* watchFile(
  dir: string,
  basename: string,
  opts: { signal: AbortSignal; debounceMs: number },
): AsyncGenerator<void, void, undefined> {
  const { signal, debounceMs } = opts;
  if (signal.aborted) return;

  let pending = false;
  let wake: (() => void) | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const fire = () => {
    pending = true;
    const w = wake;
    wake = null;
    w?.();
  };

  const watcher = watch(dir, (_event, filename) => {
    if (filename !== basename) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(fire, debounceMs);
  });

  const onAbort = () => {
    const w = wake;
    wake = null;
    w?.();
  };
  signal.addEventListener('abort', onAbort, { once: true });

  try {
    while (!signal.aborted) {
      if (!pending) {
        await new Promise<void>((resolve) => {
          wake = resolve;
        });
      }
      if (signal.aborted) return;
      pending = false;
      yield;
    }
  } finally {
    if (timer) clearTimeout(timer);
    watcher.close();
    signal.removeEventListener('abort', onAbort);
  }
}

/**
 * Builds the `content` channel for one preview: yields the file's content on
 * subscribe, then re-yields on every real change. `source` is injectable for tests.
 */
export function makeContentChannel(
  filePath: string,
  source: ChangeSource = watchFile,
): ChannelGenerator<Record<string, never>, ContentEvent> {
  return async function* (_params, { signal }) {
    yield await readContent(filePath);
    let lastSig = await signatureOf(filePath);
    for await (const _tick of source(path.dirname(filePath), path.basename(filePath), {
      signal,
      debounceMs: 50,
    })) {
      if (signal.aborted) return;
      const sig = await signatureOf(filePath);
      if (sig === lastSig) continue;
      lastSig = sig;
      yield await readContent(filePath);
    }
  };
}
