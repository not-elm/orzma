import { afterEach, describe, expect, it } from 'vitest';
import { injectTerminalPalette, removeTerminalPalette } from './palette';

const STYLE_ID = 'ozmux-terminal-palette';

afterEach(() => {
  removeTerminalPalette();
});

describe('injectTerminalPalette', () => {
  it('mounts a <style id="ozmux-terminal-palette"> element', () => {
    injectTerminalPalette();
    const style = document.getElementById(STYLE_ID);
    expect(style?.tagName).toBe('STYLE');
  });

  it('emits .fg-0 through .fg-255 and .bg-0 through .bg-255', () => {
    injectTerminalPalette();
    const css = document.getElementById(STYLE_ID)?.textContent ?? '';
    expect(css).toContain('.fg-0');
    expect(css).toContain('.fg-15');
    expect(css).toContain('.fg-231');
    expect(css).toContain('.fg-255');
    expect(css).toContain('.bg-0');
    expect(css).toContain('.bg-255');
  });

  it('emits .fg-default and .bg-default backed by theme tokens', () => {
    injectTerminalPalette();
    const css = document.getElementById(STYLE_ID)?.textContent ?? '';
    expect(css).toContain('.fg-default');
    expect(css).toContain('color: var(--color-foreground)');
    expect(css).toContain('.bg-default');
    expect(css).toContain('background-color: var(--color-background)');
  });

  it('emits the combined font-kerning selector', () => {
    injectTerminalPalette();
    const css = document.getElementById(STYLE_ID)?.textContent ?? '';
    expect(css).toContain('.terminal-grid,');
    expect(css).toContain('.terminal-grid span');
    expect(css).toContain('font-kerning: none');
  });

  it('emits a ::selection rule using --color-selection', () => {
    injectTerminalPalette();
    const css = document.getElementById(STYLE_ID)?.textContent ?? '';
    expect(css).toContain('.terminal-grid ::selection');
    expect(css).toContain('var(--color-selection)');
  });

  it('is idempotent — re-invocation replaces content without duplicating <style>', () => {
    injectTerminalPalette();
    injectTerminalPalette();
    const styles = document.querySelectorAll(`#${STYLE_ID}`);
    expect(styles.length).toBe(1);
  });
});
