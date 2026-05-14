//! Link hover overlay — underline at the cell range of a hovered hyperlink.

import type { LinkHover } from '../input/pointer-overlays';
import type { FontMetrics } from '../renderer/font';

interface LinkProps {
  hover: LinkHover;
  fm: FontMetrics;
}

export function Link({ hover, fm }: LinkProps) {
  return (
    <div
      // biome-ignore lint/plugin: cell × cellW/cellH px coords
      style={{
        left: `${hover.rangeStart * fm.cellW}px`,
        top: `${hover.row * fm.cellH}px`,
        width: `${(hover.rangeEnd - hover.rangeStart) * fm.cellW}px`,
        height: `${fm.cellH}px`,
      }}
      className="absolute pointer-events-none border-b border-primary"
      data-uri={hover.uri}
    />
  );
}
