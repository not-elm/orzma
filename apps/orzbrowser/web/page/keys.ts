import {
  type HeldKeys,
  type Press,
  runScrollAction,
  type ScrollAction,
  type Scroller,
} from '@orzma/scroller';

/** What the key handler tells the controller and the parent frame. */
interface KeyHost {
  /** Reports the first key of a pending chord, or `null` once it resolves. */
  reportPending(key: string | null): void;
  /** Hands the parent frame a scroll this frame cannot make, held while the key `code` is down. */
  handOff(action: ScrollAction, code: string): void;
  /** Tells the parent frame that the key `code` of a handed-off scroll went up. */
  release(code: string): void;
  /** Whether `event` came from the user; defaults to `event.isTrusted`. */
  isTrusted?(event: Event): boolean;
}

/** Controls the page's scroll keys. */
export interface KeyHandler {
  /** Whether the scroll keys are on. */
  readonly enabled: boolean;
  /**
   * Turns the scroll keys on or off. Off stops every scroll, forgets the held and handed-off keys,
   * and drops a pending chord; on takes focus from a text field the page focused before the user
   * acted, so the keys scroll.
   */
  setEnabled(enabled: boolean): void;
  /** Drops a pending chord, reporting `null` when one was pending. */
  cancelChord(): void;
  /** Takes focus from a focused text field. */
  blurFocusedInput(): void;
  /**
   * Runs a scroll a child frame handed off, held until `releaseFromChild(code)`, and hands it on
   * to the parent frame when this frame cannot make it either. Ignored while the keys are off.
   * `timeStamp` is on this frame's `performance.now()` timebase.
   */
  scrollFromChild(action: ScrollAction, code: string, timeStamp: number): void;
  /** Records that the key `code` of a scroll a child frame handed off went up. */
  releaseFromChild(code: string): void;
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
 * keys are on. A scroll this frame cannot make goes to the parent frame through `host`.
 */
export function installKeys(
  win: Window,
  scroller: Scroller,
  held: HeldKeys,
  host: KeyHost,
): KeyHandler {
  const doc = win.document;
  const trusted = host.isTrusted ?? ((event: Event) => event.isTrusted);
  const handedOff = new Set<string>();
  let enabled = false;
  let pending = false;
  let userActed = false;

  const dropChord = (): void => {
    if (pending) {
      pending = false;
      host.reportPending(null);
    }
  };

  const scroll = (action: ScrollAction, press: Press): void => {
    if (!runScrollAction(scroller, action, press) && !press.repeat) {
      handedOff.add(press.code);
      host.handOff(action, press.code);
    }
  };

  const release = (code: string): boolean => {
    if (handedOff.delete(code)) {
      host.release(code);
    }
    return held.release(code);
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
      if (action === 'g') {
        if (press.repeat) {
          return;
        }
        if (pending) {
          dropChord();
          scroll('top', press);
          return;
        }
        pending = true;
        host.reportPending('g');
        return;
      }
      dropChord();
      scroll(action, press);
    },
    true,
  );

  win.addEventListener(
    'keyup',
    (event) => {
      if (!trusted(event)) {
        return;
      }
      if (release(event.code)) {
        consume(event);
      }
    },
    true,
  );

  win.addEventListener('blur', () => {
    for (const code of handedOff) {
      release(code);
    }
    held.clear();
    dropChord();
  });

  return {
    get enabled() {
      return enabled;
    },
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
      handedOff.clear();
      dropChord();
    },
    cancelChord: dropChord,
    blurFocusedInput,
    scrollFromChild(action, code, timeStamp) {
      if (enabled) {
        scroll(action, held.press(code, timeStamp));
      }
    },
    releaseFromChild: release,
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
