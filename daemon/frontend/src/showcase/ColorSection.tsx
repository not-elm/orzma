import { useEffect, useState } from 'react';

type TokenRow = {
  /** CSS variable suffix shown in UI, without the --color- prefix */
  name: string;
  /** Literal Tailwind background class — must appear verbatim for Tailwind to emit it */
  bgClass: string;
  /** Literal Tailwind foreground class for the paired -foreground token, or null */
  textClass: string | null;
  /**
   * Underlying Layer 1 variable backing this token (e.g. `--tn-bg`).
   * `@theme inline` does NOT emit `--color-*` into `:root` at runtime — it only
   * inlines `var(--tn-*)` into the generated utility classes. To resolve the
   * actual hex value at runtime we must read the Layer 1 variable directly.
   */
  rawVar: string;
};

type Group = {
  title: string;
  rows: TokenRow[];
};

const GROUPS: Group[] = [
  {
    title: 'Surfaces',
    rows: [
      {
        name: 'background',
        bgClass: 'bg-background',
        textClass: 'text-foreground',
        rawVar: '--tn-bg',
      },
      {
        name: 'card',
        bgClass: 'bg-card',
        textClass: 'text-card-foreground',
        rawVar: '--tn-bg-card',
      },
      {
        name: 'popover',
        bgClass: 'bg-popover',
        textClass: 'text-popover-foreground',
        rawVar: '--tn-bg-popover',
      },
      {
        name: 'muted',
        bgClass: 'bg-muted',
        textClass: 'text-muted-foreground',
        rawVar: '--tn-bg-muted',
      },
    ],
  },
  {
    title: 'Action / Emphasis',
    rows: [
      {
        name: 'primary',
        bgClass: 'bg-primary',
        textClass: 'text-primary-foreground',
        rawVar: '--tn-blue',
      },
      {
        name: 'secondary',
        bgClass: 'bg-secondary',
        textClass: 'text-secondary-foreground',
        rawVar: '--tn-magenta',
      },
      {
        name: 'accent',
        bgClass: 'bg-accent',
        textClass: 'text-accent-foreground',
        rawVar: '--tn-border',
      },
      {
        name: 'destructive',
        bgClass: 'bg-destructive',
        textClass: 'text-destructive-foreground',
        rawVar: '--tn-red',
      },
    ],
  },
  {
    title: 'Form & Boundary',
    rows: [
      { name: 'border', bgClass: 'bg-border', textClass: null, rawVar: '--tn-border' },
      { name: 'input', bgClass: 'bg-input', textClass: null, rawVar: '--tn-border' },
      { name: 'ring', bgClass: 'bg-ring', textClass: null, rawVar: '--tn-blue' },
    ],
  },
  {
    title: 'Status',
    rows: [
      {
        name: 'success',
        bgClass: 'bg-success',
        textClass: 'text-success-foreground',
        rawVar: '--tn-green',
      },
      {
        name: 'warning',
        bgClass: 'bg-warning',
        textClass: 'text-warning-foreground',
        rawVar: '--tn-yellow',
      },
      {
        name: 'info',
        bgClass: 'bg-info',
        textClass: 'text-info-foreground',
        rawVar: '--tn-cyan',
      },
    ],
  },
  {
    title: 'tmux',
    rows: [
      {
        name: 'tmux-pane-border',
        bgClass: 'bg-tmux-pane-border',
        textClass: null,
        rawVar: '--tn-border',
      },
      {
        name: 'tmux-pane-active',
        bgClass: 'bg-tmux-pane-active',
        textClass: null,
        rawVar: '--tn-blue',
      },
      {
        name: 'tmux-status-bar',
        bgClass: 'bg-tmux-status-bar',
        textClass: 'text-tmux-status-bar-foreground',
        rawVar: '--tn-bg-status',
      },
    ],
  },
];

function hexToRgb(hex: string): [number, number, number] | null {
  const m = hex.trim().match(/^#?([0-9a-f]{6})$/i);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

function relLuminance([r, g, b]: [number, number, number]): number {
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function contrast(a: string, b: string): number | null {
  const ra = hexToRgb(a);
  const rb = hexToRgb(b);
  if (!ra || !rb) return null;
  const la = relLuminance(ra);
  const lb = relLuminance(rb);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

export function ColorSection() {
  const [resolved, setResolved] = useState<Record<string, string>>({});

  useEffect(() => {
    const style = getComputedStyle(document.documentElement);
    const map: Record<string, string> = {};
    for (const g of GROUPS) {
      for (const r of g.rows) {
        map[r.name] = style.getPropertyValue(r.rawVar).trim();
      }
    }
    setResolved(map);
  }, []);

  const bg = resolved.background ?? '';

  return (
    <section className="mb-12">
      <h2 className="text-lg text-foreground mb-4">Colors</h2>
      {GROUPS.map((g) => (
        <div key={g.title} className="mb-6">
          <h3 className="text-sm text-muted-foreground mb-2 uppercase tracking-wide">{g.title}</h3>
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-2">
            {g.rows.map((r) => {
              const value = resolved[r.name] ?? '';
              const ratio = bg && value ? contrast(value, bg) : null;
              const textClass = r.textClass ?? 'text-foreground';
              return (
                <div
                  key={r.name}
                  className={`${r.bgClass} ${textClass} border border-border rounded-md p-3 flex flex-col gap-1`}
                >
                  <div className="text-sm font-mono">--color-{r.name}</div>
                  <div className="text-xs opacity-80 font-mono">{value || '…'}</div>
                  {ratio !== null && (
                    <div className="text-xs opacity-80 font-mono">
                      {ratio.toFixed(2)}:1 vs background
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </section>
  );
}
