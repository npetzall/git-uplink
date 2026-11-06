import { Link } from "react-router-dom";
import { CopyButton } from "./copy-button";

/** The patch page link for a patch, with the query string to carry over. */
export function patchHref(id: string, search: string): string {
  return `/patches/${encodeURIComponent(id)}${search}`;
}

/** A patch id that opens its patch page, with a copy button. */
export function PatchIdLink({ id, search }: { id: string; search: string }) {
  return (
    <span className="inline-flex items-center gap-1 font-mono text-xs">
      <Link to={patchHref(id, search)} className="text-primary hover:underline">
        {id}
      </Link>
      <CopyButton value={id} label={`Copy patch id ${id}`} />
    </span>
  );
}
