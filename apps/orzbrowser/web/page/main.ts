import { orzma } from '@orzma/web';
import { HeldKeys } from './held';
import { installKeys } from './keys';
import { installScroller, runScrollAction, type ScrollAction, windowClock } from './scroller';

/** A report to the controller (`page` event), tagged by `kind`. */
type PageEvent = { kind: 'ready' } | { kind: 'pending'; key: string | null };

function emitPage(event: PageEvent): void {
  orzma.emit('page', event);
}

function install(): void {
  const held = new HeldKeys();
  const scroller = installScroller(window, windowClock(window), held);
  const keys = installKeys(window, scroller, held, {
    reportPending: (key) => emitPage({ kind: 'pending', key }),
  });
  orzma.on('keys', (payload: { enabled: boolean }) => keys.setEnabled(payload.enabled));
  orzma.on('cancelChord', () => keys.cancelChord());
  orzma.on('blurInput', () => keys.blurFocusedInput());
  orzma.on('scroll', (payload: { action: ScrollAction }) =>
    runScrollAction(scroller, payload.action),
  );
  window.addEventListener('pageshow', (event) => {
    if (event.persisted) {
      emitPage({ kind: 'ready' });
    }
  });
  emitPage({ kind: 'ready' });
}

// NOTE: the preload scripts are evaluated as one joined script, so an exception that escaped
// here would stop every script after this one.
if (window === window.top) {
  try {
    install();
  } catch (error) {
    console.error(error);
  }
}
