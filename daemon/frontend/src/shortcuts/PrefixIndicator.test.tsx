import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PrefixIndicator } from './PrefixIndicator';
import type { Prefix } from './wire';

const CTRL_B: Prefix = {
  key: 'b',
  modifiers: { ctrl: true, shift: false, alt: false, meta: false },
  timeout_ms: 2000,
};

const CTRL_ALT_X: Prefix = {
  key: 'x',
  modifiers: { ctrl: true, shift: false, alt: true, meta: false },
  timeout_ms: 2000,
};

const ESC_NO_MODS: Prefix = {
  key: 'Escape',
  modifiers: { ctrl: false, shift: false, alt: false, meta: false },
  timeout_ms: 2000,
};

describe('PrefixIndicator', () => {
  it('renders nothing when not armed', () => {
    const { container } = render(<PrefixIndicator armed={false} prefix={CTRL_B} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders nothing when prefix is null even if armed', () => {
    const { container } = render(<PrefixIndicator armed={true} prefix={null} />);
    expect(container.firstChild).toBeNull();
  });

  it('renders the prefix label when armed', () => {
    render(<PrefixIndicator armed={true} prefix={CTRL_B} />);
    expect(screen.getByText('^B')).toBeInTheDocument();
  });

  it('renders modifier combos in deterministic order', () => {
    render(<PrefixIndicator armed={true} prefix={CTRL_ALT_X} />);
    expect(screen.getByText('^⌥X')).toBeInTheDocument();
  });

  it('renders named keys with a short label', () => {
    render(<PrefixIndicator armed={true} prefix={ESC_NO_MODS} />);
    expect(screen.getByText('Esc')).toBeInTheDocument();
  });

  it('uses role=status with polite aria-live', () => {
    render(<PrefixIndicator armed={true} prefix={CTRL_B} />);
    const el = screen.getByRole('status');
    expect(el).toHaveAttribute('aria-live', 'polite');
  });
});
