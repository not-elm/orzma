//! IME preedit overlay — renders the in-progress composition string at the
//! cursor cell. Parent (Terminal.tsx) gates render on preedit !== ''.

import type { Cursor } from '../protocol/frame';
import type { FontMetrics } from '../renderer/font';

interface IMEProps {
  preedit: string;
  cursor: Cursor;
  fm: FontMetrics;
}

export function IME({ preedit, cursor, fm }: IMEProps) {
  return (
    <div
      // biome-ignore lint/plugin: preedit coords are cell × cellW/cellH
      style={{
        left: `${cursor.x * fm.cellW}px`,
        top: `${cursor.y * fm.cellH}px`,
      }}
      className="absolute pointer-events-none bg-muted text-foreground border-b border-foreground px-1"
    >
      {preedit}
    </div>
  );
}
