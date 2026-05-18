/** Mouse event kind. */
export type MouseKind = 'down' | 'up' | 'move';

/** Mouse button identifier. */
export type MouseButton = 'left' | 'middle' | 'right' | 'none';

/** Keyboard event kind. */
export type KeyKind = 'down' | 'up';

/**
 * Navigation command sent from the client to the browser activity.
 *
 * Discriminated by `kind` (`#[serde(tag = "kind", rename_all = "snake_case")]`).
 */
export type NavCommand =
  | { kind: 'navigate'; url: string }
  | { kind: 'back' }
  | { kind: 'forward' }
  | { kind: 'reload' }
  | { kind: 'stop' };

/**
 * Messages sent from the daemon (server) to the frontend over the browser WS.
 *
 * Discriminated by `kind` (`#[serde(tag = "kind", rename_all = "snake_case")]`).
 * Field names are snake_case to match Rust serde output.
 *
 * Modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8 (same as CDP).
 */
export type BrowserServerMsg =
  | { kind: 'screencast'; jpeg: Uint8Array; width: number; height: number }
  | { kind: 'nav'; url: string; title: string }
  | { kind: 'viewport'; width: number; height: number }
  | { kind: 'clipboard_write'; text: string }
  | { kind: 'page_error'; message: string };

/**
 * Messages sent from the frontend (client) to the daemon over the browser WS.
 *
 * Discriminated by `kind` (`#[serde(tag = "kind", rename_all = "snake_case")]`).
 * Field names are snake_case to match Rust serde output.
 *
 * Modifier bitmask: Alt=1, Ctrl=2, Meta=4, Shift=8 (same as CDP).
 * `text` on `key` is `string | null` (Rust `Option<String>` serializes to null or the string).
 */
export type BrowserClientMsg =
  | {
      kind: 'mouse';
      mouse_kind: MouseKind;
      x: number;
      y: number;
      button: MouseButton;
      modifiers: number;
    }
  | { kind: 'wheel'; x: number; y: number; dx: number; dy: number; modifiers: number }
  | {
      kind: 'key';
      key_kind: KeyKind;
      code: string;
      key: string;
      text: string | null;
      modifiers: number;
    }
  | { kind: 'ime_composition'; text: string; selection_start: number; selection_end: number }
  | { kind: 'ime_commit'; text: string }
  | { kind: 'nav'; nav: NavCommand }
  | { kind: 'resize'; width: number; height: number; device_scale_factor: number }
  | { kind: 'paste'; text: string }
  | { kind: 'copy_request' };
