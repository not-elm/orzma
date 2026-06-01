import { mkdtemp, rm, stat as statFs, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import type { ContentEvent } from '../content-event.ts';
import {
  type ChangeSource,
  MAX_BYTES,
  makeContentChannel,
  readContent,
  signatureOf,
} from './content.ts';

const dir = await mkdtemp(path.join(tmpdir(), 'md-content-'));
afterAll(async () => rm(dir, { recursive: true, force: true }));

const flush = () => new Promise<void>((r) => setTimeout(r, 30));

describe('readContent', () => {
  it('returns the UTF-8 text for a normal file', async () => {
    const f = path.join(dir, 'a.md');
    await writeFile(f, '# hi');
    expect(await readContent(f, await statFs(f))).toEqual({ kind: 'content', markdown: '# hi' });
  });

  it('returns missing for a null stat', async () => {
    expect(await readContent(path.join(dir, 'nope.md'), null)).toEqual({ kind: 'missing' });
  });

  it('returns too-large above the cap', async () => {
    const f = path.join(dir, 'big.md');
    await writeFile(f, 'x'.repeat(MAX_BYTES + 1));
    const ev = await readContent(f, await statFs(f));
    expect(ev.kind).toBe('too-large');
    if (ev.kind === 'too-large') expect(ev.bytes).toBe(MAX_BYTES + 1);
  });
});

describe('signatureOf', () => {
  it('changes when the file content changes', async () => {
    const f = path.join(dir, 'sig.md');
    await writeFile(f, 'one');
    const a = signatureOf(await statFs(f));
    await writeFile(f, 'one and two');
    const b = signatureOf(await statFs(f));
    expect(a).not.toBe(b);
  });

  it('returns empty string for a null stat', () => {
    expect(signatureOf(null)).toBe('');
  });
});

/** A manually-driven {@link ChangeSource}: `push()` fires one debounced change; `fail()` an error. */
function createPushSource() {
  let onChange: (() => void) | null = null;
  let onError: ((err: unknown) => void) | null = null;
  const source: ChangeSource = (_dir, _base, opts) => {
    onChange = opts.onChange;
    onError = opts.onError;
    return () => {
      onChange = null;
      onError = null;
    };
  };
  return {
    source,
    push: () => onChange?.(),
    fail: (err: unknown) => onError?.(err),
  };
}

describe('makeContentChannel', () => {
  it('yields initial content, re-yields on real change, dedupes no-op ticks, and stops on abort', async () => {
    const f = path.join(dir, 'live.md');
    await writeFile(f, 'one');
    const ac = new AbortController();
    const ps = createPushSource();
    const events: ContentEvent[] = [];

    const channel = makeContentChannel(f, ps.source);
    const consumer = (async () => {
      for await (const ev of channel({}, { signal: ac.signal })) events.push(ev);
    })();

    await flush();

    await writeFile(f, 'two');
    ps.push();
    await flush();

    ps.push(); // no file change → deduped, no event
    await flush();

    await writeFile(f, 'three');
    ps.push();
    await flush();

    ac.abort();
    await consumer;

    expect(events).toEqual([
      { kind: 'content', markdown: 'one' },
      { kind: 'content', markdown: 'two' },
      { kind: 'content', markdown: 'three' },
    ]);
  });

  it('yields a missing event when the file is deleted', async () => {
    const f = path.join(dir, 'del.md');
    await writeFile(f, 'here');
    const ac = new AbortController();
    const ps = createPushSource();
    const events: ContentEvent[] = [];

    const channel = makeContentChannel(f, ps.source);
    const consumer = (async () => {
      for await (const ev of channel({}, { signal: ac.signal })) events.push(ev);
    })();

    await flush();
    await rm(f);
    ps.push();
    await flush();

    ac.abort();
    await consumer;

    expect(events[0]).toEqual({ kind: 'content', markdown: 'here' });
    expect(events[1]).toEqual({ kind: 'missing' });
  });

  it('throws (surfaces) when the watcher reports an error', async () => {
    const f = path.join(dir, 'err.md');
    await writeFile(f, 'ok');
    const ac = new AbortController();
    const ps = createPushSource();
    const events: ContentEvent[] = [];

    const channel = makeContentChannel(f, ps.source);
    const consumer = (async () => {
      for await (const ev of channel({}, { signal: ac.signal })) events.push(ev);
    })();

    await flush();
    ps.fail(new Error('watch exploded'));

    await expect(consumer).rejects.toThrow('watch exploded');
    expect(events[0]).toEqual({ kind: 'content', markdown: 'ok' });
  });
});

/** Polls `pred` until true or the timeout elapses (for real-fs.watch timing). */
async function waitUntil(pred: () => boolean, timeoutMs: number): Promise<void> {
  const start = Date.now();
  while (!pred()) {
    if (Date.now() - start > timeoutMs) throw new Error('waitUntil timed out');
    await new Promise((r) => setTimeout(r, 10));
  }
}

describe('watchFile (real fs.watch via the default source)', () => {
  it('re-yields after a real edit and terminates the consumer on abort', async () => {
    const f = path.join(dir, 'real-watch.md');
    await writeFile(f, 'first');
    const ac = new AbortController();
    const events: ContentEvent[] = [];

    const channel = makeContentChannel(f); // default source = real watchFile
    const consumer = (async () => {
      for await (const ev of channel({}, { signal: ac.signal })) events.push(ev);
    })();

    await waitUntil(() => events.length >= 1, 3000);
    expect(events[0]).toEqual({ kind: 'content', markdown: 'first' });

    await writeFile(f, 'second');
    await waitUntil(() => events.length >= 2, 3000);
    expect(events[1]).toEqual({ kind: 'content', markdown: 'second' });

    ac.abort();
    await consumer; // resolves (no hang) because abort wakes the loop and closes the watcher
  });
});
