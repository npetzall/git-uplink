/** What each patch status and label means and what moves it on. Shown as badge tooltips and in the lab legend. */
export type StatusInfo = { meaning: string; next: string };

export const STATUS_INFO: Record<string, StatusInfo> = {
  queued: {
    meaning:
      "Merged internally and in the company build. Nothing has left the private forge; IP has not reviewed it.",
    next: "Dispatch Uplink submit when it should go upstream; with UPLINK_AUTO_SUBMIT the pack does, once its upstream dependencies are merged. An amend or resolve the last approval does not cover makes a patch queued again.",
  },
  waiting: {
    meaning:
      "A submit run passed every check: the assessment is ok, its upstream dependencies are merged, and preflight passed. It waits on IP in the to-upstream Environment.",
    next: "IP reviews the packet. Approval makes it approved; a rejection or a cancelled run makes it rejected.",
  },
  approved: {
    meaning:
      "IP approved the to-upstream Environment. The same run submits it.",
    next: "Submit pushes it to the contribution fork. If it stays here, submit failed; check that run.",
  },
  submitted: {
    meaning:
      "On the public contribution fork with an open upstream pull request. This is the first time it is public.",
    next: "Maintainers review and merge. Sync then marks it merged. A later change goes to IP as a delta review on the same public pull request.",
  },
  rejected: {
    meaning:
      "IP rejected the submit run, or it was cancelled while waiting. Nothing new was made public.",
    next: "Fix what IP asked for, then dispatch Uplink submit by hand. A rejected patch is never submitted automatically.",
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
};

export const LABEL_INFO: Record<string, StatusInfo> = {
  active: {
    meaning:
      "A tooling or internal patch that is applied to company main. It never goes upstream.",
    next: "Nothing. Amend it to change it, or drop it.",
  },
  tooling: {
    meaning:
      "Company tooling: the workflows, the pack and the hooks. Applied to company main, never contributed.",
    next: "Changed through a reviewed pull request.",
  },
  upstream: {
    meaning: "Queued for upstream: it goes to IP and then to a public PR.",
    next: "Follows the lifecycle queued, waiting, approved, submitted, merged.",
  },
  internal: {
    meaning:
      "Queued as company-only. It is applied to company main and never submitted.",
    next: "Transfer it to-upstream to contribute it.",
  },
  "internal-only": {
    meaning:
      "The PR label that queues a change as internal. It is not assessed; its check runs only the preflight script.",
    next: "Remove the label before merge to queue it for upstream.",
  },
};

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
  "tooling",
  "upstream",
  "internal",
  "internal-only",
];

export function describe(value: string): StatusInfo | undefined {
  return STATUS_INFO[value] ?? LABEL_INFO[value];
}

/** Internal and tooling patches never go upstream, so "queued" reads as "active" for them. */
export function shownStatus(status: string, layer: string): string {
  if (status === "queued" && (layer === "internal" || layer === "tooling")) {
    return "active";
  }
  return status;
}
