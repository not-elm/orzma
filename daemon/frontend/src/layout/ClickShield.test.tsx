import { fireEvent, render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ClickShield } from './ClickShield';

describe('<ClickShield>', () => {
  it('calls onActivate when clicked', () => {
    const onActivate = vi.fn();
    const { container } = render(<ClickShield onActivate={onActivate} />);
    const shield = container.firstChild as HTMLElement;
    fireEvent.pointerDown(shield);
    expect(onActivate).toHaveBeenCalledTimes(1);
  });

  it('stops pointerdown propagation so a parent handler does not fire', () => {
    const onActivate = vi.fn();
    const parentHandler = vi.fn();
    const { container } = render(
      <div onPointerDown={parentHandler}>
        <ClickShield onActivate={onActivate} />
      </div>,
    );
    const shield = container.querySelector('[aria-hidden="true"]') as HTMLElement;
    fireEvent.pointerDown(shield);
    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(parentHandler).not.toHaveBeenCalled();
  });
});
