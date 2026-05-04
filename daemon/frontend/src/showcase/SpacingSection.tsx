const STEPS = [
  { name: '1', widthClass: 'w-1', px: 4 },
  { name: '2', widthClass: 'w-2', px: 8 },
  { name: '3', widthClass: 'w-3', px: 12 },
  { name: '4', widthClass: 'w-4', px: 16 },
  { name: '6', widthClass: 'w-6', px: 24 },
  { name: '8', widthClass: 'w-8', px: 32 },
  { name: '12', widthClass: 'w-12', px: 48 },
  { name: '16', widthClass: 'w-16', px: 64 },
  { name: '24', widthClass: 'w-24', px: 96 },
] as const;

export function SpacingSection() {
  return (
    <section className="mb-12">
      <h2 className="text-lg text-foreground mb-4">Spacing</h2>
      <p className="text-sm text-muted-foreground mb-3">
        Tailwind v4 default 4px grid (--spacing: 0.25rem). Steps shown as horizontal bars.
      </p>
      <div className="border border-border rounded-md p-3 flex flex-col gap-2">
        {STEPS.map((s) => (
          <div key={s.name} className="flex items-center gap-3">
            <div className="text-xs font-mono text-muted-foreground w-12 shrink-0">{s.name}</div>
            <div className={`${s.widthClass} h-4 bg-primary rounded-sm`} />
            <div className="text-xs font-mono text-muted-foreground">{s.px}px</div>
          </div>
        ))}
      </div>
    </section>
  );
}
