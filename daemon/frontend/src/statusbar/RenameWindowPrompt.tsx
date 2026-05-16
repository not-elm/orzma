import { useEffect, useRef } from 'react';
import { renameWindow } from '../layout/renameWindow';
import type { ClosePromptOptions, RenamePromptState } from './useRenameWindowPrompt';

interface RenameWindowPromptProps {
  promptState: RenamePromptState;
  closePrompt: (options: ClosePromptOptions) => void;
}

/**
 * tmux-style prompt row, rendered directly above the status bar while
 * the rename-window prompt is open. `Enter` renames a non-empty trimmed
 * value and closes; `Escape` cancels; losing focus cancels. IME-confirm
 * and held-key auto-repeat keydowns are ignored.
 */
export function RenameWindowPrompt({ promptState, closePrompt }: RenameWindowPromptProps) {
  const inputRef = useRef<HTMLInputElement | null>(null);

  // NOTE: select-all on open so the first keystroke replaces the
  // prefilled name rather than appending to it.
  useEffect(() => {
    if (promptState.open) inputRef.current?.select();
  }, [promptState.open]);

  // NOTE: native listener required so e.isComposing reads correctly;
  // React synthetic events do not forward isComposing from jsdom events.
  useEffect(() => {
    if (!promptState.open) return;
    const input = inputRef.current;
    if (!input) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.isComposing || e.repeat) return;
      if (e.key === 'Enter') {
        e.preventDefault();
        const trimmed = input.value.trim();
        if (trimmed.length > 0) void renameWindow(promptState.windowId, trimmed);
        closePrompt({ restoreFocus: true });
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        closePrompt({ restoreFocus: true });
      }
    };

    input.addEventListener('keydown', handleKeyDown);
    return () => input.removeEventListener('keydown', handleKeyDown);
  }, [promptState, closePrompt]);

  if (!promptState.open) return null;

  return (
    <div className="flex h-6 shrink-0 items-center gap-2 border-t border-border bg-tmux-status-bar px-2 text-xs">
      <span className="font-mono text-muted-foreground">(rename-window)</span>
      <input
        ref={inputRef}
        type="text"
        aria-label="New window name"
        defaultValue={promptState.initialName}
        // biome-ignore lint/a11y/noAutofocus: the prompt is a transient modal input — focus must land here immediately so keystrokes are captured
        autoFocus
        onBlur={() => closePrompt({ restoreFocus: false })}
        className="flex-1 border-0 bg-transparent font-mono text-tmux-status-bar-foreground outline-none"
      />
    </div>
  );
}
