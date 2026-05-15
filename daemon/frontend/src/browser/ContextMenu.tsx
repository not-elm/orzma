import type { ReactNode } from 'react';
import { useEffect } from 'react';
import { pasteFromOs, requestCopy } from './input/clipboard';
import type { BrowserClientMsg } from './protocol/wire';

interface Props {
  x: number;
  y: number;
  send: (m: BrowserClientMsg) => void;
  onClose: () => void;
}

/**
 * Frontend-drawn context menu for the browser activity. Closes on any outside
 * click. Anchored to the click position via inline style — the only place in
 * this component where inline style is used, and an explicit escape hatch per
 * `.claude/rules/styling.md` (runtime-computed pointer coordinates cannot be
 * expressed as static Tailwind utilities).
 */
export function ContextMenu({ x, y, send, onClose }: Props) {
  useEffect(() => {
    const dismiss = () => onClose();
    document.addEventListener('click', dismiss);
    return () => document.removeEventListener('click', dismiss);
  }, [onClose]);

  return (
    <div
      className="absolute z-50 rounded border border-border bg-background shadow"
      // biome-ignore lint/plugin: anchored to runtime-computed pointer coordinates — cannot use static Tailwind utilities for arbitrary x/y position
      style={{ left: x, top: y }}
    >
      <Item onClick={() => send({ kind: 'nav', nav: { kind: 'back' } })}>Back</Item>
      <Item onClick={() => send({ kind: 'nav', nav: { kind: 'forward' } })}>Forward</Item>
      <Item onClick={() => send({ kind: 'nav', nav: { kind: 'reload' } })}>Reload</Item>
      <hr className="border-border" />
      <Item onClick={() => requestCopy(send)}>Copy</Item>
      <Item onClick={() => pasteFromOs(send)}>Paste</Item>
    </div>
  );
}

interface ItemProps {
  children: ReactNode;
  onClick: () => void;
}

function Item({ children, onClick }: ItemProps) {
  return (
    <button
      type="button"
      className="block w-full px-3 py-1 text-left text-foreground hover:bg-muted"
      onClick={onClick}
    >
      {children}
    </button>
  );
}
