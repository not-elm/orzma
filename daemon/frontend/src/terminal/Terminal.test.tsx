import { render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

const focusSpy = vi.fn();
const blurSpy = vi.fn();
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
  useXtermTerminal: () => ({ focus: focusSpy, blur: blurSpy }),
}));

import { Terminal } from './Terminal';

describe('<Terminal>', () => {
  it('does NOT call focus or blur on initial mount when isActive=true', () => {
    focusSpy.mockClear();
    blurSpy.mockClear();
    render(<Terminal windowId="wid" paneId="pid" activityId="aid" isActive={true} />);
    expect(focusSpy).not.toHaveBeenCalled();
    expect(blurSpy).not.toHaveBeenCalled();
  });

  it('does NOT call focus or blur on initial mount when isActive=false', () => {
    focusSpy.mockClear();
    blurSpy.mockClear();
    render(<Terminal windowId="wid" paneId="pid" activityId="aid" isActive={false} />);
    expect(focusSpy).not.toHaveBeenCalled();
    expect(blurSpy).not.toHaveBeenCalled();
  });

  it('calls focus (not blur) when isActive transitions false → true', () => {
    focusSpy.mockClear();
    blurSpy.mockClear();
    const { rerender } = render(
      <Terminal windowId="wid" paneId="pid" activityId="aid" isActive={false} />,
    );
    expect(focusSpy).not.toHaveBeenCalled();
    rerender(<Terminal windowId="wid" paneId="pid" activityId="aid" isActive={true} />);
    expect(focusSpy).toHaveBeenCalledTimes(1);
    expect(blurSpy).not.toHaveBeenCalled();
  });

  it('calls blur (not focus) when isActive transitions true → false', () => {
    focusSpy.mockClear();
    blurSpy.mockClear();
    const { rerender } = render(
      <Terminal windowId="wid" paneId="pid" activityId="aid" isActive={true} />,
    );
    focusSpy.mockClear();
    blurSpy.mockClear();
    rerender(<Terminal windowId="wid" paneId="pid" activityId="aid" isActive={false} />);
    expect(blurSpy).toHaveBeenCalledTimes(1);
    expect(focusSpy).not.toHaveBeenCalled();
  });
});
