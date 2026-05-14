//! Cursor overlay — DOM-rendered cursor with CSS step-end blink animation.
//! Replaces the in-canvas drawCursor that Phase 2B used.

import { clsx } from 'clsx';
import type { Cursor as CursorType } from '../protocol/frame';
import type { FontMetrics } from '../renderer/font';

interface CursorProps {
  cursor: CursorType;
  isActive: boolean;
  fm: FontMetrics;
}

export function Cursor({ cursor, isActive, fm }: CursorProps) {
  if (!cursor.visible) return null;

  const width = cursor.shape === 'bar' ? 2 : fm.cellW;
  const height = cursor.shape === 'underline' ? 2 : fm.cellH;
  const top =
    cursor.shape === 'underline' ? cursor.y * fm.cellH + (fm.cellH - 2) : cursor.y * fm.cellH;

  return (
    <div
      // biome-ignore lint/plugin: positioning requires computed px (cell × cellW/cellH)
      style={{
        left: `${cursor.x * fm.cellW}px`,
        top: `${top}px`,
        width: `${width}px`,
        height: `${height}px`,
      }}
      className={clsx(
        'absolute pointer-events-none',
        isActive ? 'bg-foreground' : 'bg-muted-foreground',
        isActive && cursor.blinking && 'animate-cursor-blink',
        !isActive && 'opacity-60',
      )}
      data-testid="vt-cursor"
    />
  );
}
