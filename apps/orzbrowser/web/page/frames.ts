import { isScrollAction, type ScrollAction } from '@orzma/scroller';
import type { KeyHandler } from './keys';

/** The parts of a window the frames talk through. */
interface FrameWindow extends EventTarget {
  readonly parent: FrameWindow;
  readonly closed: boolean;
  postMessage(message: unknown, targetOrigin: string): void;
}

/** A host message, which the top frame's page script passes down to every frame below. */
type HostMessage =
  | { kind: 'keys'; enabled: boolean }
  | { kind: 'cancelChord' }
  | { kind: 'blurInput' };

/** A message from the page script in a frame to the one in its parent frame. */
type ToParent =
  | { kind: 'hello' }
  | { kind: 'scroll'; action: ScrollAction; code: string }
  | { kind: 'release'; code: string };

const CHANNEL = 'orzbrowser';

/**
 * Links the page script in `win` with the ones in its parent and child frames, which is how the
 * host's messages to the top frame reach every frame. On install it asks the parent frame for the
 * state of the keys. It takes the parent's messages only from the parent and the children's only
 * from a child, and hides every message on its channel from the page's listeners.
 *
 * Returns the function that applies a host message here and passes it to every frame below; a
 * `keys` message that leaves the keys as they are goes nowhere.
 */
export function installFrames(win: FrameWindow, keys: KeyHandler): (message: HostMessage) => void {
  const children = new Set<FrameWindow>();

  const prune = (): void => {
    for (const child of children) {
      if (child.closed) {
        children.delete(child);
      }
    }
  };

  const relay = (message: HostMessage): void => {
    switch (message.kind) {
      case 'keys':
        if (message.enabled === keys.enabled) {
          return;
        }
        keys.setEnabled(message.enabled);
        break;
      case 'cancelChord':
        keys.cancelChord();
        break;
      case 'blurInput':
        keys.blurFocusedInput();
        break;
    }
    prune();
    for (const child of children) {
      post(child, message);
    }
  };

  const fromChild = (
    child: FrameWindow,
    message: Record<string, unknown>,
    timeStamp: number,
  ): void => {
    switch (message.kind) {
      case 'hello':
        prune();
        children.add(child);
        if (keys.enabled) {
          post(child, { kind: 'keys', enabled: true });
        }
        break;
      case 'scroll':
        if (isScrollAction(message.action) && typeof message.code === 'string') {
          keys.scrollFromChild(message.action, message.code, timeStamp);
        }
        break;
      case 'release':
        if (typeof message.code === 'string') {
          keys.releaseFromChild(message.code);
        }
        break;
    }
  };

  win.addEventListener(
    'message',
    (event) => {
      const { data, source } = event as MessageEvent;
      const message = onChannel(data);
      if (message === null) {
        return;
      }
      event.stopImmediatePropagation();
      if (!isWindow(source) || source === win) {
        return;
      }
      if (source === win.parent) {
        const host = hostMessage(message);
        if (host !== null) {
          relay(host);
        }
      } else if (source.parent === win) {
        fromChild(source, message, event.timeStamp);
      }
    },
    true,
  );

  toParent(win, { kind: 'hello' });
  return relay;
}

/**
 * The key host's hand-off to the page script in the parent frame: a scroll this frame cannot make,
 * and the release of its key. Both do nothing in the top frame.
 */
export function parentHandOff(win: FrameWindow): {
  handOff(action: ScrollAction, code: string): void;
  release(code: string): void;
} {
  return {
    handOff: (action, code) => toParent(win, { kind: 'scroll', action, code }),
    release: (code) => toParent(win, { kind: 'release', code }),
  };
}

function hostMessage(message: Record<string, unknown>): HostMessage | null {
  switch (message.kind) {
    case 'keys':
      return typeof message.enabled === 'boolean'
        ? { kind: 'keys', enabled: message.enabled }
        : null;
    case 'cancelChord':
    case 'blurInput':
      return { kind: message.kind };
    default:
      return null;
  }
}

function toParent(win: FrameWindow, message: ToParent): void {
  if (win.parent !== win) {
    post(win.parent, message);
  }
}

function post(target: FrameWindow, message: HostMessage | ToParent): void {
  target.postMessage({ [CHANNEL]: message }, '*');
}

function onChannel(data: unknown): Record<string, unknown> | null {
  const message = (data as Record<string, unknown> | null | undefined)?.[CHANNEL];
  return typeof message === 'object' && message !== null
    ? (message as Record<string, unknown>)
    : null;
}

function isWindow(source: unknown): source is FrameWindow {
  return typeof source === 'object' && source !== null && 'parent' in source;
}
