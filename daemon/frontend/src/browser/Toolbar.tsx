import { useState } from 'react';
import type { BrowserClientMsg } from './protocol/wire';

interface Props {
  url: string;
  canBack: boolean;
  canForward: boolean;
  loading: boolean;
  send: (m: BrowserClientMsg) => void;
}

/**
 * Toolbar with back / forward / reload-or-stop buttons and an editable URL
 * input. Pressing Enter in the URL input issues a navigate command.
 *
 * The draft URL state is seeded from `url` on mount and then owned by the
 * user — server-driven URL changes do not auto-reflect while the user may be
 * editing the input.
 */
export function Toolbar({ url, canBack, canForward, loading, send }: Props) {
  const [draft, setDraft] = useState(url);
  return (
    <div className="flex items-center gap-1 border-b border-border bg-background p-1">
      <button
        type="button"
        className="px-2 py-1 text-foreground disabled:opacity-50"
        disabled={!canBack}
        aria-label="Back"
        onClick={() => send({ kind: 'nav', nav: { kind: 'back' } })}
      >
        ←
      </button>
      <button
        type="button"
        className="px-2 py-1 text-foreground disabled:opacity-50"
        disabled={!canForward}
        aria-label="Forward"
        onClick={() => send({ kind: 'nav', nav: { kind: 'forward' } })}
      >
        →
      </button>
      <button
        type="button"
        className="px-2 py-1 text-foreground"
        aria-label={loading ? 'Stop' : 'Reload'}
        onClick={() => send({ kind: 'nav', nav: { kind: loading ? 'stop' : 'reload' } })}
      >
        {loading ? '×' : '⟳'}
      </button>
      <input
        type="text"
        value={draft}
        className="flex-1 rounded border border-border bg-background px-2 py-1 text-foreground"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            send({ kind: 'nav', nav: { kind: 'navigate', url: normalizeUrl(draft) } });
          }
        }}
      />
    </div>
  );
}

function normalizeUrl(input: string): string {
  if (/^[a-z]+:\/\//.test(input) || input.startsWith('about:')) return input;
  return `https://${input}`;
}
