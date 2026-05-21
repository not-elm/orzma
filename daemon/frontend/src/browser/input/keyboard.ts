// DOM keyboard → InputEvent::Key bridge for the cef-backed BrowserActivity.
//
// On macOS (the platform cef_host targets) press events are sent as
// `KEYEVENT_CHAR` — not `KEYEVENT_RAWKEYDOWN`. CEF's macOS OSR path lacks
// the `_hasUnhandledKeyDownEvent` safeguard that Chromium's own
// `BridgedContentView` uses, so `RAWKEYDOWN` causes editor commands (e.g.
// DeleteForward) to fire BOTH via Blink's `keyDownCommandsMap` AND via
// AppKit's `interpretKeyEvents:`-routed `doCommandBySelector:` — i.e. one
// press deletes two characters. `KEYEVENT_CHAR` enters Blink's
// `keyPressCommandsMap`, which has no Delete/Backspace bindings, so only
// the AppKit-side path fires (once). Reference implementation:
// `bevy_cef/.../browsers/keyboard.rs::create_cef_key_events`.

import type { InputEvent } from '../protocol/input';

const MOD_ALT = 1 << 3;
const MOD_CTRL = 1 << 2;
const MOD_SHIFT = 1 << 1;
const MOD_META = 1 << 7;

function modifiers(e: KeyboardEvent): number {
  return (
    (e.altKey ? MOD_ALT : 0) |
    (e.ctrlKey ? MOD_CTRL : 0) |
    (e.shiftKey ? MOD_SHIFT : 0) |
    (e.metaKey ? MOD_META : 0)
  );
}

// VK_* table (Chromium subset). Unknown keys fall back to KeyboardEvent.keyCode.
const VK_MAP: Record<string, number> = {
  Backspace: 0x08,
  Tab: 0x09,
  Enter: 0x0d,
  Escape: 0x1b,
  ArrowLeft: 0x25,
  ArrowUp: 0x26,
  ArrowRight: 0x27,
  ArrowDown: 0x28,
  Delete: 0x2e,
  Home: 0x24,
  End: 0x23,
  PageUp: 0x21,
  PageDown: 0x22,
  ' ': 0x20,
  Shift: 0x10,
  Control: 0x11,
  Alt: 0x12,
  Meta: 0x5b,
  F1: 0x70,
  F2: 0x71,
  F3: 0x72,
  F4: 0x73,
  F5: 0x74,
  F6: 0x75,
  F7: 0x76,
  F8: 0x77,
  F9: 0x78,
  F10: 0x79,
  F11: 0x7a,
  F12: 0x7b,
};

function windowsKeyCode(e: KeyboardEvent): number {
  if (e.key in VK_MAP) return VK_MAP[e.key];
  // NOTE: KeyboardEvent.keyCode is deprecated but still populated on
  // Chrome / Safari / Firefox; preferred over deriving a code from `e.key`
  // because it matches Chromium's VK_* enum directly.
  return (e as unknown as { keyCode?: number }).keyCode ?? 0;
}

export interface KeyboardAttachOpts {
  /** Sink for every translated InputEvent. */
  send: (ev: InputEvent) => void;
  /** Element whose keydown/keyup listeners we install. */
  element: HTMLElement;
  /** Returns `true` when the embedded page focus is on an editable field. */
  focusOnEditable: () => boolean;
}

/** Attaches keydown/keyup listeners and returns a detach closure. */
export function attachKeyboard({ send, element, focusOnEditable }: KeyboardAttachOpts): () => void {
  const onKey = (eventType: 'char' | 'key_up') => (e: KeyboardEvent) => {
    // NOTE: capture-phase consumers (the global prefix dispatcher in
    // `shortcuts/usePrefixMode.ts`) signal "do not forward to CEF" via
    // `preventDefault()`; `isComposing` keystrokes belong to the IME path
    // (`browser/input/ime.ts`) and would double-emit if we forwarded them.
    if (e.defaultPrevented || e.isComposing) return;
    // NOTE: macOS auto-repeat fires keydown at the OS-configured rate.
    // Forwarding each as a fresh CEF event runs the editor command per
    // repeat, so even a short tap of Delete deletes 2+ chars. Drop repeats
    // so 1 physical press = 1 wire event.
    if (e.repeat) return;
    const charCode = e.key.length === 1 ? e.key.charCodeAt(0) : 0;
    send({
      kind: 'key',
      event_type: eventType,
      windows_key_code: windowsKeyCode(e),
      // NOTE: native scan code is not exposed by the DOM KeyboardEvent — leave 0.
      native_key_code: 0,
      modifiers: modifiers(e),
      character: charCode,
      unmodified_character: e.key.length === 1 ? e.key.toLowerCase().charCodeAt(0) : 0,
      focus_on_editable_field: focusOnEditable(),
    });
  };

  const down = onKey('char');
  const up = onKey('key_up');
  element.addEventListener('keydown', down);
  element.addEventListener('keyup', up);
  return () => {
    element.removeEventListener('keydown', down);
    element.removeEventListener('keyup', up);
  };
}
