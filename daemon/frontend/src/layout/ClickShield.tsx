interface ClickShieldProps {
  onActivate: () => void;
}

export function ClickShield({ onActivate }: ClickShieldProps) {
  return (
    <div
      className="absolute inset-0 z-10 cursor-pointer bg-tmux-pane-inactive-overlay"
      onPointerDown={(e) => {
        e.stopPropagation();
        onActivate();
      }}
      aria-hidden="true"
    />
  );
}
