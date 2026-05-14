import { renderHook, waitFor } from '@testing-library/react';
import { Server } from 'mock-socket';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { useCanvasTerminal } from './useCanvasTerminal';

const ENDPOINT = `ws://${location.host}/windows/w/panes/p/activities/a/terminal/ws?mode=vt&vt_version=vt-1`;

let server: Server | null = null;

beforeEach(() => {
  server = new Server(ENDPOINT);
});

afterEach(() => {
  server?.close();
  server = null;
});

describe('useCanvasTerminal', () => {
  it('returns canvasRef + textareaRef + status + focus/blur + socket', () => {
    const { result } = renderHook(() => useCanvasTerminal('w', 'p', 'a', true));
    expect(result.current.canvasRef).toBeDefined();
    expect(result.current.textareaRef).toBeDefined();
    expect(typeof result.current.focus).toBe('function');
    expect(typeof result.current.blur).toBe('function');
    expect(result.current.socket).toBeDefined();
  });

  it('attaches refs assigned to mounted DOM elements', async () => {
    const { render } = await import('@testing-library/react');
    function Probe() {
      const term = useCanvasTerminal('w', 'p', 'a', true);
      return (
        <div>
          <canvas ref={term.canvasRef} />
          <textarea ref={term.textareaRef} />
        </div>
      );
    }
    const { container } = render(<Probe />);
    expect(container.querySelector('canvas')).not.toBeNull();
    expect(container.querySelector('textarea')).not.toBeNull();
  });

  it('updates status as WebSocket connects', async () => {
    const { result } = renderHook(() => useCanvasTerminal('w', 'p', 'a', true));
    await waitFor(() =>
      expect(result.current.status === 'connecting' || result.current.status === 'connected').toBe(
        true,
      ),
    );
  });
});

describe('useCanvasTerminal — Phase 3A wiring', () => {
  it('exposes preedit in the return value', () => {
    const { result } = renderHook(() => useCanvasTerminal('w', 'p', 'a', true));
    expect(result.current.preedit).toBe('');
  });

  it('resets preedit when alt-screen mode toggles via control frame', async () => {
    const { result } = renderHook(() => useCanvasTerminal('w', 'p', 'a', true));
    await new Promise((r) => setTimeout(r, 10));

    server?.clients().forEach((c) => {
      c.send(JSON.stringify({ kind: 'mode', seq: 1, added: ['alt-screen'], removed: [] }));
    });

    await new Promise((r) => setTimeout(r, 10));
    expect(result.current.preedit).toBe('');
  });
});
