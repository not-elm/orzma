const ENC = new TextEncoder();

const SPECIAL: Record<string, Uint8Array> = {
  Enter: new Uint8Array([0x0d]),
  Backspace: new Uint8Array([0x7f]),
  Tab: new Uint8Array([0x09]),
  Escape: new Uint8Array([0x1b]),
  Delete: ENC.encode('\x1b[3~'),
  Home: ENC.encode('\x1b[H'),
  End: ENC.encode('\x1b[F'),
  PageUp: ENC.encode('\x1b[5~'),
  PageDown: ENC.encode('\x1b[6~'),
};

const ARROW_NORMAL: Record<string, Uint8Array> = {
  ArrowUp: ENC.encode('\x1b[A'),
  ArrowDown: ENC.encode('\x1b[B'),
  ArrowRight: ENC.encode('\x1b[C'),
  ArrowLeft: ENC.encode('\x1b[D'),
};

const ARROW_APP: Record<string, Uint8Array> = {
  ArrowUp: ENC.encode('\x1bOA'),
  ArrowDown: ENC.encode('\x1bOB'),
  ArrowRight: ENC.encode('\x1bOC'),
  ArrowLeft: ENC.encode('\x1bOD'),
};

const MODIFIER_ONLY = new Set(['Control', 'Shift', 'Alt', 'Meta']);

/** Translates a keydown event into VT escape bytes. Returns null when there is nothing to send. */
export function handleKeyDown(e: KeyboardEvent, mode: ReadonlySet<string>): Uint8Array | null {
  if (e.isComposing) return null;
  if (MODIFIER_ONLY.has(e.key)) return null;

  if (e.ctrlKey && e.key.length === 1) {
    const ch = e.key.toLowerCase();
    if (ch >= 'a' && ch <= 'z') {
      return new Uint8Array([ch.charCodeAt(0) - 96]);
    }
  }

  const arrow = mode.has('app-cursor-keys') ? ARROW_APP : ARROW_NORMAL;
  if (e.key in arrow) return arrow[e.key];

  if (e.key in SPECIAL) return SPECIAL[e.key];

  if (e.key.length === 1) {
    return new Uint8Array(ENC.encode(e.key));
  }
  return null;
}
