// Mirror of daemon/browser_cef_protocol::wire::InputEvent (spec §5 + parent §20.6).
//
// The Rust enum is tagged with `#[serde(tag = "kind", rename_all = "snake_case")]`,
// so each TS variant carries `kind: <snake_case>` and msgpack-roundtrips
// straight through to the Rust side. The `Key` inner discriminator is
// named `event_type` in both Rust and TS so it does not collide with the
// outer `kind` tag.

export type KeyEventType = 'raw_key_down' | 'key_up' | 'char';
export type MouseButton = 'left' | 'middle' | 'right';

export interface ImeUnderline {
  /** Inclusive start index in the composition string (UTF-16 units). */
  from: number;
  /** Exclusive end index in the composition string (UTF-16 units). */
  to: number;
  /** 32-bit ARGB color. */
  color: number;
  /** 32-bit ARGB background color. */
  background_color: number;
  /** `true` for a thick underline. */
  thick: boolean;
}

export type InputEvent =
  | { kind: 'mouse_move'; x: number; y: number; modifiers: number }
  | {
      kind: 'mouse_click';
      x: number;
      y: number;
      button: MouseButton;
      count: number;
      mouse_up: boolean;
      modifiers: number;
    }
  | {
      kind: 'mouse_wheel';
      x: number;
      y: number;
      delta_x: number;
      delta_y: number;
      modifiers: number;
    }
  | {
      kind: 'key';
      event_type: KeyEventType;
      windows_key_code: number;
      native_key_code: number;
      modifiers: number;
      /** UTF-16 code unit; 0 when N/A. */
      character: number;
      unmodified_character: number;
      focus_on_editable_field: boolean;
    }
  | {
      kind: 'ime_set_composition';
      text: string;
      underlines: ImeUnderline[];
      /** `[-1, -1]` denotes "no replacement range" — the cef_host side
       *  translates to `Option<Range>` for cef-rs 148. */
      replacement_range: [number, number];
      selection_range: [number, number];
    }
  | {
      kind: 'ime_commit';
      text: string;
      replacement_range: [number, number] | null;
      relative_cursor_pos: number;
    }
  | { kind: 'ime_cancel' };
