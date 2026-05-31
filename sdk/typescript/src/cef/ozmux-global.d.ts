/**
 * Ambient declaration of the host-injected `window.ozmux` global for extension
 * frontends. Reference it from an extension's tsconfig
 * (`"types": ["@ozmux/sdk/ozmux-global"]`) with the DOM lib enabled
 * (`"lib": ["DOM", "ES2023"]`) to type `window.ozmux` in authored `.ts` frontends.
 */
import type { Client, OzmuxContext, SubscribeOptions } from './client.ts';

declare global {
  interface Window {
    ozmux: {
      call<Req, Resp>(name: string, payload: Req): Promise<Resp>;
      subscribe<Params, Event>(
        name: string,
        params: Params,
        opts?: SubscribeOptions,
      ): AsyncIterable<Event>;
      createClient(): Client;
      readonly context: OzmuxContext;
    };
  }
}
