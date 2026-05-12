interface PrefixIndicatorProps {
  armed: boolean;
}

export function PrefixIndicator({ armed }: PrefixIndicatorProps) {
  if (!armed) return null;
  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed bottom-2 right-2 rounded bg-muted px-2 py-0.5 font-mono text-xs text-muted-foreground"
    >
      ^B
    </div>
  );
}
