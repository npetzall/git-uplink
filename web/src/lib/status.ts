/** What each patch status means and what moves it on. Shown as badge tooltips and in the queue legend. */
export const STATUS_INFO: Record<string, { meaning: string; next: string }> = {
  queued: {
    meaning:
      "Merged internally and in the company build. Nothing has left the private forge; IP has not reviewed it.",
    next: "Dispatch Uplink submit when it should go upstream. Internal-only patches stay queued.",
  },
  approved: {
    meaning: "IP approved the to-upstream Environment. The same run submits it.",
    next: "Submit pushes it to the contribution fork. If it stays here, submit failed (for example preflight); check that run.",
  },
  submitted: {
    meaning: "On the public contribution fork with an open upstream pull request. This is the first time it is public.",
    next: "Maintainers review and merge. Sync then marks it merged.",
  },
  amended: {
    meaning:
      "Submitted, then changed by a conflict resolve. Company main has the new bytes; the fork still has the last approved ones.",
    next: "IP reviews the delta on the re-dispatched Uplink submit. Approval force-pushes the same public pull request.",
  },
  conflict: {
    meaning: "No longer applies on the new upstream. Company main is frozen at the last good rebuild until it is fixed.",
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
  // Not a status: a flag next to approved or submitted.
  "needs approval": {
    meaning:
      "The patch changed since its last approval, for example by a replay onto a moved upstream. Submit refuses it as it is.",
    next: "Dispatch Uplink submit: IP reviews a new packet, and the approval covers the current content.",
  },
};

export const NEEDS_APPROVAL = "needs approval";

export const STATUS_ORDER = ["queued", "approved", "submitted", "amended", "conflict", "merged", "dropped"];
