export class DaemonError extends Error {
  constructor(
    public readonly status: number,
    public readonly bodyText: string,
    public readonly path: string,
  ) {
    super(`daemon ${path} → ${status}: ${bodyText}`);
    this.name = "DaemonError";
  }
}

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`missing required env: ${name}`);
  return value;
}

function buildHeaders(): Record<string, string> {
  return {
    "Content-Type": "application/json",
    "X-Ozmux-Extension": requireEnv("EXTENSION_NAME"),
  };
}

function buildUrl(path: string): string {
  return `${requireEnv("OZMUX_DAEMON_URL")}${path}`;
}

async function send(path: string, body: unknown): Promise<Response> {
  const response = await fetch(buildUrl(path), {
    method: "POST",
    headers: buildHeaders(),
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    const bodyText = await response.text();
    throw new DaemonError(response.status, bodyText, path);
  }
  return response;
}

export async function postJson<T>(path: string, body: unknown): Promise<T> {
  const response = await send(path, body);
  return (await response.json()) as T;
}

export async function postNoContent(path: string, body: unknown): Promise<void> {
  await send(path, body);
}
