import { useId, useRef } from "react";
import type React from "react";
import { cn } from "../../lib/utils";

export type TabItem = { id: string; label: string; content: React.ReactNode };

/** Minimal accessible tabs (WAI-ARIA tablist). Selection is controlled by the caller. */
export function Tabs({
  items,
  value,
  onChange,
  label,
}: {
  items: TabItem[];
  value: string;
  onChange: (id: string) => void;
  label: string;
}) {
  const baseId = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const index = Math.max(
    0,
    items.findIndex((item) => item.id === value),
  );
  const active = items[index];

  function focusAt(next: number) {
    const i = (next + items.length) % items.length;
    onChange(items[i].id);
    refs.current[i]?.focus();
  }

  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === "ArrowRight") focusAt(index + 1);
    else if (event.key === "ArrowLeft") focusAt(index - 1);
    else if (event.key === "Home") focusAt(0);
    else if (event.key === "End") focusAt(items.length - 1);
    else return;
    event.preventDefault();
  }

  return (
    <div className="space-y-6">
      <div role="tablist" aria-label={label} className="flex flex-wrap gap-1 border-b border-border" onKeyDown={onKeyDown}>
        {items.map((item, i) => {
          const selected = i === index;
          return (
            <button
              key={item.id}
              ref={(el) => {
                refs.current[i] = el;
              }}
              id={`${baseId}-tab-${item.id}`}
              role="tab"
              type="button"
              aria-selected={selected}
              aria-controls={`${baseId}-panel-${item.id}`}
              tabIndex={selected ? 0 : -1}
              onClick={() => onChange(item.id)}
              className={cn(
                "-mb-px border-b-2 px-3 py-2 text-sm transition-colors focus-visible:outline-2 focus-visible:outline-primary",
                selected
                  ? "border-primary text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground",
              )}
            >
              {item.label}
            </button>
          );
        })}
      </div>
      <div
        role="tabpanel"
        id={`${baseId}-panel-${active.id}`}
        aria-labelledby={`${baseId}-tab-${active.id}`}
        tabIndex={0}
      >
        {active.content}
      </div>
    </div>
  );
}
