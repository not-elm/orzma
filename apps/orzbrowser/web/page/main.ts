import { orzma } from '@orzma/web';
import { installFrames, parentHandOff } from './frames';
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
    ...parentHandOff(window),
  });
  const relay = installFrames(window, keys);
  if (window !== window.top) {
    return;
  }
  orzma.on('keys', ({ enabled }: { enabled: boolean }) => relay({ kind: 'keys', enabled }));
  orzma.on('cancelChord', () => relay({ kind: 'cancelChord' }));
  orzma.on('blurInput', () => relay({ kind: 'blurInput' }));
  orzma.on('scroll', (payload: { action: ScrollAction }) => {
    runScrollAction(scroller, payload.action);
  });
  window.addEventListener('pageshow', (event) => {
    if (event.persisted) {
      relay({ kind: 'keys', enabled: false });
      emitPage({ kind: 'ready' });
    }
  });
  emitPage({ kind: 'ready' });
}

// NOTE: the preload scripts are evaluated as one joined script, so an exception that escaped
// here would stop every script after this one.
try {
  install();
} catch (error) {
  console.error(error);
}
