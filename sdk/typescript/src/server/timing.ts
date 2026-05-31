/**
 * Resolve after `ms` milliseconds, or immediately if `signal` is aborted.
 * Removes the abort listener on the timeout path so callers can reuse a
 * long-lived AbortSignal across many sleeps without leaking listeners.
 */
export function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.resolve();
  return new Promise((resolve) => {
    const onAbort = () => {
      clearTimeout(t);
      resolve();
    };
    const t = setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}
