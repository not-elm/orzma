type Kind = "disconnected" | "exited";

const COPY: Record<Kind, { title: string; cta: string }> = {
  disconnected: { title: "Disconnected", cta: "Reconnect" },
  exited: { title: "Process exited", cta: "Restart" },
};

export function StatusBanner({
  kind,
  onReconnect,
}: {
  kind: Kind;
  onReconnect: () => void;
}) {
  const { title, cta } = COPY[kind];
  return (
    <div className="absolute inset-x-0 bottom-0 flex items-center justify-between border-t border-border bg-card px-4 py-2 text-foreground">
      <span>{title}</span>
      <button
        type="button"
        onClick={onReconnect}
        className="rounded-sm bg-primary px-3 py-1 text-primary-foreground"
      >
        {cta}
      </button>
    </div>
  );
}
