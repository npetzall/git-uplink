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

/** The query string for a source and view; the defaults are left out. */
export function viewSearch(source: QueueSource, view: QueueView): string {
  const params = new URLSearchParams();
  if (source === "remote") {
    params.set("source", source);
  }
  if (view !== "queue") {
    params.set("view", view);
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
