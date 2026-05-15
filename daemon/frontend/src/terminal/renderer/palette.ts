//! ANSI 16/256 palette as CSS classes injected at runtime + global terminal
//! styles (::selection, font-kerning, fg-default, bg-default). xterm.js
//! issue #4445 (CSP unsafe-inline) drove this pattern; truecolor stays on
//! inline style. Re-invocation on theme/font/options change is idempotent.

import { getFontConfig, pointsToPx } from '../../config/font';
import { colorToCss } from './colors';
import { FACE_BOLD, FACE_BOLD_ITALIC, FACE_ITALIC, PROBE_CLASS } from './font';

/** Element id of the runtime terminal stylesheet. */
export const STYLE_ID = 'ozmux-terminal-palette';

function fontCss(): string {
  const font = getFontConfig();
  const sizeRule = `font-size: ${pointsToPx(font.size)}px;`;
  // Each face: the tf-* class (empty for normal) and its resolved family.
  const faces: ReadonlyArray<readonly [string, string]> = [
    ['', font.normalFamily],
    [FACE_BOLD, font.boldFamily],
    [FACE_ITALIC, font.italicFamily],
    [FACE_BOLD_ITALIC, font.boldItalicFamily],
  ];
  const rules = [`.terminal-grid { ${sizeRule} }`, `.${PROBE_CLASS} { ${sizeRule} }`];
  for (const [face, family] of faces) {
    const decl = `font-family: ${family}, monospace;`;
    // Runs are descendants of `.terminal-grid`; probes carry the class directly.
    rules.push(`.terminal-grid${face ? ` .${face}` : ''} { ${decl} }`);
    rules.push(`.${PROBE_CLASS}${face ? `.${face}` : ''} { ${decl} }`);
  }
  return rules.join('\n');
}

function buildPaletteCss(): string {
  const lines: string[] = [];
  // ANSI 0-255 indexed colors.
  for (let i = 0; i < 256; i++) {
    const css = colorToCss(i, 'fg');
    if (!css) continue;
    lines.push(`.fg-${i} { color: ${css}; }`);
    lines.push(`.bg-${i} { background-color: ${css}; }`);
  }
  // Default fg / bg use theme tokens so reverse-video composes correctly.
  lines.push(`.fg-default { color: var(--color-foreground); }`);
  lines.push(`.bg-default { background-color: var(--color-background); }`);
  // Combined font-kerning selector — container alone doesn't inherit to
  // inline children on every browser engine.
  lines.push(`.terminal-grid, .terminal-grid span { font-kerning: none; }`);
  // Lock inline-block run alignment to the top of the row's line-box.
  // Default `vertical-align: baseline` lets glyphs of mixed metrics (bold,
  // italic, link, etc.) drift vertically — the absolute-positioned Cursor
  // overlay cannot follow those drifts. F3: force top-alignment.
  lines.push(`.terminal-grid span, .terminal-grid a { vertical-align: top; }`);
  // Native selection color uses theme token.
  lines.push(`.terminal-grid ::selection { background-color: var(--color-selection); }`);
  lines.push(fontCss());
  return lines.join('\n');
}

/** Injects (or replaces) the palette + global terminal stylesheet. Call on
 *  initial mount, theme change, or font/options change (xterm.js `_injectCss`
 *  pattern). */
export function injectTerminalPalette(): void {
  let style = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = document.createElement('style');
    style.id = STYLE_ID;
    document.head.appendChild(style);
  }
  style.textContent = buildPaletteCss();
}

/** Removes the palette stylesheet (test-only / unmount). */
export function removeTerminalPalette(): void {
  document.getElementById(STYLE_ID)?.remove();
}
