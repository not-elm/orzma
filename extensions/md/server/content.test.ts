import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import * as path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import type { ContentEvent } from '../content-event.ts';
import { MAX_BYTES, makeContentChannel, readContent, signatureOf } from './content.ts';

const dir = await mkdtemp(path.join(tmpdir(), 'md-content-'));
afterAll(async () => rm(dir, { recursive: true, force: true }));

const flush = () => new Promise<void>((r) => setTimeout(r, 15));

describe('readContent', () => {
  it('returns the UTF-8 text for a normal file', async () => {
    const f = path.join(dir, 'a.md');
    await writeFile(f, '# hi');
    expect(await readContent(f)).toEqual({ kind: 'content', markdown: '# hi' });
  });

  it('returns missing for a non-existent file', async () => {
    expect(await readContent(path.join(dir, 'nope.md'))).toEqual({ kind: 'missing' });
  });

  it('returns too-large above the cap', async () => {
    const f = path.join(dir, 'big.md');
    await writeFile(f, 'x'.repeat(MAX_BYTES + 1));
    const ev = await readContent(f);
    expect(ev.kind).toBe('too-large');
    if (ev.kind === 'too-large') expect(ev.bytes).toBe(MAX_BYTES + 1);
  });
});

describe('signatureOf', () => {
  it('changes when the file content changes', async () => {
    const f = path.join(dir, 'sig.md');
    await writeFile(f, 'one');
    const a = await signatureOf(f);
    await writeFile(f, 'one and two');
    const b = await signatureOf(f);
    expect(a).not.toBe(b);
  });

  it('returns empty string for a missing file', async () => {
    expect(await signatureOf(path.join(dir, 'gone.md'))).toBe('');
  });
});

/** A manually-driven {@link ChangeSource}: `push()` emits one tick. */
function createPushSource() {
  const buf: undefined[] = [];
  let wake: (() => void) | null = null;
  const source = async function* (
    _dir: string,
    _base: string,
    { signal }: { signal: AbortSignal },
  ): AsyncGenerator<void, void, undefined> {
    while (!signal.aborted) {
      if (buf.length === 0) {
        await new Promise<void>((resolve) => {
          wake = resolve;
        });
      }
      if (signal.aborted) return;
      buf.shift();
      yield;
    }
  };
  const push = () => {
    buf.push(undefined);
    const w = wake;
    wake = null;
    w?.();
  };
  return { source, push };
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

    ps.push();
    await flush();

    await writeFile(f, 'three');
    ps.push();
    await flush();

    ac.abort();
    ps.push();
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
    ps.push();
    await consumer;

    expect(events[0]).toEqual({ kind: 'content', markdown: 'here' });
    expect(events[1]).toEqual({ kind: 'missing' });
  });
});
