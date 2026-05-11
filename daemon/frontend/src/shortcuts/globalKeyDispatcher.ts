type KeyHandler = (e: KeyboardEvent) => void;

export interface KeyDispatcher {
  attachTo: (target: EventTarget) => void;
  detachFrom: (target: EventTarget) => void;
}

export function createKeyDispatcher(handler: KeyHandler): KeyDispatcher {
  const attached = new WeakSet<EventTarget>();
  const listener = (e: Event) => handler(e as KeyboardEvent);
  return {
    attachTo(target: EventTarget) {
      if (attached.has(target)) return;
      attached.add(target);
      target.addEventListener('keydown', listener, { capture: true });
    },
    detachFrom(target: EventTarget) {
      if (!attached.has(target)) return;
      attached.delete(target);
      target.removeEventListener('keydown', listener, { capture: true });
    },
  };
}
