import { SlidersHorizontal } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { StatusBadge } from "./status-badge";
import { Button } from "./ui/button";
import { Switch } from "./ui/switch";

export type FinishedOption = {
  status: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
};

/** A dropdown of switches that show finished (merged, dropped) patches in a card. */
export function FinishedFilter({
  label,
  options,
  hiddenCount,
}: {
  /** Names the card, for screen readers. */
  label: string;
  options: FinishedOption[];
  hiddenCount: number;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) {
      return;
    }
    function onPointer(event: MouseEvent) {
      if (!root.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={root} className="relative">
      <Button
        variant="outline"
        size="sm"
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={`${label}: show finished patches`}
        onClick={() => setOpen((value) => !value)}
      >
        <SlidersHorizontal />
        Show
        {hiddenCount ? (
          <span className="font-mono text-xs text-muted-foreground">
            {hiddenCount} hidden
          </span>
        ) : null}
      </Button>
      {open ? (
        <div
          id={panelId}
          className="absolute right-0 z-10 mt-1 w-56 space-y-1 rounded-lg border border-border bg-card p-2 shadow-lg"
        >
          {options.map((option) => (
            <div
              key={option.status}
              className="flex items-center justify-between gap-3 rounded-md px-2 py-1.5 text-sm hover:bg-muted"
            >
              <label
                htmlFor={`${panelId}-${option.status}`}
                className="flex flex-1 cursor-pointer items-center gap-2"
              >
                Show <StatusBadge value={option.status} />
              </label>
              <Switch
                id={`${panelId}-${option.status}`}
                aria-label={`Show ${option.status} patches`}
                checked={option.checked}
                onCheckedChange={option.onChange}
              />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
