export class DaemonError extends Error {
  readonly status: number;
  readonly bodyText: string;
  readonly path: string;

  constructor(status: number, bodyText: string, path: string) {
    super(`daemon ${path} → ${status}: ${bodyText}`);
    this.name = "DaemonError";
    this.status = status;
    this.bodyText = bodyText;
    this.path = path;
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

export async function getJson<T>(path: string): Promise<T> {
  const response = await fetch(buildUrl(path), {
    method: "GET",
    headers: buildHeaders(),
  });
  if (!response.ok) {
    const bodyText = await response.text();
    throw new DaemonError(response.status, bodyText, path);
  }
  return (await response.json()) as T;
}

export async function deleteNoContent(path: string): Promise<void> {
  const response = await fetch(buildUrl(path), {
    method: "DELETE",
    headers: buildHeaders(),
  });
  if (!response.ok) {
    const bodyText = await response.text();
    throw new DaemonError(response.status, bodyText, path);
  }
}

/**
 * Centralized URL builders for the hierarchical control plane introduced in
 * PR4/PR5. Keeping them here avoids per-call string interpolation drift across
 * the SDK class layer.
 */
export const paths = {
  session: (sid: string) => `/sessions/${sid}`,
  window: (wid: string) => `/windows/${wid}`,
  windowSelect: (wid: string) => `/windows/${wid}/select`,
  pane: (wid: string, pid: string) => `/windows/${wid}/panes/${pid}`,
  paneSplit: (wid: string, pid: string) =>
    `/windows/${wid}/panes/${pid}/split`,
  paneActivate: (wid: string, pid: string) =>
    `/windows/${wid}/panes/${pid}/activate`,
  paneActivities: (wid: string, pid: string) =>
    `/windows/${wid}/panes/${pid}/activities`,
  activity: (wid: string, pid: string, aid: string) =>
    `/windows/${wid}/panes/${pid}/activities/${aid}`,
  activityActivate: (wid: string, pid: string, aid: string) =>
    `/windows/${wid}/panes/${pid}/activities/${aid}/activate`,
};
