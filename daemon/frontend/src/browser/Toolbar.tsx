import { useState } from 'react';

interface Props {
  /** Server-driven URL, used to seed the draft input. */
  url: string;
  /** `false` greys out the back button. Defaults to `true` for callers that
   *  don't track NavStateChanged. */
  canBack?: boolean;
  /** Same as `canBack` for forward. */
  canForward?: boolean;
  /** Fires when the user clicks the back button. */
  onBack: () => void;
  /** Fires when the user clicks the forward button. */
  onForward: () => void;
  /** Fires when the user clicks the reload button. */
  onReload: () => void;
  /** Fires when the user presses Enter in the URL input. URL is already
   *  normalized (scheme prepended if missing). */
  onGo: (url: string) => void;
}

/**
 * Toolbar with back / forward / reload buttons and an editable URL input.
 * Pressing Enter in the URL input issues a navigate command.
 *
 * The draft URL state is seeded from `url` on mount and then owned by the
 * user — server-driven URL changes do not auto-reflect while the user may be
 * editing the input. Callers decide how to translate the four callbacks to
 * their backend's wire protocol.
 */
export function Toolbar({
  url,
  canBack = true,
  canForward = true,
  onBack,
  onForward,
  onReload,
  onGo,
}: Props) {
  const [draft, setDraft] = useState(url);
  return (
    <div className="flex items-center gap-1 border-b border-border bg-background p-1">
      <button
        type="button"
        className="px-2 py-1 text-foreground disabled:opacity-50"
        aria-label="Back"
        disabled={!canBack}
        onClick={onBack}
      >
        ←
      </button>
      <button
        type="button"
        className="px-2 py-1 text-foreground disabled:opacity-50"
        aria-label="Forward"
        disabled={!canForward}
        onClick={onForward}
      >
        →
      </button>
      <button
        type="button"
        className="px-2 py-1 text-foreground"
        aria-label="Reload"
        onClick={onReload}
      >
        ⟳
      </button>
      <input
        type="text"
        value={draft}
        className="flex-1 rounded border border-border bg-background px-2 py-1 text-foreground"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            onGo(normalizeUrl(draft));
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
