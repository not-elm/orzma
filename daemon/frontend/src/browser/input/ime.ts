// DOM composition → InputEvent::Ime* bridge for the cef-backed BrowserActivity.
//
// A hidden `<textarea>` overlay absorbs OS-level IME composition events
// (compositionstart / compositionupdate / compositionend) and translates them
// into the wire variants the daemon forwards to cef_host:
//
//   compositionupdate → ImeSetComposition (with current preedit string)
//   compositionend    → ImeCommit (with the final committed string) — Task A12
//                       spike confirmed `ime_commit_text` is the right cef-rs
//                       148 call, distinct from `ime_finish_composing_text`.
//   Esc during composition → ImeCancel

import type { ImeUnderline, InputEvent } from '../protocol/input';

export interface ImeAttachOpts {
  /** Sink for every translated InputEvent. */
  send: (ev: InputEvent) => void;
  /** Hidden textarea overlay that captures IME composition events. */
  textarea: HTMLTextAreaElement;
}

/** Attaches IME composition listeners and returns a detach closure. */
export function attachIme({ send, textarea }: ImeAttachOpts): () => void {
  let composing = false;

  const onCompositionStart = () => {
    composing = true;
  };

  const onCompositionUpdate = (e: CompositionEvent) => {
    const text = e.data ?? '';
    // PoC: Plan 3 wires colored underlines per parent §20.6.
    const underlines: ImeUnderline[] = [];
    send({
      kind: 'ime_set_composition',
      text,
      underlines,
      // NOTE: (-1, -1) is the wire sentinel for "no replacement range";
      // cef_host's input.rs translates this to `Option<Range>::None`.
      replacement_range: [-1, -1],
      selection_range: [text.length, text.length],
    });
  };

  const onCompositionEnd = (e: CompositionEvent) => {
    composing = false;
    const text = e.data ?? '';
    send({
      kind: 'ime_commit',
      text,
      replacement_range: null,
      relative_cursor_pos: 0,
    });
    textarea.value = '';
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (composing && e.key === 'Escape') {
      composing = false;
      send({ kind: 'ime_cancel' });
    }
  };

  textarea.addEventListener('compositionstart', onCompositionStart);
  textarea.addEventListener('compositionupdate', onCompositionUpdate);
  textarea.addEventListener('compositionend', onCompositionEnd);
  textarea.addEventListener('keydown', onKeyDown);

  return () => {
    textarea.removeEventListener('compositionstart', onCompositionStart);
    textarea.removeEventListener('compositionupdate', onCompositionUpdate);
    textarea.removeEventListener('compositionend', onCompositionEnd);
    textarea.removeEventListener('keydown', onKeyDown);
  };
}
