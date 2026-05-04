export function App() {
  return (
    <div className="min-h-screen bg-background text-foreground font-mono p-6">
      <h1 className="text-xl text-primary">Hello World!</h1>
      <p className="text-base text-muted-foreground mt-2">
        Tokens are wired up. Visit{' '}
        <code className="bg-card text-primary px-1 rounded-sm">?showcase=tokens</code> to see them
        all.
      </p>
    </div>
  );
}
