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
