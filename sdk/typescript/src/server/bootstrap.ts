import * as fs from 'node:fs/promises';
import * as net from 'node:net';
import * as path from 'node:path';
import { Writable } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { serveAssets } from './asset-server.ts';
import { fileAssetHandler } from './file-assets.ts';
import { bindHandlersServer } from './handlers-server.ts';
import { Pane } from './pane.ts';
import { type ClientFrame, encodeFrame, MAX_FRAME_PAYLOAD_BYTES } from './protocol.ts';
import { Session } from './session.ts';
import { assertCommandName, writeShim } from './shim-writer.ts';
import { Window } from './window.ts';

export interface BootstrapEnv {
  binDir: string;
  sockPath: string;
  extensionName: string;
  /** Extension-host control endpoint. Absent outside a running host (tests/dev); daemon-client write ops no-op when unset. */
  extensionHostUrl?: string;
  handlersSockPath: string;
  /** Dedicated asset socket; when set, bootstrap serves the extension dir over it. */
  assetSockPath?: string;
}

export function resolveBootstrapEnv(env: Record<string, string | undefined>): BootstrapEnv {
  const binDir = env.OZMUX_BIN_DIR;
  const sockPath = env.OZMUX_SOCK_PATH;
  const extensionName = env.EXTENSION_NAME;
  const handlersSockPath = env.OZMUX_HANDLERS_SOCK_PATH;
  for (const [k, v] of Object.entries({
    OZMUX_BIN_DIR: binDir,
    OZMUX_SOCK_PATH: sockPath,
    EXTENSION_NAME: extensionName,
    OZMUX_HANDLERS_SOCK_PATH: handlersSockPath,
  })) {
    if (!v) throw new Error(`missing required env: ${k}`);
  }
  return {
    binDir: binDir!,
    sockPath: sockPath!,
    extensionName: extensionName!,
    extensionHostUrl: env.OZMUX_EXTENSION_HOST_URL,
    handlersSockPath: handlersSockPath!,
    assetSockPath: env.OZMUX_ASSET_SOCK_PATH,
  };
}

export interface BindServerOptions {
  maxConnections?: number;
}

export async function bindServer(
  sockPath: string,
  onConnection: (conn: net.Socket) => void,
  options: BindServerOptions = {},
): Promise<net.Server> {
  await fs.unlink(sockPath).catch(() => {});
  const server = net.createServer(onConnection);
  if (options.maxConnections !== undefined) {
    server.maxConnections = options.maxConnections;
  }
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(sockPath, () => {
      server.off('error', reject);
      resolve();
    });
  });
  await fs.chmod(sockPath, 0o600);
  return server;
}

export interface MaterializeShimsArgs {
  binDir: string;
  sockPath: string;
  commandNames: string[];
  execPath: string;
  helperPath: string;
}

export async function materializeShims(args: MaterializeShimsArgs): Promise<void> {
  await fs.mkdir(args.binDir, { recursive: true, mode: 0o700 });
  await fs.chmod(args.binDir, 0o700);
  for (const name of args.commandNames) {
    await writeShim({
      filePath: path.join(args.binDir, name),
      execPath: args.execPath,
      helperPath: args.helperPath,
      socketPath: args.sockPath,
      commandName: name,
    });
  }
}

export interface CommandContext {
  argv: string[];
  cwd: string;
  stdout: Writable;
  stderr: Writable;
  signal: AbortSignal;
  /** The Pane this command was invoked from. */
  pane: Pane;
  /** The Window containing the invoking Pane. */
  window: Window;
  /** Owning Session, or `null` for orphan Windows. */
  session: Session | null;
}

export type CommandHandler = (ctx: CommandContext) => Promise<number | undefined>;

function chunkWriter(kind: 'stdout' | 'stderr', target: Writable): Writable {
  return new Writable({
    write(c: Buffer, _enc, cb) {
      let offset = 0;
      while (offset < c.length) {
        const slice = c.subarray(offset, offset + MAX_FRAME_PAYLOAD_BYTES);
        const ok = target.write(encodeFrame({ type: kind, data: slice.toString('base64') }));
        offset += slice.length;
        if (!ok) return target.once('drain', () => cb());
      }
      cb();
    },
  });
}

