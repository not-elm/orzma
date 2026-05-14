import { render } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

const focusSpy = vi.fn();
const blurSpy = vi.fn();
const socketStub = {
  status: 'connecting' as const,
  setBinaryHandler: vi.fn(),
  setFrameHandler: vi.fn(),
  setControlHandler: vi.fn(),
  sendBinary: vi.fn(),
  sendControl: vi.fn(),
  reportDecodeError: vi.fn(),
};

vi.mock('./useTerminalSocket', () => ({
  useTerminalSocket: () => socketStub,
}));
vi.mock('./useCanvasTerminal', () => ({
  useCanvasTerminal: () => ({
    paneRef: { current: null },
    textareaRef: { current: null },
    status: 'connecting' as const,
    focus: focusSpy,
    blur: blurSpy,
    socket: socketStub,
    preedit: '',
    hyperlinks: new Map(),
    fm: { cellW: 8, cellH: 16, baseline: 12, fontCss: '14px monospace', dpr: 1 },
  }),
}));
vi.mock('./renderer/TerminalGrid', () => ({
  TerminalGrid: () => <div data-testid="terminal-grid" />,
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

  it('renders <TerminalGrid> + textarea (DOM renderer is the only path)', () => {
    const { container } = render(<Terminal windowId="w" paneId="p" activityId="a" isActive />);
    expect(container.querySelector('[data-testid="terminal-grid"]')).not.toBeNull();
    expect(container.querySelector('canvas')).toBeNull();
    expect(container.querySelector('textarea')).not.toBeNull();
  });
});
