import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { parseShortcuts } from './wire';

const DEFAULT_JSON = {
  prefix: {
    key: 'b',
    modifiers: { ctrl: true, shift: false, alt: false, meta: false },
    timeout_ms: 2000,
  },
  bindings: [
    {
      key: 'x',
      modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      action: { type: 'close-pane' },
    },
  ],
};

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('parseShortcuts', () => {
  it('parses the default Shortcuts JSON', () => {
    const out = parseShortcuts(DEFAULT_JSON);
    expect(out).not.toBeNull();
    expect(out?.prefix.key).toBe('b');
    expect(out?.prefix.modifiers.ctrl).toBe(true);
    expect(out?.prefix.timeout_ms).toBe(2000);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0]).toEqual({
      chord: {
        key: 'x',
        modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      },
      action: { type: 'close-pane' },
    });
  });

  it('returns null when prefix is missing or malformed', () => {
    expect(parseShortcuts({ bindings: [] })).toBeNull();
    expect(
      parseShortcuts({
        prefix: { key: 'b', modifiers: {}, timeout_ms: 'soon' },
        bindings: [],
      }),
    ).toBeNull();
  });

  it('returns null when bindings field is absent', () => {
    expect(
      parseShortcuts({
        prefix: {
          key: 'b',
          modifiers: { ctrl: true, shift: false, alt: false, meta: false },
          timeout_ms: 2000,
        },
      }),
    ).toBeNull();
  });

  it('drops bindings with unknown action type but keeps the rest', () => {
    const withUnknown = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'q',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'swap-pane', offset: 'next' },
        },
      ],
    };
    const out = parseShortcuts(withUnknown);
    expect(out).not.toBeNull();
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].action.type).toBe('close-pane');
    expect(console.warn).toHaveBeenCalledTimes(1);
  });

  it.each([
    'up',
    'down',
    'left',
    'right',
  ] as const)('parses a focus-pane binding with %s direction', (direction) => {
    const payload = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'k',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'focus-pane', direction },
        },
      ],
    };
    const out = parseShortcuts(payload);
    expect(out).not.toBeNull();
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action).toEqual({ type: 'focus-pane', direction });
  });

  it('parses a split-pane binding with horizontal direction', () => {
    const withSplit = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 's',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'split-pane', direction: 'horizontal' },
        },
        {
          key: 'v',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'split-pane', direction: 'vertical' },
        },
      ],
    };
    const out = parseShortcuts(withSplit);
    expect(out?.bindings).toHaveLength(3);
    expect(out?.bindings[1].action).toEqual({ type: 'split-pane', direction: 'horizontal' });
    expect(out?.bindings[2].action).toEqual({ type: 'split-pane', direction: 'vertical' });
  });

  it('drops a split-pane binding with an invalid direction', () => {
    const withBadDir = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 's',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'split-pane', direction: 'diagonal' },
        },
      ],
    };
    const out = parseShortcuts(withBadDir);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].action.type).toBe('close-pane');
    expect(console.warn).toHaveBeenCalled();
  });

  it('drops bindings whose key is not a known token', () => {
    const withWeirdKey = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'f12',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'close-pane' },
        },
      ],
    };
    const out = parseShortcuts(withWeirdKey);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].chord.key).toBe('x');
  });

  it('returns null when input is not an object', () => {
    expect(parseShortcuts(null)).toBeNull();
    expect(parseShortcuts('nope')).toBeNull();
    expect(parseShortcuts(42)).toBeNull();
  });

  it('parses a new-terminal-activity binding', () => {
    const withNTA = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'c',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'new-terminal-activity' },
        },
      ],
    };
    const out = parseShortcuts(withNTA);
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action).toEqual({ type: 'new-terminal-activity' });
  });

  it('parses a break-activity-to-pane binding', () => {
    const out = parseShortcuts({
      ...DEFAULT_JSON,
      bindings: [
        {
          key: 's',
          modifiers: { ctrl: false, shift: true, alt: false, meta: false },
          action: { type: 'break-activity-to-pane', direction: 'horizontal' },
        },
      ],
    });
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].action).toEqual({
      type: 'break-activity-to-pane',
      direction: 'horizontal',
    });
    expect(out?.bindings[0].chord.modifiers.shift).toBe(true);
  });

  it('parses a close-activity binding', () => {
    const withCloseActivity = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'w',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'close-activity' },
        },
      ],
    };
    const out = parseShortcuts(withCloseActivity);
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action).toEqual({ type: 'close-activity' });
  });

  it.each(['next', 'prev'] as const)('parses a focus-activity binding with %s offset', (offset) => {
    const withFA = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: offset === 'next' ? ']' : '[',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'focus-activity', offset },
        },
      ],
    };
    const out = parseShortcuts(withFA);
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action).toEqual({ type: 'focus-activity', offset });
  });

  it('drops a focus-activity binding with an unknown offset (e.g. last)', () => {
    const withBadOffset = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'l',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'focus-activity', offset: 'last' },
        },
      ],
    };
    const out = parseShortcuts(withBadOffset);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].action.type).toBe('close-pane');
    expect(console.warn).toHaveBeenCalled();
  });
});
