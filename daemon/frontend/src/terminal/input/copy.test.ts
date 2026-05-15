import { describe, expect, it, vi } from 'vitest';
import { formatSelectionText, setupCopy } from './copy';

describe('formatSelectionText', () => {
  it('trims trailing spaces per line and joins with \\n', () => {
    const raw = 'first line   \nsecond line  \nthird';
    expect(formatSelectionText(raw)).toBe('first line\nsecond line\nthird');
  });

  it('converts NBSP back to regular space (R4 reverse)', () => {
    const raw = 'hello world  ';
    expect(formatSelectionText(raw)).toBe('hello world');
  });

  it('returns empty string for empty input', () => {
    expect(formatSelectionText('')).toBe('');
  });
});

describe('setupCopy', () => {
  it('writes formatted selection to clipboardData on copy event', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    try {
      const cleanup = setupCopy(textarea);
      // Stub document.getSelection to return a known string
      const origSelection = document.getSelection.bind(document);
      document.getSelection = vi.fn(() => ({
        toString: () => 'foo  \nbar  ',
        rangeCount: 1,
      })) as unknown as typeof document.getSelection;
      try {
        const setData = vi.fn();
        const event = new Event('copy') as Event & {
          clipboardData: { setData: typeof setData };
          preventDefault: () => void;
        };
        Object.defineProperty(event, 'clipboardData', {
          value: { setData },
          writable: false,
        });
        Object.defineProperty(event, 'preventDefault', {
          value: vi.fn(),
          writable: false,
        });
        textarea.dispatchEvent(event);
        expect(setData).toHaveBeenCalledWith('text/plain', 'foo\nbar');
      } finally {
        document.getSelection = origSelection;
      }
      cleanup();
    } finally {
      document.body.removeChild(textarea);
    }
  });

  it('no-op when selection is empty', () => {
    const textarea = document.createElement('textarea');
    document.body.appendChild(textarea);
    try {
      const cleanup = setupCopy(textarea);
      const origSelection = document.getSelection.bind(document);
      document.getSelection = vi.fn(() => ({
        toString: () => '',
        rangeCount: 0,
      })) as unknown as typeof document.getSelection;
      try {
        const setData = vi.fn();
        const event = new Event('copy') as Event & {
          clipboardData: { setData: typeof setData };
          preventDefault: () => void;
        };
        Object.defineProperty(event, 'clipboardData', {
          value: { setData },
          writable: false,
        });
        Object.defineProperty(event, 'preventDefault', {
          value: vi.fn(),
          writable: false,
        });
        textarea.dispatchEvent(event);
        expect(setData).not.toHaveBeenCalled();
      } finally {
        document.getSelection = origSelection;
      }
      cleanup();
    } finally {
      document.body.removeChild(textarea);
    }
  });
});
