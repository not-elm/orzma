import { z } from 'zod';

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

export type KeyChord = z.infer<typeof KeyChordFieldsSchema>;
export type Action = z.infer<typeof ActionSchema>;
export type Prefix = z.infer<typeof PrefixSchema>;
export interface Binding {
  chord: KeyChord;
  action: Action;
  repeatable: boolean;
}
export interface Shortcuts {
  prefix: Prefix;
  bindings: Binding[];
  repeat_timeout_ms: number;
}

/**
 * Parse a wire payload from `GET /configs/shortcuts` into a `Shortcuts`
 * value. Returns `null` when the top-level shape or the prefix is invalid.
 *
 * Per-binding parse failures are logged with `console.warn` and the
 * failing entry is dropped, so a single unsupported `Action` variant or a
 * key outside the known token set does not nuke the whole config.
 */
export function parseShortcuts(raw: unknown): Shortcuts | null {
  const top = ShortcutsRawSchema.safeParse(raw);
  if (!top.success) {
    console.warn('parseShortcuts: top-level parse failed', top.error.issues);
    return null;
  }
  const bindings: Binding[] = [];
  for (const entry of top.data.bindings) {
    const parsed = BindingSchema.safeParse(entry);
    if (!parsed.success) {
      console.warn('parseShortcuts: dropping binding', { entry, issues: parsed.error.issues });
      continue;
    }
    const { key, modifiers, action, repeatable } = parsed.data;
    bindings.push({ chord: { key, modifiers }, action, repeatable });
  }
  return {
    prefix: top.data.prefix,
    bindings,
    repeat_timeout_ms: top.data.repeat_timeout_ms,
  };
}
