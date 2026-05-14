//! TerminalGrid — subscribes to grid-store and renders each row via <Row>.
//! aria-hidden + role=presentation: native screen readers ignore the visual
//! grid until the AccessibilityManager phase adds a parallel a11y tree (R8).

import { useRef } from 'react';
import type { FontMetrics } from './font';
import { useGridStore } from './grid-store';
import { Row } from './Row';

interface TerminalGridProps {
  fm: FontMetrics;
  hyperlinks: ReadonlyMap<number, string>;
}

export function TerminalGrid({ fm, hyperlinks }: TerminalGridProps) {
  const grid = useGridStore();
  const probeRef = useRef<HTMLDivElement | null>(null);
  return (
    <div
      ref={probeRef}
      role="presentation"
      aria-hidden="true"
      className="terminal-grid font-mono whitespace-pre leading-none select-text cursor-text text-foreground"
    >
      {grid.cells.map((cells, i) => (
        <Row
          // biome-ignore lint/suspicious/noArrayIndexKey: row index is the stable identity per grid row
          key={i}
          cells={cells}
          version={grid.rowVersions[i] ?? 0}
          fm={fm}
          hyperlinks={hyperlinks}
          probeRef={probeRef.current}
        />
      ))}
    </div>
  );
}
