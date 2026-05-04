const RADII = [
  { name: 'rounded-sm', px: '2px' },
  { name: 'rounded-md', px: '4px (default)' },
  { name: 'rounded-lg', px: '6px' },
  { name: 'rounded-xl', px: '8px' },
] as const;

export function RadiusSection() {
  return (
    <section className="mb-12">
      <h2 className="text-lg text-foreground mb-4">Border Radius</h2>
      <div className="border border-border rounded-md p-3 flex flex-wrap gap-4">
        {RADII.map((r) => (
          <div key={r.name} className="flex flex-col items-center gap-2">
            <div className={`${r.name} bg-primary w-20 h-20`} />
            <div className="text-xs font-mono text-foreground">{r.name}</div>
            <div className="text-xs font-mono text-muted-foreground">{r.px}</div>
          </div>
        ))}
      </div>
    </section>
  );
}
