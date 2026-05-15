import type { FC } from 'react';

type Props = {
  displayOffset: number;
  historySize: number;
  viewportRows: number;
};

const THUMB_MIN_HEIGHT_PX = 16;

/**
 * Thin scrollbar overlay shown only when the user has scrolled into the
 * alacritty history. Hidden when displayOffset === 0.
 *
 * Read-only: dragging the thumb is not supported in this iteration.
 */
export const TerminalScrollbar: FC<Props> = ({ displayOffset, historySize, viewportRows }) => {
  if (displayOffset === 0 || historySize === 0) return null;
  const totalLines = historySize + viewportRows;
  const thumbHeightPct = Math.max(viewportRows / totalLines, THUMB_MIN_HEIGHT_PX / 300);
  // NOTE: thumb top = (history_size - displayOffset) / total
  const thumbTopPct = (historySize - displayOffset) / totalLines;

  return (
    <div className="pointer-events-none absolute top-0 right-0 h-full w-1 bg-muted/30">
      <div
        className="bg-foreground/40 absolute right-0 w-full rounded-sm"
        // biome-ignore lint/plugin: scrollbar thumb position requires dynamic CSS sizing
        style={{
          top: `${thumbTopPct * 100}%`,
          height: `${thumbHeightPct * 100}%`,
        }}
      />
    </div>
  );
};
