import * as React from "react";
import { cn } from "../../lib/utils";

function Badge({
  className,
  variant = "outline",
  ...props
}: React.ComponentProps<"span"> & { variant?: "outline" | "default" }) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-md border px-2 py-0.5 text-xs font-medium",
        variant === "outline" && "border-border bg-transparent",
        className,
      )}
      {...props}
    />
  );
}

export { Badge };
