import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ClockSegment } from './ClockSegment';

describe('ClockSegment', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-05-16T13:42:00Z'));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders HH:MM and ticks every second', () => {
    render(<ClockSegment />);
    const initial = screen.getByTestId('clock').textContent;
    expect(initial).toMatch(/^\d{2}:\d{2}$/);

    act(() => {
      vi.setSystemTime(new Date('2026-05-16T13:43:00Z'));
      vi.advanceTimersByTime(1000);
    });

    const after = screen.getByTestId('clock').textContent;
    expect(after).not.toBe(initial);
  });
});
