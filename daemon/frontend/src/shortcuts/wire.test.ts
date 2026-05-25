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
      repeatable: false,
    },
  ],
  repeat_timeout_ms: 500,
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
    expect(out?.prefix?.key).toBe('b');
    expect(out?.prefix?.modifiers.ctrl).toBe(true);
    expect(out?.prefix?.timeout_ms).toBe(2000);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0]).toEqual({
      chord: {
        key: 'x',
        modifiers: { ctrl: false, shift: false, alt: false, meta: false },
      },
      action: { type: 'close-pane' },
      repeatable: false,
    });
    expect(out?.repeat_timeout_ms).toBe(500);
  });

  it.each([
    'left',
    'right',
    'up',
    'down',
  ] as const)('parses a resize-pane binding with %s direction and repeatable=true', (direction) => {
    const payload = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key:
            direction === 'left'
              ? 'ArrowLeft'
              : direction === 'right'
                ? 'ArrowRight'
                : direction === 'up'
                  ? 'ArrowUp'
                  : 'ArrowDown',
          modifiers: { ctrl: true, shift: false, alt: false, meta: false },
          action: { type: 'resize-pane', direction },
          repeatable: true,
        },
      ],
    };
    const out = parseShortcuts(payload);
    expect(out).not.toBeNull();
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action.type).toBe('resize-pane');
    if (out?.bindings[1].action.type === 'resize-pane') {
      expect(out.bindings[1].action.direction).toBe(direction);
    }
    expect(out?.bindings[1].repeatable).toBe(true);
  });

  it('defaults repeatable to false when the field is missing', () => {
    const payload = {
      ...DEFAULT_JSON,
      bindings: [
        {
          key: 'x',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'close-pane' },
        },
      ],
    };
    const out = parseShortcuts(payload);
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].repeatable).toBe(false);
  });

  it('defaults repeat_timeout_ms to 500 when the field is missing', () => {
    const { repeat_timeout_ms: _omit, ...without } = DEFAULT_JSON;
    const out = parseShortcuts(without);
    expect(out).not.toBeNull();
    expect(out?.repeat_timeout_ms).toBe(500);
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

  it('parses a swap-pane binding for prev and next offsets', () => {
    const payload = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: '{',
          modifiers: { ctrl: false, shift: true, alt: false, meta: false },
          action: { type: 'swap-pane', offset: 'prev' },
        },
        {
          key: '}',
          modifiers: { ctrl: false, shift: true, alt: false, meta: false },
          action: { type: 'swap-pane', offset: 'next' },
        },
      ],
    };
    const out = parseShortcuts(payload);
    expect(out).not.toBeNull();
    expect(out?.bindings).toHaveLength(3);
    expect(out?.bindings[1].action).toEqual({ type: 'swap-pane', offset: 'prev' });
    expect(out?.bindings[2].action).toEqual({ type: 'swap-pane', offset: 'next' });
  });

  it('drops bindings with an unknown action type but keeps the rest', () => {
    const withUnknown = {
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: 'q',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'definitely-not-a-real-action' },
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

  it('parses focus-window action', () => {
    const raw = {
      prefix: {
        key: 'b',
        modifiers: { ctrl: true, shift: false, alt: false, meta: false },
        timeout_ms: 2000,
      },
      bindings: [
        {
          key: 'n',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'focus-window', offset: 'next' },
          repeatable: true,
        },
      ],
    };
    const out = parseShortcuts(raw);
    expect(out?.bindings[0]?.action).toEqual({ type: 'focus-window', offset: 'next' });
  });

  it('parses focus-window-number action', () => {
    const raw = {
      prefix: {
        key: 'b',
        modifiers: { ctrl: true, shift: false, alt: false, meta: false },
        timeout_ms: 2000,
      },
      bindings: [
        {
          key: '0',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'focus-window-number', index: 0 },
          repeatable: false,
        },
      ],
    };
    const out = parseShortcuts(raw);
    expect(out?.bindings[0]?.action).toEqual({ type: 'focus-window-number', index: 0 });
  });

  it('parses a rename-window binding', () => {
    const out = parseShortcuts({
      ...DEFAULT_JSON,
      bindings: [
        DEFAULT_JSON.bindings[0],
        {
          key: ',',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'rename-window' },
          repeatable: false,
        },
      ],
    });
    expect(out?.bindings).toHaveLength(2);
    expect(out?.bindings[1].action).toEqual({ type: 'rename-window' });
  });

  it('parses a new-window binding', () => {
    const out = parseShortcuts({
      ...DEFAULT_JSON,
      bindings: [
        {
          key: 'c',
          modifiers: { ctrl: false, shift: true, alt: false, meta: false },
          action: { type: 'new-window' },
          repeatable: false,
        },
      ],
    });
    expect(out?.bindings).toHaveLength(1);
    expect(out?.bindings[0].action).toEqual({ type: 'new-window' });
  });

  it('parses a choose-tree action binding', () => {
    const result = parseShortcuts({
      prefix: {
        key: 'b',
        modifiers: { ctrl: true, shift: false, alt: false, meta: false },
        timeout_ms: 2000,
      },
      bindings: [
        {
          key: 'w',
          modifiers: { ctrl: false, shift: false, alt: false, meta: false },
          action: { type: 'choose-tree' },
          repeatable: false,
        },
      ],
      repeat_timeout_ms: 500,
    });
    expect(result).not.toBeNull();
    expect(result?.bindings).toHaveLength(1);
    expect(result?.bindings[0]?.action).toEqual({ type: 'choose-tree' });
  });
});
