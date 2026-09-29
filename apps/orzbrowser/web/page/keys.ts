import type { HeldKeys } from './held';
import { runScrollAction, type ScrollAction, type Scroller } from './scroller';

/** What the key handler tells the controller. */
export interface KeyHost {
  /** Reports the first key of a pending chord, or `null` once it resolves. */
  reportPending(key: string | null): void;
  /** Whether `event` came from the user; defaults to `event.isTrusted`. */
  isTrusted?(event: Event): boolean;
}

/** Controls the page's scroll keys. */
export interface KeyHandler {
  /**
   * Turns the scroll keys on or off. Off stops every scroll and drops a pending chord; on takes
   * focus from a text field the page focused before the user acted, so the keys scroll.
   */
  setEnabled(enabled: boolean): void;
  /** Drops a pending chord and reports `null`. */
  cancelChord(): void;
  /** Takes focus from a focused text field. */
  blurFocusedInput(): void;
}

const TEXTLESS_INPUT_TYPES = new Set([
  'button',
  'checkbox',
  'color',
  'file',
  'hidden',
  'image',
  'radio',
  'range',
  'reset',
  'submit',
]);

/**
 * Installs the scroll keys on `win` with capture-phase listeners, which run before the page's
 * own, so a handled key never reaches the page. The keys start off. Until the user presses a key
 * or the mouse on the page, a text field the page focuses on its own loses focus again while the
 * keys are on.
 */
export function installKeys(
  win: Window,
  scroller: Scroller,
  held: HeldKeys,
  host: KeyHost,
): KeyHandler {
  const doc = win.document;
  const trusted = host.isTrusted ?? ((event: Event) => event.isTrusted);
  const consumed = new Set<string>();
  let enabled = false;
  let pending = false;
  let userActed = false;

  const dropChord = (): void => {
    if (pending) {
      pending = false;
      host.reportPending(null);
    }
  };

  const blurFocusedInput = (): void => {
    const focused = deepActiveElement(doc);
    if (isEditable(focused)) {
      (focused as HTMLElement).blur();
    }
  };

  win.addEventListener(
    'mousedown',
    (event) => {
      if (trusted(event)) {
        userActed = true;
      }
    },
    true,
  );

  win.addEventListener(
    'focusin',
    () => {
      if (enabled && !userActed) {
        blurFocusedInput();
      }
    },
    true,
  );

  win.addEventListener(
    'keydown',
    (event) => {
      if (!trusted(event)) {
        return;
      }
      userActed = true;
      if (!enabled || event.isComposing || event.keyCode === 229) {
        return;
      }
      if (isEditable(deepActiveElement(doc))) {
        dropChord();
        return;
      }
      const action = actionFor(event);
      if (action === null) {
        dropChord();
        return;
      }
      const press = held.press(event.code, event.timeStamp);
      consume(event);
      consumed.add(event.code);
      if (action === 'g') {
        if (press.repeat) {
          return;
        }
        if (pending) {
          dropChord();
          runScrollAction(scroller, 'top', press);
          return;
        }
        pending = true;
        host.reportPending('g');
        return;
      }
      dropChord();
      runScrollAction(scroller, action, press);
    },
    true,
  );

  win.addEventListener(
    'keyup',
    (event) => {
      held.release(event.code);
      if (consumed.delete(event.code)) {
        consume(event);
      }
    },
    true,
  );

  win.addEventListener('blur', () => {
    held.clear();
    consumed.clear();
  });

  return {
    setEnabled(next) {
      if (next === enabled) {
        return;
      }
      enabled = next;
      if (enabled) {
        if (!userActed) {
          blurFocusedInput();
        }
        return;
      }
      scroller.cancelAll();
      held.clear();
      consumed.clear();
      dropChord();
    },
    cancelChord() {
      pending = false;
      host.reportPending(null);
    },
    blurFocusedInput,
  };
}

function consume(event: Event): void {
  event.preventDefault();
  event.stopImmediatePropagation();
}

function actionFor(event: KeyboardEvent): ScrollAction | 'g' | null {
  if (event.metaKey || event.altKey) {
    return null;
  }
  if (event.ctrlKey) {
    if (event.shiftKey) {
      return null;
    }
    switch (event.key) {
      case 'd':
        return 'halfDown';
      case 'u':
        return 'halfUp';
      case 'f':
        return 'pageDown';
      case 'b':
        return 'pageUp';
      default:
        return null;
    }
  }
  if (event.key === 'G') {
    return 'bottom';
  }
  if (event.shiftKey) {
    return null;
  }
  switch (event.key) {
    case 'j':
    case 'ArrowDown':
      return 'down';
    case 'k':
    case 'ArrowUp':
      return 'up';
    case ' ':
      return 'halfDown';
    case 'PageDown':
      return 'pageDown';
    case 'PageUp':
      return 'pageUp';
    case 'g':
      return 'g';
    default:
      return null;
  }
}

function deepActiveElement(doc: Document): Element | null {
  let focused = doc.activeElement;
  while (focused?.shadowRoot?.activeElement) {
    focused = focused.shadowRoot.activeElement;
  }
  return focused;
}

function isEditable(el: Element | null): boolean {
  if (el === null) {
    return false;
  }
  if ((el as HTMLElement).isContentEditable) {
    return true;
  }
  if (el.closest('[contenteditable]:not([contenteditable="false"])') !== null) {
    return true;
  }
  switch (el.tagName) {
    case 'TEXTAREA':
    case 'SELECT':
      return true;
    case 'INPUT':
      return !TEXTLESS_INPUT_TYPES.has((el as HTMLInputElement).type);
    default:
      return false;
  }
}
