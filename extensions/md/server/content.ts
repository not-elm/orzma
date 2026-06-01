import type { Stats } from 'node:fs';
import { watch } from 'node:fs';
import { readFile } from 'node:fs/promises';
import * as path from 'node:path';
import type { ChannelGenerator } from '@ozmux/sdk/server';
import type { ContentEvent } from '../content-event.ts';
import { statOrNull } from './target.ts';

/** Maximum file size streamed to the preview; larger files report `too-large`. */
export const MAX_BYTES = 2 * 1024 * 1024;

/** A `${mtimeMs}:${size}` change fingerprint, or `''` when the file is absent. */
export function signatureOf(stat: Stats | null): string {
  return stat ? `${stat.mtimeMs}:${stat.size}` : '';
}

/** Reads the file as UTF-8 using an already-obtained `stat`, or reports missing/too-large. */
export async function readContent(filePath: string, stat: Stats | null): Promise<ContentEvent> {
  if (!stat) return { kind: 'missing' };
  if (stat.size > MAX_BYTES) return { kind: 'too-large', bytes: stat.size };
  try {
    return { kind: 'content', markdown: await readFile(filePath, 'utf8') };
  } catch {
    return { kind: 'missing' };
  }
}

/** Callbacks a {@link ChangeSource} drives over its lifetime. */
export interface WatchCallbacks {
  /** Invoked (debounced) when the watched file may have changed. */
  onChange: () => void;
  /** Invoked when the underlying watcher fails irrecoverably. */
  onError: (err: unknown) => void;
}

/**
 * Watches `basename` within `dir` and reports debounced changes, returning a
 * disposer that stops watching. Injectable so the channel can be tested without
 * real filesystem events.
 */
export type ChangeSource = (
  dir: string,
  basename: string,
  opts: { debounceMs: number } & WatchCallbacks,
) => () => void;

/** Default {@link ChangeSource}: parent-dir `fs.watch`, debounced, surviving editor temp+rename saves. */
export const watchFile: ChangeSource = (dir, basename, { debounceMs, onChange, onError }) => {
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    const watcher = watch(dir, (_event, filename) => {
      // NOTE: `filename` is null on some platforms/events; treat null as "maybe
      // ours" (the re-stat + signature dedupe decides) rather than dropping it.
      if (filename !== null && filename !== basename) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(onChange, debounceMs);
    });
    // NOTE: an unhandled FSWatcher 'error' event crashes the process; route it to
    // onError so the channel terminates cleanly (e.g. when the dir is removed).
    watcher.on('error', onError);
    return () => {
      if (timer) clearTimeout(timer);
      watcher.close();
    };
  } catch (err) {
    onError(err);
    return () => {};
  }
};

/**
 * Builds the `content` channel for one preview: arms the watcher first, then
 * yields the file's content and re-yields on every real change (deduped via a
 * single stat per tick). `source` is injectable for tests.
 */
export function makeContentChannel(
  filePath: string,
  source: ChangeSource = watchFile,
): ChannelGenerator<Record<string, never>, ContentEvent> {
  return async function* (_params, { signal }) {
    let notify: (() => void) | null = null;
    let pending = false;
    let failure: unknown = null;
    const wake = () => {
      const n = notify;
      notify = null;
      n?.();
    };
    const onAbort = () => wake();
    signal.addEventListener('abort', onAbort, { once: true });

    // Arm the watcher BEFORE the initial read so a write during startup is observed.
    const dispose = source(path.dirname(filePath), path.basename(filePath), {
      debounceMs: 50,
      onChange: () => {
        pending = true;
        wake();
      },
      onError: (err) => {
        failure = err;
        wake();
      },
    });

    try {
      let stat = await statOrNull(filePath);
      let lastSig = signatureOf(stat);
      yield await readContent(filePath, stat);

      while (!signal.aborted) {
        if (!pending && failure === null) {
          await new Promise<void>((resolve) => {
            notify = resolve;
          });
        }
        if (signal.aborted) return;
        if (failure !== null) {
          throw failure instanceof Error ? failure : new Error(String(failure));
        }
        pending = false;
        stat = await statOrNull(filePath);
        const sig = signatureOf(stat);
        if (sig === lastSig) continue;
        lastSig = sig;
        if (signal.aborted) return;
        yield await readContent(filePath, stat);
      }
    } finally {
      signal.removeEventListener('abort', onAbort);
      dispose();
    }
  };
}
