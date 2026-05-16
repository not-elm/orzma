import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as renameWindowModule from '../layout/renameWindow';
import { RenameWindowPrompt } from './RenameWindowPrompt';
import type { RenamePromptState } from './useRenameWindowPrompt';

const openState: RenamePromptState = {
  open: true,
  windowId: 'wid-1',
  initialName: 'old-name',
  returnFocus: null,
};

let renameSpy: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  renameSpy = vi.spyOn(renameWindowModule, 'renameWindow').mockResolvedValue(undefined);
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe('RenameWindowPrompt', () => {
  it('renders nothing when closed', () => {
    const { container } = render(
      <RenameWindowPrompt promptState={{ open: false }} closePrompt={vi.fn()} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('mounts focused with the name fully selected', () => {
    render(<RenameWindowPrompt promptState={openState} closePrompt={vi.fn()} />);
    const input = screen.getByRole('textbox') as HTMLInputElement;
    expect(input).toHaveFocus();
    expect(input.value).toBe('old-name');
    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe('old-name'.length);
  });

  it('Enter with a non-empty value renames and closes with focus restore', () => {
    const closePrompt = vi.fn();
    render(<RenameWindowPrompt promptState={openState} closePrompt={closePrompt} />);
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: '  new-name  ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(renameSpy).toHaveBeenCalledWith('wid-1', 'new-name');
    expect(closePrompt).toHaveBeenCalledWith({ restoreFocus: true });
  });

  it('Enter with a whitespace-only value closes without renaming', () => {
    const closePrompt = vi.fn();
    render(<RenameWindowPrompt promptState={openState} closePrompt={closePrompt} />);
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(renameSpy).not.toHaveBeenCalled();
    expect(closePrompt).toHaveBeenCalledWith({ restoreFocus: true });
  });

  it('Escape closes without renaming, with focus restore', () => {
    const closePrompt = vi.fn();
    render(<RenameWindowPrompt promptState={openState} closePrompt={closePrompt} />);
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
    expect(renameSpy).not.toHaveBeenCalled();
    expect(closePrompt).toHaveBeenCalledWith({ restoreFocus: true });
  });

  it('blur closes without renaming and without focus restore', () => {
    const closePrompt = vi.fn();
    render(<RenameWindowPrompt promptState={openState} closePrompt={closePrompt} />);
    fireEvent.blur(screen.getByRole('textbox'));
    expect(renameSpy).not.toHaveBeenCalled();
    expect(closePrompt).toHaveBeenCalledWith({ restoreFocus: false });
  });

  it('ignores IME-composing and auto-repeat keydowns', () => {
    const closePrompt = vi.fn();
    render(<RenameWindowPrompt promptState={openState} closePrompt={closePrompt} />);
    const input = screen.getByRole('textbox');
    fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(input, { key: 'Enter', repeat: true });
    expect(renameSpy).not.toHaveBeenCalled();
    expect(closePrompt).not.toHaveBeenCalled();
  });
});
