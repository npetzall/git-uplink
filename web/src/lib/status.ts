/** What each patch status and label means and what moves it on. Shown as badge tooltips and in the queue legend. */
export const STATUS_INFO: Record<string, { meaning: string; next: string }> = {
  queued: {
    meaning:
      "Merged internally and in the company build. IP has not approved its current content. A previously submitted patch that was amended is queued again; its public pull request keeps the last approved bytes until IP approves the delta.",
    next: "Dispatch Uplink submit when it should go upstream; with UPLINK_AUTO_SUBMIT the pack does, once its upstream dependencies are merged. Internal-only patches stay queued.",
  },
  waiting: {
    meaning:
      "A submit run passed all its checks and waits for IP approval in the to-upstream Environment. Nothing has left the private forge.",
    next: "IP approves or rejects the run. Approval submits it; rejection, or cancelling the run, makes it rejected.",
  },
  approved: {
    meaning:
      "IP approved the to-upstream Environment. The same run submits it.",
    next: "Submit pushes it to the contribution fork. If it stays here, submit failed (for example preflight); check that run.",
  },
  submitted: {
    meaning:
      "On the public contribution fork with an open upstream pull request. The contribution fork has exactly what IP approved.",
    next: "Maintainers review and merge. Sync then marks it merged.",
  },
  rejected: {
    meaning:
      "IP rejected the submit run, or it was cancelled while waiting for approval. Nothing new left the private forge.",
    next: "Address the review, then dispatch Uplink submit again; it waits for approval once more.",
  },
  conflict: {
    meaning:
      "No longer applies on the new upstream. Company main is frozen at the last good rebuild until it is fixed.",
    next: "The owner fixes it on uplink/conflict/<id>-work and merges the gated pull request. Resolve then rebuilds.",
  },
  merged: {
    meaning: "Upstream has it. It is never applied again.",
    next: "Nothing. Upstream maintains it now.",
  },
  dropped: {
    meaning: "Removed from the queue by an operator. It is no longer applied.",
    next: "Nothing.",
  },
  // Labels below are not upstream statuses.
  active: {
    meaning:
      "A tooling or internal patch carried on company main. It is never exported.",
    next: "Nothing. Every rebuild applies it.",
  },
  "needs approval": {
    meaning:
      "The lines the patch adds or removes, its subject or its public message changed since its last approval. Submit refuses it as it is.",
    next: "Dispatch Uplink submit: IP reviews a new packet, and the approval covers the current content.",
  },
  tooling: {
    meaning:
      "The tooling layer: the forge pack and Uplink configuration, applied first on company main.",
    next: "Nothing. It is never exported.",
  },
  upstream: {
    meaning:
      "The upstream layer: patches meant to be contributed upstream after IP approval.",
    next: "Dispatch Uplink submit when a patch should go upstream.",
  },
  internal: {
    meaning:
      "The internal layer: company-only patches carried on company main.",
    next: "Nothing. They are never exported.",
  },
  "internal-only": {
    meaning:
      "A patch marked internal-only. It stays on company main and is never submitted upstream.",
    next: "Nothing. Submit refuses it.",
  },
};

export const NEEDS_APPROVAL = "needs approval";

export const STATUS_ORDER = [
  "queued",
  "waiting",
  "approved",
  "submitted",
  "rejected",
  "conflict",
  "merged",
  "dropped",
];

export const LABEL_ORDER = [
  "active",
  NEEDS_APPROVAL,
  "tooling",
  "upstream",
  "internal",
  "internal-only",
];

/** The status a badge shows: queued tooling and internal patches are active, since they are never exported. */
export function shownStatus(status: string, layer?: string | null): string {
  if ((layer === "internal" || layer === "tooling") && status === "queued") {
    return "active";
  }
  return status;
}
