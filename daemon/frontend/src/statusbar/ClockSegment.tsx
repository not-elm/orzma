import { useEffect, useState } from 'react';

const formatter = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
  hour12: false,
});

/**
 * `HH:MM` clock segment. Ticks once per second. Wrapped in
 * `aria-hidden` because the time is decorative — not interactive
 * information that screen readers should announce.
 */
export function ClockSegment() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(id);
  }, []);
  return (
    <span aria-hidden="true" className="tabular-nums" data-testid="clock">
      {formatter.format(now)}
    </span>
  );
}
