import {
  type HeldKeys,
  type Press,
  runScrollAction,
  type ScrollAction,
  type Scroller,
} from '@orzma/scroller';
import type { SearchStage } from './find';

/** A key the TUI relayed (`key` event), under its `KeyboardEvent.key` name. */
export interface RelayedKey {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}

/** A request to the controller (`page` event kind). */
type Request = 'quit' | 'reload' | 'back';

/** The outline sidebar as the keys drive it. */
interface OutlineKeys {
  isOpen(): boolean;
  open(current: number | null): void;
  close(): void;
  move(delta: 1 | -1): void;
  choose(): void;
}

/** The find box as the keys drive it. */
interface FindKeys {
  readonly stage: SearchStage;
  open(): void;
  typeText(text: string): void;
  backspace(): void;
  enter(): void;
  escape(): void;
  nav(dir: 'next' | 'prev'): void;
  clearHighlights(): void;
}

/** What the keys read from and act on in the rest of the page. */
interface KeyView {
  scroller: Scroller;
  outline: OutlineKeys;
  find: FindKeys;
  /** Scrolls heading `index` into view at once. */
  jumpToHeading(index: number): void;
  /** The index of the heading being read, or `null` above the first one. */
  currentHeading(): number | null;
  /** The number of headings in the document. */
  headingCount(): number;
  /** Shows the first key of a pending chord in the rail, or hides it with `null`. */
  showPending(key: string | null): void;
  /** Hides the toast on screen. */
  dismissToast(): void;
  /** Asks the controller to quit, re-read the file, or go back. */
  request(kind: Request): void;
}

/** The window the keys listen on and read the focused element from. */
interface KeyWindow extends EventTarget {
  readonly document: Document;
}

/** The first key of a two-key chord. */
type ChordKey = 'g' | '[' | ']';

/** What a key does, before the outline decides how to take it. */
type Binding =
  | ScrollAction
  | ChordKey
  | Request
  | 'next'
  | 'prev'
  | 'outline'
  | 'escape'
  | 'enter'
  | 'search';

/** A binding whose effect depends on whether the outline is open. */
type ModeBinding = Exclude<Binding, 'search' | Request>;

/** A key press from a DOM keydown or from the TUI, which never reports Meta. */
type KeyInput = RelayedKey & { meta: boolean };

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
 * Installs the page's keys on `win` with capture-phase listeners, which run before any other, so a
 * handled key and its keyup reach no other listener. Keys go to a focused editable field untouched.
 * Returns the function that runs a key the TUI relayed: typed into the find box while a query is
 * typed, and otherwise run as a single tap.
 */
export function installKeys(
  win: KeyWindow,
  held: HeldKeys,
  view: KeyView,
): (key: RelayedKey) => void {
  const doc = win.document;
  let pending: ChordKey | null = null;

  const setPending = (next: ChordKey | null): void => {
    if (next !== pending) {
      pending = next;
      view.showPending(next);
    }
  };

  const openSearch = (): void => {
    view.scroller.cancelAll();
    view.find.open();
  };

  const jumpHeading = (forward: boolean): void => {
    const count = view.headingCount();
    if (count === 0) {
      return;
    }
    const current = view.currentHeading();
    const target =
      current === null ? 0 : forward ? Math.min(current + 1, count - 1) : Math.max(current - 1, 0);
    if (target !== current) {
      view.jumpToHeading(target);
    }
  };

  const inOutline = (binding: ModeBinding): void => {
    switch (binding) {
      case 'down':
        view.outline.move(1);
        break;
      case 'up':
        view.outline.move(-1);
        break;
      case 'enter':
        view.outline.choose();
        break;
      case 'outline':
        view.outline.close();
        break;
      case 'escape':
        view.outline.close();
        view.find.clearHighlights();
        break;
      default:
        break;
    }
  };

  const inReading = (binding: ModeBinding, press: Press | undefined): void => {
    switch (binding) {
      case 'g':
      case '[':
      case ']':
        if (!press?.repeat) {
          setPending(binding);
        }
        break;
      case 'next':
      case 'prev':
        view.find.nav(binding);
        break;
      case 'outline':
        view.outline.open(view.currentHeading());
        break;
      case 'escape':
        view.find.clearHighlights();
        break;
      case 'enter':
        break;
      default:
        runScrollAction(view.scroller, binding, press);
    }
  };

  const handle = (binding: Binding, press: Press | undefined): void => {
    if (pending !== null) {
      if (binding === pending) {
        if (press?.repeat) {
          return;
        }
        const chord = pending;
        setPending(null);
        if (chord === 'g') {
          runScrollAction(view.scroller, 'top', press);
        } else {
          jumpHeading(chord === ']');
        }
        return;
      }
      setPending(null);
    }
    switch (binding) {
      case 'search':
        openSearch();
        return;
      case 'quit':
      case 'reload':
      case 'back':
        view.request(binding);
        return;
    }
    if (view.outline.isOpen()) {
      inOutline(binding);
    } else {
      inReading(binding, press);
    }
  };

  const typeIntoFind = (key: RelayedKey): void => {
    if (key.ctrl && !key.alt) {
      return;
    }
    switch (key.key) {
      case 'Enter':
        view.find.enter();
        break;
      case 'Escape':
        view.find.escape();
        break;
      case 'Backspace':
        view.find.backspace();
        break;
      default:
        if (Array.from(key.key).length === 1) {
          view.find.typeText(key.key);
        }
    }
  };

  win.addEventListener(
    'keydown',
    (event) => {
      const key = event as KeyboardEvent;
      view.dismissToast();
      if (key.isComposing || key.keyCode === 229) {
        return;
      }
      if (isEditable(doc.activeElement)) {
        setPending(null);
        return;
      }
      const binding = bindingFor({
        key: key.key,
        ctrl: key.ctrlKey,
        alt: key.altKey,
        shift: key.shiftKey,
        meta: key.metaKey,
      });
      if (binding === null) {
        setPending(null);
        return;
      }
      consume(key);
      handle(binding, held.press(key.code, key.timeStamp));
    },
    true,
  );

  win.addEventListener(
    'keyup',
    (event) => {
      const key = event as KeyboardEvent;
      if (held.release(key.code)) {
        consume(key);
      }
    },
    true,
  );

  win.addEventListener('blur', () => {
    held.clear();
    setPending(null);
  });

  return (key) => {
    view.dismissToast();
    if (view.find.stage === 'typing') {
      typeIntoFind(key);
      return;
    }
    const binding = bindingFor({ ...key, meta: false });
    if (binding === null) {
      setPending(null);
      return;
    }
    handle(binding, undefined);
  };
}

