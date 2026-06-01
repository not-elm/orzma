import { fileURLToPath } from 'node:url';
import { bootstrap } from '@ozmux/sdk/server';
import { mdCommand } from './server/command.ts';
import { makeContentChannel } from './server/content.ts';

const distIndexPath = fileURLToPath(new URL('./dist/index.html', import.meta.url));

bootstrap({
  commands: {
    '@md': (ctx) => mdCommand(ctx, { distIndexPath, makeChannel: makeContentChannel }),
  },
});
