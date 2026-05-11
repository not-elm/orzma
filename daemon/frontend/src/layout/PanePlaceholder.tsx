interface Props {
  paneId: string;
}

export function PanePlaceholder({ paneId }: Props) {
  return (
    <div className="h-full w-full border border-tmux-pane-border bg-background p-2 text-xs text-muted-foreground">
      <span>pane </span>
      <code className="font-mono">{paneId}</code>
    </div>
  );
}