export async function handleConnection(
  socket: Writable,
  handlers: Record<string, CommandHandler>,
  parseLine: (line: string) => ClientFrame,
  firstLine: string,
): Promise<void> {
  const frame = parseLine(firstLine);
  if (frame.type !== 'invoke') {
    socket.write(encodeFrame({ type: 'exit', code: 2 }));
    return;
  }
  const handler = handlers[frame.command];
  if (!handler) {
    socket.write(
      encodeFrame({
        type: 'stderr',
        data: Buffer.from(`ozmux: unknown command '${frame.command}'\n`).toString('base64'),
      }),
    );
    socket.write(encodeFrame({ type: 'exit', code: 127 }));
    return;
  }
  const stdout = chunkWriter('stdout', socket);
  const stderr = chunkWriter('stderr', socket);
  const ac = new AbortController();

  // The PTY env carries the addressing tuple; bail early if anything required
  // is missing rather than letting the handler hit broken Pane methods.
  const paneId = frame.env.OZMUX_PANE_ID ?? '';
  const sessionId = frame.env.OZMUX_SESSION_ID ?? null;
  const windowId = sessionId ?? '';
  if (!paneId || !sessionId) {
    stderr.write(Buffer.from('ozmux: missing OZMUX_PANE_ID / OZMUX_SESSION_ID\n'));
    socket.write(encodeFrame({ type: 'exit', code: 2 }));
    return;
  }
  const pane = new Pane({ id: paneId, windowId, sessionId });
  const window = new Window({ id: windowId, name: '', sessionId });
  const session = sessionId ? new Session({ id: sessionId, name: '' }) : null;

  const ctx: CommandContext = {
    argv: frame.argv,
    cwd: frame.cwd,
    stdout,
    stderr,
    signal: ac.signal,
    pane,
    window,
    session,
  };
  let exitCode = 0;
  try {
    const result = await handler(ctx);
    exitCode = typeof result === 'number' ? result : 0;
  } catch (err) {
    const stack = err instanceof Error ? (err.stack ?? err.message) : String(err);
    socket.write(
      encodeFrame({
        type: 'stderr',
        data: Buffer.from(`${stack}\n`).toString('base64'),
      }),
    );
    exitCode = 1;
  }
  socket.write(encodeFrame({ type: 'exit', code: exitCode }));
}

export interface BootstrapArgs {
  commands: Record<string, CommandHandler>;
}

export async function bootstrap(args: BootstrapArgs): Promise<void> {
  const env = resolveBootstrapEnv(process.env);
  for (const name of Object.keys(args.commands)) assertCommandName(name);
  const helperPath = fileURLToPath(import.meta.resolve('./cmd-shim.ts'));
  await materializeShims({
    binDir: env.binDir,
    sockPath: env.sockPath,
    commandNames: Object.keys(args.commands),
    execPath: process.execPath,
    helperPath,
  });

  const server = await bindServer(env.sockPath, (conn) => {
    let buffer = '';
    let dispatched = false;
    conn.on('data', async (chunk: Buffer) => {
      if (dispatched) return;
      buffer += chunk.toString('utf8');
      const idx = buffer.indexOf('\n');
      if (idx === -1) return;
      const line = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 1);
      dispatched = true;
      let frame: ClientFrame;
      try {
        frame = JSON.parse(line);
      } catch {
        conn.write(encodeFrame({ type: 'exit', code: 2 }));
        conn.end();
        return;
      }
      try {
        await handleConnection(conn, args.commands, () => frame, line);
      } finally {
        conn.end();
      }
    });
  });

  const handlersServer = await bindHandlersServer(env.handlersSockPath);
  // NOTE: process.cwd() is the extension dir — CommandExtension spawns node
  // with the extension root as cwd, making it the correct asset root.
  const assetServer = env.assetSockPath
    ? serveAssets(fileAssetHandler(process.cwd()), { sockPath: env.assetSockPath })
    : undefined;

  let cleanupPromise: Promise<void> | undefined;
  const cleanup = (): Promise<void> => {
    cleanupPromise ??= (async () => {
      await Promise.all([
        new Promise<void>((res) => server.close(() => res())),
        new Promise<void>((res) => handlersServer.close(() => res())),
        Promise.resolve(assetServer?.close()),
      ]);
      await Promise.all([
        fs.rm(env.binDir, { recursive: true, force: true }),
        fs.unlink(env.sockPath).catch(() => {}),
        fs.unlink(env.handlersSockPath).catch(() => {}),
        ...(env.assetSockPath ? [fs.unlink(env.assetSockPath).catch(() => {})] : []),
      ]);
      // NOTE: required for the SIGTERM/SIGINT and inherited-stdin paths —
      // server.close() releases the server refs but process.stdin.resume()
      // still pins the event loop. Without destroy() the process hangs
      // after cleanup completes.
      process.stdin.destroy();
    })();
    return cleanupPromise;
  };

  for (const sig of ['SIGTERM', 'SIGINT'] as const) {
    process.once(sig, () => {
      void cleanup();
    });
  }
  // NOTE: extension stdin is reserved by the SDK as a parent-death channel —
  // any other consumer that reads stdin will steal the EOF and break shutdown.
  process.stdin.once('end', () => {
    void cleanup();
  });
  process.stdin.resume();
}
