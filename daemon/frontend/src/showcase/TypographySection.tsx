const SIZES = [
  { token: 'text-xs', label: 'xs (12px)' },
  { token: 'text-sm', label: 'sm (13px)' },
  { token: 'text-base', label: 'base (14px) — default' },
  { token: 'text-lg', label: 'lg (16px)' },
  { token: 'text-xl', label: 'xl (18px)' },
] as const;

const SAMPLE = 'The quick brown fox jumps over the lazy dog · 0123456789 · 日本語サンプル';

function FontStack({ label, fontClass }: { label: string; fontClass: string }) {
  return (
    <>
      <h3 className="text-sm text-muted-foreground mb-2 uppercase tracking-wide">{label}</h3>
      <div className="border border-border rounded-md p-3 mb-6 last:mb-0">
        {SIZES.map((s) => (
          <div key={s.token} className="mb-3 last:mb-0">
            <div className="text-xs text-muted-foreground font-mono mb-1">{s.label}</div>
            <div className={`${s.token} ${fontClass} text-foreground`}>{SAMPLE}</div>
          </div>
        ))}
      </div>
    </>
  );
}

export function TypographySection() {
  return (
    <section className="mb-12">
      <h2 className="text-lg text-foreground mb-4">Typography</h2>
      <FontStack label="Mono" fontClass="font-mono" />
      <FontStack label="Sans" fontClass="font-sans" />
    </section>
  );
}
