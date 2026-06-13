/**
 * Demo: mount the memo view inline twice in the current ozmux terminal.
 *
 * Run inside an ozmux terminal: `node mount.ts` (or `pnpm --filter memo mount`).
 * Prints a heading before each mount so the inline webviews anchor below them,
 * then writes each mount-inline OSC + reserved rows in one write. The two
 * mounts share `view_id` `memo.main` and are distinguished by `instanceId`.
 */
import { mountInline } from '@ozmux/sdk/inline';

const ROWS = 12;
const COLS = 48;

process.stdout.write('memo (instance a):\n');
process.stdout.write(mountInline('memo.main', { rows: ROWS, cols: COLS, instanceId: 'a' }));
process.stdout.write('\nmemo (instance b):\n');
process.stdout.write(mountInline('memo.main', { rows: ROWS, cols: COLS, instanceId: 'b' }));
