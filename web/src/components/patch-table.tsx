import { Link } from "react-router-dom";
import type { Patch } from "../lib/api";
import { NEEDS_APPROVAL, shownStatus } from "../lib/status";
import { PatchIdLink, patchHref } from "./patch-id-link";
import { StatusBadge } from "./status-badge";

export function PatchTable({
  patches,
  empty,
  search,
  layer,
  stale = [],
}: {
  patches: Patch[];
  empty: string;
  /** Query string carried to the patch page. */
  search: string;
  layer: "tooling" | "upstream" | "internal";
  /** Ids whose approval no longer covers their content. */
  stale?: string[];
}) {
  if (!patches.length) {
    return <p className="text-sm text-muted-foreground">{empty}</p>;
  }
  return (
    <table className="w-full min-w-[560px] text-left text-sm">
      <thead className="text-xs tracking-wide text-muted-foreground uppercase">
        <tr className="border-b">
          <th className="py-2 pr-3 font-medium">ID</th>
          <th className="py-2 pr-3 font-medium">Change</th>
          <th className="py-2 pr-3 font-medium">Status</th>
          <th className="py-2 font-medium">Depends on</th>
        </tr>
      </thead>
      <tbody>
        {patches.map((patch) => (
          <tr key={patch.id} className="border-b border-border/60">
            <td className="py-2 pr-3">
              <PatchIdLink id={patch.id} search={search} />
            </td>
            <td className="py-2 pr-3">
              <Link
                to={patchHref(patch.id, search)}
                className="hover:underline"
              >
                {patch.title}
              </Link>
            </td>
            <td className="space-x-1 py-2 pr-3">
              <StatusBadge value={shownStatus(patch.status, layer)} />
              {stale.includes(patch.id) ? (
                <StatusBadge value={NEEDS_APPROVAL} />
              ) : null}
            </td>
            <td className="py-2 font-mono text-xs text-muted-foreground">
              {(patch.dependsOn ?? []).join(", ") || "—"}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
