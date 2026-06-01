import { useEffect, useMemo, useState } from 'react';
import type { ContentEvent } from '../content-event.ts';
import { Preview } from './Preview.tsx';

/** Subscribes to the `content` channel and renders the current preview state. */
export function App() {
  const client = useMemo(() => window.ozmux.createClient(), []);
  const [event, setEvent] = useState<ContentEvent | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const ac = new AbortController();
    void (async () => {
      try {
        for await (const ev of client.subscribe<Record<string, never>, ContentEvent>(
          'content',
          {},
          { signal: ac.signal },
        )) {
          setEvent(ev);
        }
      } catch (e) {
        // NOTE: aborting resolves the iterator cleanly (no throw), so reaching here
        // means a real channel error — surface it rather than freezing on stale content.
        if (!ac.signal.aborted) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => ac.abort();
  }, [client]);

  if (error !== null) return <div className="md-status">preview error: {error}</div>;
  if (event === undefined) return <div className="md-status">Loading…</div>;
  if (event.kind === 'missing') return <div className="md-status">file not found</div>;
  if (event.kind === 'too-large') {
    return <div className="md-status">file too large to preview ({event.bytes} bytes)</div>;
  }
  return <Preview markdown={event.markdown} />;
}
