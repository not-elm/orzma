/** A key press, identified so a scroll knows how long its key stays down. */
export interface Press {
  /** The key's `KeyboardEvent.code`. */
  code: string;
  /** Unique per press; a repeat carries the id of the press it repeats. */
  id: number;
  /** When the keydown happened, on the `performance.now()` timebase. */
  timeStamp: number;
  /** Whether this keydown repeats a key that is still held. */
  repeat: boolean;
}

/**
 * The keys held down right now, tracked from keydown and keyup.
 *
 * A keydown of a code that is still held counts as a repeat, because CEF never sets
 * `KeyboardEvent.repeat`.
 */
export class HeldKeys {
  private readonly held = new Map<string, number>();
  private nextId = 1;

  /** Records a keydown of `code` and returns its press. */
  press(code: string, timeStamp: number): Press {
    const id = this.held.get(code);
    if (id !== undefined) {
      return { code, id, timeStamp, repeat: true };
    }
    const fresh = this.nextId++;
    this.held.set(code, fresh);
    return { code, id: fresh, timeStamp, repeat: false };
  }

  /** Records a keyup of `code` and returns whether the key was held. */
  release(code: string): boolean {
    return this.held.delete(code);
  }

  /** Forgets every held key. */
  clear(): void {
    this.held.clear();
  }

  /** Whether the key of `press` is still down from that same press. */
  isHeld(press: Press): boolean {
    return this.held.get(press.code) === press.id;
  }
}
