//! Selection overlay — 1 to 3 absolute-positioned rect divs covering a
//! cell range, normalized to topological order. Half-open at col (head
//! is exclusive), inclusive at row.

import type { SelectionRange } from '../input/pointer-overlays';
import type { FontMetrics } from '../renderer/font';

interface SelectionProps {
  selection: SelectionRange;
  cols: number;
  fm: FontMetrics;
}

function orderTopologically(
  anchor: { col: number; row: number },
  head: { col: number; row: number },
): [{ col: number; row: number }, { col: number; row: number }] {
  if (anchor.row < head.row || (anchor.row === head.row && anchor.col <= head.col)) {
    return [anchor, head];
  }
  return [head, anchor];
}

export function Selection({ selection, cols, fm }: SelectionProps) {
  const [start, end] = orderTopologically(selection.anchor, selection.head);

  if (start.row === end.row && start.col === end.col) return null;

  const rects: Array<{ left: number; top: number; width: number; height: number }> = [];

  if (start.row === end.row) {
    rects.push({
      left: start.col * fm.cellW,
      top: start.row * fm.cellH,
      width: (end.col - start.col) * fm.cellW,
      height: fm.cellH,
    });
  } else {
    rects.push({
      left: start.col * fm.cellW,
      top: start.row * fm.cellH,
      width: (cols - start.col) * fm.cellW,
      height: fm.cellH,
    });
    if (end.row - start.row > 1) {
      rects.push({
        left: 0,
        top: (start.row + 1) * fm.cellH,
        width: cols * fm.cellW,
        height: (end.row - start.row - 1) * fm.cellH,
      });
    }
    rects.push({
      left: 0,
      top: end.row * fm.cellH,
      width: end.col * fm.cellW,
      height: fm.cellH,
    });
  }

  return (
    <>
      {rects.map((r, i) => (
        <div
          key={`${r.top}-${r.left}`}
          data-rect={i}
          // biome-ignore lint/plugin: rect coords computed from cell × cellW/cellH
          style={{
            left: `${r.left}px`,
            top: `${r.top}px`,
            width: `${r.width}px`,
            height: `${r.height}px`,
          }}
          className="absolute pointer-events-none bg-primary opacity-30"
        />
      ))}
    </>
  );
}
