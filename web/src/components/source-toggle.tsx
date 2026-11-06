import type { QueueSource } from "../lib/api";
import { Button } from "./ui/button";

export function SourceToggle({
  source,
  onChange,
}: {
  source: QueueSource;
  onChange: (source: QueueSource) => void;
}) {
  return (
    <div className="flex rounded-lg border border-border p-0.5">
      <Button
        variant={source === "local" ? "secondary" : "ghost"}
        size="sm"
        onClick={() => onChange("local")}
      >
        Local
      </Button>
      <Button
        variant={source === "remote" ? "secondary" : "ghost"}
        size="sm"
        onClick={() => onChange("remote")}
      >
        Remote
      </Button>
    </div>
  );
}
