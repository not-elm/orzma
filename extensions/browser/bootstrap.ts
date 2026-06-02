import { bootstrap } from '@ozmux/sdk/server';

bootstrap({
  commands: {
    '@browser': async (ctx) => {
      const [subcommand, ...rest] = ctx.argv;
      if (subcommand !== 'open') {
        ctx.stderr.write('usage: @browser open <url-or-search-words>\n');
        return 1;
      }
      const input = rest.join(' ').trim();
      if (input.length === 0) {
        ctx.stderr.write('usage: @browser open <url-or-search-words>\n');
        return 1;
      }

      await ctx.pane.split({
        orientation: 'vertical',
        side: 'after',
        surface: { kind: 'browser', url: input },
      });

      return 0;
    },
  },
});
