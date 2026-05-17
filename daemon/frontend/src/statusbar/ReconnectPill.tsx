interface ReconnectPillProps {
  visible: boolean;
}

/**
 * Inline pill shown in the status bar while any underlying WS is
 * reconnecting. Mounts/unmounts based on `visible` so screen readers
 * only announce once per reconnect cycle.
 */
export function ReconnectPill({ visible }: ReconnectPillProps) {
  if (!visible) return null;
  return (
    <span
      role="status"
      aria-live="polite"
      className="rounded bg-warning/20 px-2 py-0.5 text-xs text-warning"
    >
      Reconnecting…
    </span>
  );
}
