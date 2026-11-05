import { describe } from "../lib/status";
import { cn } from "../lib/utils";
import { Badge } from "./ui/badge";

const STYLES: Record<string, string> = {
  queued: "border-sky-500/30 bg-sky-500/10 text-sky-300",
  waiting: "border-indigo-500/30 bg-indigo-500/10 text-indigo-300",
  approved: "border-violet-500/30 bg-violet-500/10 text-violet-300",
  submitted: "border-teal-500/30 bg-teal-500/10 text-teal-300",
  rejected: "border-rose-500/30 bg-rose-500/10 text-rose-300",
  merged: "border-emerald-500/30 bg-emerald-500/10 text-emerald-300",
  dropped: "border-zinc-500/30 bg-zinc-500/10 text-zinc-400",
  conflict: "border-rose-500/30 bg-rose-500/10 text-rose-300",
  active: "border-emerald-500/30 bg-emerald-500/10 text-emerald-300",
  tooling: "border-zinc-500/30 bg-zinc-500/10 text-zinc-300",
  upstream: "border-teal-500/30 bg-teal-500/10 text-teal-300",
  internal: "border-amber-500/30 bg-amber-500/10 text-amber-300",
  "internal-only": "border-amber-500/30 bg-amber-500/10 text-amber-300",
};

export function StatusBadge({ value }: { value: string }) {
  const info = describe(value);
  return (
    <Badge
      variant="outline"
      className={cn("font-mono capitalize", STYLES[value] ?? "")}
      title={info ? `${info.meaning} Next: ${info.next}` : undefined}
    >
      {value}
    </Badge>
  );
}
