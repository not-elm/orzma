/**
 * Demo: mount the memo view inline in the current ozmux terminal.
 *
 * Run inside an ozmux terminal: `node mount.ts` (or `pnpm --filter memo mount`).
 * Prints a heading so the inline webview anchors below it, then writes the
 * mount-inline OSC + reserved rows in one write.
 */
import { mountInline } from '@ozmux/sdk/inline';

const ROWS = 12;
const COLS = 48;

process.stdout.write('memo (inline webview demo):\n');
process.stdout.write(mountInline('memo.main', { rows: ROWS, cols: COLS }));
