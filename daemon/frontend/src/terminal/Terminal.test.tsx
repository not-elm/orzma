import { render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

const focusSpy = vi.fn();
const socketStub = {
  status: 'connecting' as const,
  setBinaryHandler: vi.fn(),
  sendBinary: vi.fn(),
  sendControl: vi.fn(),
};

vi.mock('./useTerminalSocket', () => ({
  useTerminalSocket: () => socketStub,
}));
vi.mock('./useXtermTerminal', () => ({
  useXtermTerminal: () => ({ focus: focusSpy }),
}));

import { Terminal } from './Terminal';

describe('<Terminal>', () => {
  it('does NOT call focus on initial mount when isActive=true', () => {
    focusSpy.mockClear();
    render(<Terminal activityId="aid" isActive={true} />);
    expect(focusSpy).not.toHaveBeenCalled();
  });

  it('does NOT call focus on initial mount when isActive=false', () => {
    focusSpy.mockClear();
    render(<Terminal activityId="aid" isActive={false} />);
    expect(focusSpy).not.toHaveBeenCalled();
  });

  it('calls focus when isActive transitions false → true', () => {
    focusSpy.mockClear();
    const { rerender } = render(<Terminal activityId="aid" isActive={false} />);
    expect(focusSpy).not.toHaveBeenCalled();
    rerender(<Terminal activityId="aid" isActive={true} />);
    expect(focusSpy).toHaveBeenCalledTimes(1);
  });

  it('does NOT call focus on true → false transition', () => {
    focusSpy.mockClear();
    const { rerender } = render(<Terminal activityId="aid" isActive={true} />);
    focusSpy.mockClear();
    rerender(<Terminal activityId="aid" isActive={false} />);
    expect(focusSpy).not.toHaveBeenCalled();
  });
});
