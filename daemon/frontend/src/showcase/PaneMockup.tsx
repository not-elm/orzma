export function PaneMockup() {
  return (
    <section className="mb-12">
      <h2 className="text-lg text-foreground mb-4">Pane Mockup</h2>
      <p className="text-sm text-muted-foreground mb-3">
        End-to-end token usage in a tmux-style mini-UI. All colors, sizes, and borders come from
        semantic tokens.
      </p>
      <div className="border border-border rounded-md overflow-hidden font-mono text-sm">
        <div className="grid grid-cols-[1.4fr_1fr] min-h-[200px]">
          {/* Left pane: active */}
          <div className="bg-background text-foreground p-3 border-2 border-tmux-pane-active">
            <div>
              <span className="text-muted-foreground">$</span>{' '}
              <span className="text-primary">cargo</span> run
            </div>
            <div className="text-success"> ✓ Compiled in 1.2s</div>
            <div>
              <span className="text-muted-foreground">$</span>{' '}
              <span className="text-primary">git</span> status
            </div>
            <div className="text-warning"> M src/main.rs</div>
            <div className="text-warning"> M src/lib.rs</div>
          </div>

          {/* Right pane: inactive */}
          <div className="bg-background text-foreground p-3 border border-tmux-pane-border">
            <div className="text-muted-foreground">[logs]</div>
            <div>listening :8080</div>
            <div className="text-success">200 GET /api/sessions</div>
            <div className="text-info">101 WS /api/stream</div>
            <div className="text-destructive">404 GET /favicon.ico</div>
          </div>
        </div>

        {/* Status bar */}
        <div className="bg-tmux-status-bar text-tmux-status-bar-foreground px-3 py-1 text-xs flex justify-between border-t border-border">
          <span>ozmux · main</span>
          <span className="text-muted-foreground">2 panes</span>
        </div>
      </div>
    </section>
  );
}
