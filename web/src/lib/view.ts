import type { Patch, QueueSource } from "./api";

/** The tab under the queue cards. */
export type QueueView = "queue" | "ready" | "stream";

export function parseView(value: string | null): QueueView {
  return value === "ready" || value === "stream" ? value : "queue";
}

/** The branch a source reads, as shown to the operator. */
export function sourceBranch(source: QueueSource): string {
  return source === "remote" ? "origin/uplink/state" : "local uplink/state";
}

/** A finished status the operator chose to show in one layer's card. */
export type ShownFinished =
  | "upstream-merged"
  | "upstream-dropped"
  | "internal-dropped";

/** Every toggle, in the order the query string writes them. */
export const SHOWN_FINISHED: ShownFinished[] = [
  "upstream-merged",
  "upstream-dropped",
  "internal-dropped",
];

/** Reads `show=upstream-merged,internal-dropped`; unknown tokens are ignored. */
export function parseShown(value: string | null): ShownFinished[] {
  const tokens = new Set((value ?? "").split(","));
  return SHOWN_FINISHED.filter((token) => tokens.has(token));
}

/** The finished statuses a layer's card can hide: internal patches are never merged upstream. */
export function finishedStatuses(layer: "upstream" | "internal"): string[] {
  return layer === "upstream" ? ["merged", "dropped"] : ["dropped"];
}

/** The layer's patches without the finished statuses the operator has not chosen to show. */
export function visiblePatches<T extends Pick<Patch, "status">>(
  patches: T[],
  layer: "upstream" | "internal",
  shown: ShownFinished[],
): T[] {
  const hidden = finishedStatuses(layer).filter(
    (status) => !shown.includes(`${layer}-${status}` as ShownFinished),
  );
  return patches.filter((patch) => !hidden.includes(patch.status));
}

/** The query string for a source, view and shown finished statuses; the defaults are left out. */
export function viewSearch(
  source: QueueSource,
  view: QueueView,
  shown: ShownFinished[] = [],
): string {
  const params = new URLSearchParams();
  if (source === "remote") {
    params.set("source", source);
  }
  if (view !== "queue") {
    params.set("view", view);
  }
  if (shown.length) {
    params.set(
      "show",
      SHOWN_FINISHED.filter((token) => shown.includes(token)).join(","),
    );
  }
  const query = params.toString();
  return query ? `?${query}` : "";
}

/** The patch's name: the first non-empty line of its commit message. */
export function patchSubject(patch: Pick<Patch, "commitMessage">): string {
  return (
    (patch.commitMessage ?? "")
      .split("\n")
      .map((line) => line.trim())
      .find((line) => line !== "") ?? ""
  );
}
