/** Visual state of `SessionSegment`. */
export type SessionSegmentState =
  | { status: 'loading' }
  | { status: 'ready'; name: string }
  | { status: 'gone'; reason: string };

interface SessionSegmentProps {
  state: SessionSegmentState;
}

/**
 * Left segment of the status bar. Shows the session name, a loading
 * indicator, or a "gone" message depending on transport state.
 */
export function SessionSegment({ state }: SessionSegmentProps) {
  switch (state.status) {
    case 'loading':
      return <span className="max-w-56 truncate italic text-muted-foreground">Loading…</span>;
    case 'ready':
      return (
        <span className="max-w-56 truncate font-semibold text-tmux-status-bar-foreground">
          {state.name}
        </span>
      );
    case 'gone':
      return (
        <span className="max-w-56 truncate text-destructive">Session is gone ({state.reason})</span>
      );
  }
}
