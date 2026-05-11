interface Props {
  type: string;
}

export function UnknownLayoutNode({ type }: Props) {
  return (
    <div className="h-full w-full border border-destructive bg-background p-2 text-xs text-destructive">
      Unknown layout node type: <code className="font-mono">{type}</code>
    </div>
  );
}
