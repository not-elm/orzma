/** A single preview update streamed from the server `content` channel to the client. */
export type ContentEvent =
  | { kind: 'content'; markdown: string }
  | { kind: 'missing' }
  | { kind: 'too-large'; bytes: number };
