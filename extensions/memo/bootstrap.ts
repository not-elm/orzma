import { fileURLToPath } from 'node:url';
import { abortableSleep, bootstrap } from '@ozmux/sdk/server';

bootstrap({
  commands: {
    '@memo': async (ctx) => {
      ctx.stdout.write(`memo invoked in pane ${ctx.pane.id}\n`);

      await ctx.pane.split({
        orientation: 'vertical',
        side: 'after',
        surface: {
          kind: 'extension',
          cwd: ctx.cwd,
          html: fileURLToPath(new URL('./index.html', import.meta.url)),
          handlers: {
            greet: async ({ name }: { name: string }) => ({
              message: `Hello, ${name}!`,
            }),
          },
          channels: {
            clock: async function* (
              { intervalMs }: { intervalMs: number },
              { signal }: { signal: AbortSignal },
            ) {
              yield { time: new Date().toISOString() };
              while (!signal.aborted) {
                await abortableSleep(intervalMs, signal);
                if (signal.aborted) return;
                yield { time: new Date().toISOString() };
              }
            },
          },
        },
      });

      return 0;
    },
  },
});
