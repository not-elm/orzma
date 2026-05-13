import { z } from 'zod';

const NAMED_KEYS = [
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

const ActionSchema = z.discriminatedUnion('type', [z.object({ type: z.literal('close-pane') })]);

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

export type KeyToken = z.infer<typeof KeyTokenSchema>;
export type Modifiers = z.infer<typeof ModifiersSchema>;
export interface KeyChord {
  key: KeyToken;
  modifiers: Modifiers;
}
export type Action = z.infer<typeof ActionSchema>;
export interface Prefix extends KeyChord {
  timeout_ms: number;
}
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
    bindings.push({
      chord: { key: parsed.data.key, modifiers: parsed.data.modifiers },
      action: parsed.data.action,
    });
  }
  return {
    prefix: {
      key: top.data.prefix.key,
      modifiers: top.data.prefix.modifiers,
      timeout_ms: top.data.prefix.timeout_ms,
    },
    bindings,
  };
}
