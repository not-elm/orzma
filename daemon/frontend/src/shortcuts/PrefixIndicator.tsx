import type { Prefix } from './wire';

interface PrefixIndicatorProps {
  armed: boolean;
  prefix: Prefix | null;
}

const NAMED_KEY_LABELS: Record<string, string> = {
  Escape: 'Esc',
  Space: '␣',
  Enter: '↵',
  Tab: '⇥',
  Backspace: '⌫',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
};

function keyLabel(key: string): string {
  if (key in NAMED_KEY_LABELS) return NAMED_KEY_LABELS[key];
  return key.length === 1 ? key.toUpperCase() : key;
}

function prefixLabel(prefix: Prefix): string {
  let label = '';
  // Order: Ctrl, Alt, Shift, Meta — matches macOS menu convention.
  if (prefix.modifiers.ctrl) label += '^';
  if (prefix.modifiers.alt) label += '⌥';
  if (prefix.modifiers.shift) label += '⇧';
  if (prefix.modifiers.meta) label += '⌘';
  return label + keyLabel(prefix.key);
}

export function PrefixIndicator({ armed, prefix }: PrefixIndicatorProps) {
  if (!armed || !prefix) return null;
  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed bottom-2 right-2 rounded bg-muted px-2 py-0.5 font-mono text-xs text-muted-foreground"
    >
      {prefixLabel(prefix)}
    </div>
  );
}
