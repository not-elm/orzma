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
]);

const PrefixSchema = KeyChordFieldsSchema.extend({
  timeout_ms: z.number().int().nonnegative(),
});

const BindingSchema = KeyChordFieldsSchema.extend({
  action: ActionSchema,
});

const ShortcutsRawSchema = z.object({
  prefix: PrefixSchema,
  bindings: z.array(z.unknown()),
});

export type KeyChord = z.infer<typeof KeyChordFieldsSchema>;
export type Action = z.infer<typeof ActionSchema>;
export type Prefix = z.infer<typeof PrefixSchema>;
export interface Binding {
  chord: KeyChord;
  action: Action;
}
export interface Shortcuts {
  prefix: Prefix;
  bindings: Binding[];
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
    const { key, modifiers, action } = parsed.data;
    bindings.push({ chord: { key, modifiers }, action });
  }
  return { prefix: top.data.prefix, bindings };
}
