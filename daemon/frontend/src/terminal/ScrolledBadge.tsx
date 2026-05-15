import type { FC } from 'react';

type Props = {
  displayOffset: number;
  onResume: () => void;
};

/**
 * Badge shown at the pane's bottom-left while the viewport is scrolled
 * back. Clicking resumes the live tail.
 */
export const ScrolledBadge: FC<Props> = ({ displayOffset, onResume }) => {
  if (displayOffset === 0) return null;
  return (
    <button
      type="button"
      onClick={onResume}
      className="bg-accent text-accent-foreground hover:bg-accent/80 absolute bottom-2 left-2 cursor-pointer rounded-md px-2 py-1 text-xs shadow-sm"
    >
      {displayOffset} lines back · click to resume
    </button>
  );
};
