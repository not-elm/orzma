import { orzma } from '@orzma/web';
import { AddressBar, type Preview } from './address';
import { type Chrome, renderChrome } from './chrome';

/** A report to the controller (`page` event), tagged by `kind`. */
type PageEvent = { kind: 'cancel' } | { kind: 'openAddress' };

function emitPage(event: PageEvent): void {
  orzma.emit('page', event);
}

const root = document.getElementById('chrome') as HTMLElement;
const omnibox = root.querySelector('.omnibox') as HTMLElement;

const bar = new AddressBar(omnibox, {
  preview: (text) => orzma.call<Preview>('preview', { text }),
  submit: (text) => orzma.call<Preview>('submit', { text }),
  cancel: () => emitPage({ kind: 'cancel' }),
  openAddress: () => emitPage({ kind: 'openAddress' }),
});

orzma.on('chrome', (chrome: Chrome) => {
  renderChrome(root, chrome);
  bar.apply(chrome);
});

void orzma.call('ready').catch(console.error);
