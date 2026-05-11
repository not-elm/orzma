export interface InvokeFrame {
  type: "invoke";
  command: string;
  argv: string[];
  cwd: string;
  env: Record<string, string>;
}

export interface SignalFrame {
  type: "signal";
  signal: "SIGINT";
}

export type ClientFrame = InvokeFrame | SignalFrame;

export interface StdoutFrame { type: "stdout"; data: string }   // base64
export interface StderrFrame { type: "stderr"; data: string }   // base64
export interface ExitFrame   { type: "exit";   code: number }

export type ServerFrame = StdoutFrame | StderrFrame | ExitFrame;

export const MAX_FRAME_PAYLOAD_BYTES = 64 * 1024;

export function encodeFrame(frame: ClientFrame | ServerFrame): Buffer {
  return Buffer.from(JSON.stringify(frame) + "\n", "utf8");
}

// Handler RPC frames (browser ↔ daemon ↔ extension)
// Reserved frame kinds are flat union members; same shape on WS and inside the
// daemon's `{aid, frame: <here>}` UDS envelope.

export interface HandlerCallFrame {
  kind: "call";
  id: string;
  name: string;
  payload: unknown;
}
export interface HandlerResultFrame {
  kind: "result";
  id: string;
  payload: unknown;
}
export interface HandlerErrorFrame {
  kind: "error";
  id: string;
  code: string;
  message: string;
}
export interface HandlerPushFrame {
  kind: "push";
  topic: string;
  payload: unknown;
}

export type HandlerClientFrame = HandlerCallFrame;
export type HandlerServerFrame =
  | HandlerResultFrame
  | HandlerErrorFrame
  | HandlerPushFrame;

/** NDJSON envelope written by the daemon onto the handlers UDS. */
export interface HandlerUdsEnvelope {
  aid: string;
  frame: HandlerClientFrame | HandlerServerFrame;
}

export const MAX_HANDLER_FRAME_BYTES = 1 << 20; // 1 MiB