function consume(event: Event): void {
  event.preventDefault();
  event.stopImmediatePropagation();
}

/**
 * Names what `input` does. A single character matches by `key`: a letter by its case, with Shift
 * ignored and Alt refused; any other character with Shift ignored and Alt or Ctrl+Alt accepted, as
 * layouts type `/`, `[` and `]` that way. Named keys match only without modifiers, Ctrl matches only
 * its listed letters, and Meta matches nothing.
 */
function bindingFor(input: KeyInput): Binding | null {
  if (input.meta) {
    return null;
  }
  if (input.key.length === 1) {
    return characterBinding(input);
  }
  if (input.ctrl || input.alt || input.shift) {
    return null;
  }
  switch (input.key) {
    case 'ArrowDown':
      return 'down';
    case 'ArrowUp':
      return 'up';
    case 'PageDown':
      return 'pageDown';
    case 'PageUp':
      return 'pageUp';
    case 'Tab':
      return 'outline';
    case 'Enter':
      return 'enter';
    case 'Escape':
      return 'escape';
    case 'Backspace':
      return 'back';
    default:
      return null;
  }
}

function characterBinding({ key, ctrl, alt }: KeyInput): Binding | null {
  if (ctrl && !alt) {
    return controlBinding(key);
  }
  if (key.toLowerCase() !== key.toUpperCase()) {
    return alt ? null : letterBinding(key);
  }
  switch (key) {
    case ' ':
      return 'pageDown';
    case '/':
      return 'search';
    case '[':
    case ']':
      return key;
    default:
      return null;
  }
}

function controlBinding(key: string): Binding | null {
  switch (key) {
    case 'd':
      return 'halfDown';
    case 'u':
      return 'halfUp';
    case 'f':
      return 'pageDown';
    case 'b':
      return 'pageUp';
    case 'o':
      return 'back';
    default:
      return null;
  }
}

function letterBinding(key: string): Binding | null {
  switch (key) {
    case 'j':
      return 'down';
    case 'k':
      return 'up';
    case 'g':
      return 'g';
    case 'G':
      return 'bottom';
    case 'n':
      return 'next';
    case 'N':
      return 'prev';
    case 'o':
      return 'outline';
    case 'q':
      return 'quit';
    case 'r':
      return 'reload';
    default:
      return null;
  }
}

/** Whether `el` takes typed text: a writable text field, a select, or editable content. */
function isEditable(el: Element | null): boolean {
  if (el === null) {
    return false;
  }
  if (
    (el as HTMLElement).isContentEditable ||
    el.closest('[contenteditable]:not([contenteditable="false"])') !== null
  ) {
    return true;
  }
  switch (el.tagName) {
    case 'TEXTAREA': {
      const area = el as HTMLTextAreaElement;
      return !area.readOnly && !area.disabled;
    }
    case 'SELECT':
      return !(el as HTMLSelectElement).disabled;
    case 'INPUT': {
      const input = el as HTMLInputElement;
      return !TEXTLESS_INPUT_TYPES.has(input.type) && !input.readOnly && !input.disabled;
    }
    default:
      return false;
  }
}
