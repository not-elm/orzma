import { z } from 'zod';

// NAMED_KEYS must mirror the Rust `Key` enum's named variants (shortcuts.rs).
export const NAMED_KEYS = [
  'Escape',
  'Space',
  'Enter',
  'Tab',
  'Backspace',
  'ArrowUp',
  'ArrowDown',
  'ArrowLeft',
  'ArrowRight',
  'Plus',
] as const;

export type NamedKey = (typeof NAMED_KEYS)[number];

const KeyTokenSchema = z.union([z.string().length(1), z.enum(NAMED_KEYS)]);

const ModifiersSchema = z.object({
  ctrl: z.boolean(),
  shift: z.boolean(),
  alt: z.boolean(),
  meta: z.boolean(),
});

const KeyChordFieldsSchema = z.object({
  key: KeyTokenSchema,
  modifiers: ModifiersSchema,
});

const ActionSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('close-pane') }),
  z.object({ type: z.literal('rename-window') }),
  z.object({ type: z.literal('new-window') }),
  z.object({
    type: z.literal('split-pane'),
    direction: z.enum(['horizontal', 'vertical']),
  }),
  z.object({
    type: z.literal('break-activity-to-pane'),
    direction: z.enum(['horizontal', 'vertical']),
  }),
  z.object({ type: z.literal('new-terminal-activity') }),
  z.object({ type: z.literal('close-activity') }),
  z.object({ type: z.literal('choose-tree') }),
  z.object({ type: z.literal('enter-copy-mode') }),
  z.object({
    type: z.literal('focus-activity'),
    offset: z.enum(['next', 'prev']),
  }),
  z.object({
    type: z.literal('focus-window'),
    offset: z.enum(['next', 'prev']),
  }),
  z.object({
    type: z.literal('focus-window-number'),
    index: z.number().int().min(0).max(9),
  }),
  z.object({
    type: z.literal('focus-pane'),
    direction: z.enum(['up', 'down', 'left', 'right']),
  }),
  z.object({
    type: z.literal('resize-pane'),
    direction: z.enum(['up', 'down', 'left', 'right']),
  }),
  z.object({
    type: z.literal('swap-pane'),
    offset: z.enum(['prev', 'next']),
  }),
]);

const PrefixSchema = KeyChordFieldsSchema.extend({
  timeout_ms: z.number().int().nonnegative(),
});

const BindingSchema = KeyChordFieldsSchema.extend({
  action: ActionSchema,
  repeatable: z.boolean().default(false),
});

const ShortcutsRawSchema = z.object({
  prefix: PrefixSchema,
  bindings: z.array(z.unknown()),
  repeat_timeout_ms: z.number().int().nonnegative().default(500),
});

const NewShortcutsRawSchema = z.object({
  bindings: z.record(z.string(), z.unknown()),
});

export type KeyChord = z.infer<typeof KeyChordFieldsSchema>;
export type Action = z.infer<typeof ActionSchema>;
export type Prefix = z.infer<typeof PrefixSchema>;
export interface Binding {
  chord: KeyChord;
  action: Action;
  repeatable: boolean;
}
export interface Shortcuts {
  prefix: Prefix | null;
  bindings: Binding[];
  repeat_timeout_ms?: number;
}

/**
 * Parses the legacy prefix-mode shape OR the new named-field shape.
 * Returns `null` on completely unrecognized input. For the new named-field
 * shape (no `prefix` field), returns `{ prefix: null, bindings: [] }` so
 * the dispatcher disables gracefully.
 */
export function parseShortcuts(raw: unknown): Shortcuts | null {
  const legacy = ShortcutsRawSchema.safeParse(raw);
  if (legacy.success) {
    return parseLegacyShortcuts(legacy.data);
  }
  const fresh = NewShortcutsRawSchema.safeParse(raw);
  if (fresh.success) {
    // NOTE: new named-field shape; the React frontend is deprecated for shortcuts
    // (see D2 in spec). Return a stub that disables prefix-mode dispatch.
    return { prefix: null, bindings: [] };
  }
  console.warn('parseShortcuts: unrecognized shape', raw);
  return null;
}

function parseLegacyShortcuts(data: z.infer<typeof ShortcutsRawSchema>): Shortcuts {
  const bindings: Binding[] = [];
  for (const entry of data.bindings) {
    const parsed = BindingSchema.safeParse(entry);
    if (!parsed.success) {
      console.warn('parseShortcuts: dropping binding', { entry, issues: parsed.error.issues });
      continue;
    }
    const { key, modifiers, action, repeatable } = parsed.data;
    bindings.push({ chord: { key, modifiers }, action, repeatable });
  }
  return {
    prefix: data.prefix,
    bindings,
    repeat_timeout_ms: data.repeat_timeout_ms,
  };
}
