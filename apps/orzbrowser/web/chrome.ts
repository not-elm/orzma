/** Mode the controller is in, as the chrome shows it. */
type ChromeMode = 'normal' | 'insert' | 'hint' | 'address' | 'help';

/** The chrome state the controller pushes with the `chrome` event. */
export interface Chrome {
  /** Current mode. */
  mode: ChromeMode;
  /** URL loaded in the page webview. */
  url: string;
  /** First key of a pending two-key chord, or `null`. */
  pendingKey: string | null;
  /** Text the address input starts with when the address bar opens. */
  seed: string;
  /** Incremented each time the address bar opens. */
  addressEpoch: number;
}

/** The parts of a URL the omnibox draws. */
interface UrlParts {
  /** Whether the URL is plain `http`. */
  insecure: boolean;
  /** Host, with the port when it is not the default one. */
  host: string;
  /** Path, query and fragment; empty for the bare root path. */
  rest: string;
}

const BADGES: Record<ChromeMode, string> = {
  normal: 'NORMAL',
  insert: 'INSERT',
  hint: 'HINT',
  address: 'OPEN',
  help: 'HELP',
};

const KEY_HINTS: Record<ChromeMode, readonly (readonly [string, string])[]> = {
  normal: [
    ['o', 'open'],
    ['f', 'hint'],
    ['?', ''],
  ],
  insert: [['esc', 'normal']],
  hint: [['esc', 'cancel']],
  address: [['esc', 'cancel']],
  help: [],
};

/** Splits `url` into the parts the omnibox draws; a URL that does not parse is all `rest`. */
export function splitUrl(url: string): UrlParts {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { insecure: false, host: '', rest: url };
  }
  const host = parsed.port === '' ? parsed.hostname : `${parsed.hostname}:${parsed.port}`;
  const path = parsed.pathname === '/' ? '' : parsed.pathname;
  return {
    insecure: parsed.protocol === 'http:',
    host,
    rest: `${path}${parsed.search}${parsed.hash}`,
  };
}

/** Draws `chrome` into `root`: the mode class and badge, the pending key, the URL and the key hints. */
export function renderChrome(root: HTMLElement, chrome: Chrome): void {
  root.className = `mode-${chrome.mode}`;
  setText(root, '.badge', BADGES[chrome.mode]);
  const pending = root.querySelector<HTMLElement>('.pending');
  if (pending !== null) {
    pending.hidden = chrome.pendingKey === null;
    pending.textContent = chrome.pendingKey ?? '';
  }
  const parts = splitUrl(chrome.url);
  const notSecure = root.querySelector<HTMLElement>('.not-secure');
  if (notSecure !== null) {
    notSecure.hidden = !parts.insecure;
  }
  setText(root, '.host', parts.host);
  setText(root, '.rest', parts.rest);
  root
    .querySelector('.hints')
    ?.replaceChildren(...KEY_HINTS[chrome.mode].map(([key, label]) => keyHint(key, label)));
}

function setText(root: HTMLElement, selector: string, text: string): void {
  const el = root.querySelector(selector);
  if (el !== null) {
    el.textContent = text;
  }
}

function keyHint(key: string, label: string): HTMLElement {
  const hint = document.createElement('span');
  const kbd = document.createElement('kbd');
  kbd.textContent = key;
  hint.append(kbd);
  if (label !== '') {
    hint.append(` ${label}`);
  }
  return hint;
}
