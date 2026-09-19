export type QueueLayer = "upstream" | "internal";
export type Status =
  | "queued"
  | "approved"
  | "submitted"
  | "amended"
  | "merged"
  | "dropped"
  | "conflict";

export type SimFileMap = Record<string, string>;

export type SimPatch = {
  id: string;
  title: string;
  queue: QueueLayer;
  status: Status;
  dependsOn: string[];
  files: SimFileMap;
  prNumber?: number;
  mergedVia?: string;
};

export type SimConflict = {
  patchId: string;
  branch: string;
  blockedIds: string[];
  files: SimFileMap;
  phase: "stopped" | "fixed";
};

export type SimState = {
  stepId: string;
  upstream: SimFileMap;
  company: SimFileMap;
  contrib: { branch: string; files: SimFileMap; prNumber?: number }[];
  patches: SimPatch[];
  log: string[];
  conflict?: SimConflict;
};

const README = "tokenkit\n";

function tree(tokens: string): SimFileMap {
  return { "src/tokens.js": tokens, "README.md": README };
}

function withVendor(tokens: string): SimFileMap {
  return tree(
    `${tokens.trimEnd()}\n\nexport function vendorTelemetry() {\n  return "internal-only";\n}\n`,
  );
}

const TOKENS_SHA1 = `export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_SHA256 = `export function hash(value) {
  return sha256(value);
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_SHA256_LOGS = `export function hash(value) {
  console.log("hash", value);
  return sha256(value);
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_SALTED = `export function hash(value) {
  return saltedSha256(value);
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_SALTED_LOGS = `export function hash(value) {
  console.log("hash", value);
  return saltedSha256(value);
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_DIGEST = `export function hash(value) {
  const digest = saltedSha256(value);
  return digest;
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_CONFLICT = `export function hash(value) {
<<<<<<< HEAD (upstream/main)
  const digest = saltedSha256(value);
  return digest;
=======
  console.log("hash", value);
  return saltedSha256(value);
>>>>>>> upl_logs
}

export function ttl() {
  return 3600;
}
`;

const TOKENS_RESOLVED = `export function hash(value) {
  const digest = saltedSha256(value);
  console.log("hash", value);
  return digest;
}

export function ttl() {
  return 3600;
}
`;

function clone(files: SimFileMap): SimFileMap {
  return { ...files };
}

export function initialState(): SimState {
  return {
    stepId: "start",
    upstream: tree(TOKENS_SHA1),
    company: tree(TOKENS_SHA1),
    contrib: [],
    patches: [],
    log: ["Company mirror created from public upstream."],
  };
}

function applyPatches(upstream: SimFileMap, patches: SimPatch[]): SimFileMap {
  const next = clone(upstream);
  const active = (patch: SimPatch) => patch.status !== "merged" && patch.status !== "dropped";
  const ordered = [
    ...patches.filter((patch) => patch.queue === "upstream" && active(patch)),
    ...patches.filter((patch) => patch.queue === "internal" && active(patch)),
  ];
  for (const patch of ordered) {
    if (patch.status === "conflict") break;
    Object.assign(next, patch.files);
  }
  return next;
}

function rebuild(state: SimState): SimState {
  return { ...state, company: applyPatches(state.upstream, state.patches) };
}

export const LAB_STEPS: {
  id: string;
  title: string;
  summary: string;
  why: string;
  apply: (state: SimState) => SimState;
}[] = [
  {
    id: "carry-hash",
    title: "Developer lands one internal change",
    summary:
      "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Engineering review plus merge is internal product approval: Uplink imports that PR as a queued patch. She never opens a second branch for upstream. IP has not run yet.",
    why: "Developers keep a normal GitHub Enterprise workflow. The upstream fork branch is derived later from this patch object.",
    apply: (state) => {
      const patch: SimPatch = {
        id: "upl_hash",
        title: "Use SHA-256 for tokens",
        queue: "upstream",
        status: "queued",
        dependsOn: [],
        files: tree(TOKENS_SHA256),
      };
      return rebuild({
        ...state,
        stepId: "carry-hash",
        patches: [...state.patches, patch],
        log: [
          ...state.log,
          "Imported internal PR #88 as upl_hash. Merged to company main and recorded on uplink/state.",
        ],
      });
    },
  },
  {
    id: "stack-logs",
    title: "Build on the unmerged contribution",
    summary:
      "Ben branches from company main, which already includes SHA-256, and adds structured logging. Uplink stores that as a second patch stacked on the first.",
    why: "The company can ship and test on top of work that is not public yet, without merging a long-lived product fork by hand.",
    apply: (state) => {
      const patch: SimPatch = {
        id: "upl_logs",
        title: "Log token hashes",
        queue: "upstream",
        status: "queued",
        dependsOn: ["upl_hash"],
        files: tree(TOKENS_SHA256_LOGS),
      };
      return rebuild({
        ...state,
        stepId: "stack-logs",
        patches: [...state.patches, patch],
        log: [...state.log, "Imported PR #91 as upl_logs, stacked on upl_hash."],
      });
    },
  },
  {
    id: "internal-only",
    title: "Escape hatch: never-upstream patch",
    summary:
      "Compliance needs a vendor telemetry hook that must not leave the private forge. It is labeled internal-only. It still rebases onto upstream, but Uplink will refuse to export it.",
    why: "Odd case, but supported. The default remains: every other internal change is intended for upstream.",
    apply: (state) => {
      const patch: SimPatch = {
        id: "upl_vendor",
        title: "Vendor telemetry hook",
        queue: "internal",
        status: "queued",
        dependsOn: [],
        files: withVendor(TOKENS_SHA256_LOGS),
      };
      return rebuild({
        ...state,
        stepId: "internal-only",
        patches: [...state.patches, patch],
        log: [
          ...state.log,
          "Imported upl_vendor as internal-only. It will never be pushed to the contribution fork.",
        ],
      });
    },
  },
  {
    id: "approve-submit",
    title: "IP review, then one export",
    summary:
      "Legal approves the to-upstream GitHub Environment on the waiting submit run — a second gate, after the patch was already on company main. The same run records the receipt, pushes a generated branch to the public contribution fork, and opens a PR against public main. Asha still has only her original internal branch.",
    why: "The IP gate is GitHub Environment to-upstream (audit log + Deployments). The change stays private until submit lands it on the contribution fork. Maintainers merge a normal GitHub PR.",
    apply: (state) => {
      const patches = state.patches.map((patch) =>
        patch.id === "upl_hash"
          ? { ...patch, status: "submitted" as const, prNumber: 412 }
          : patch,
      );
      const hash = patches.find((patch) => patch.id === "upl_hash")!;
      return {
        ...state,
        stepId: "approve-submit",
        patches,
        contrib: [
          {
            branch: "uplink/upl_hash",
            files: hash.files,
            prNumber: 412,
          },
        ],
        log: [
          ...state.log,
          "Approved to-upstream environment for upl_hash. Pushed uplink/upl_hash to the contribution fork and opened public PR #412.",
        ],
      };
    },
  },
  {
    id: "upstream-merges",
    title: "Maintainer merges, later hardens the change",
    summary:
      "Upstream squash-merges PR #412. They then land a follow-up that salts the hash. Because Uplink recorded the PR (and the Uplink-Patch-Id trailer), the original patch is dropped and the salt fix is kept. The logging delta still applies.",
    why: "If the company kept applying its old SHA-256 patch, the later salt fix would be reverted on the next sync. Drop-on-merge is the whole point of the link between the internal patch and the upstream PR.",
    apply: (state) => {
      const patches = state.patches.map((patch) => {
        if (patch.id === "upl_hash") {
          return { ...patch, status: "merged" as const, mergedVia: "pr #412" };
        }
        if (patch.id === "upl_logs") {
          return { ...patch, files: tree(TOKENS_SALTED_LOGS) };
        }
        if (patch.id === "upl_vendor") {
          return { ...patch, files: withVendor(TOKENS_SALTED_LOGS) };
        }
        return patch;
      });
      return rebuild({
        ...state,
        stepId: "upstream-merges",
        upstream: tree(TOKENS_SALTED),
        patches,
        contrib: [],
        log: [
          ...state.log,
          "PR #412 merged. Dropped upl_hash. Rebased remaining patches onto saltedSha256. Fork branch uplink/upl_hash is done.",
        ],
      });
    },
  },
  {
    id: "conflict",
    title: "Sync stops: upstream overlaps a pending patch",
    summary:
      "Someone rewrites hash() to bind the digest before returning. The logging patch still inserts console.log next to return saltedSha256(value), so git apply fails. Sync records upl_logs as conflict, opens uplink/conflict/upl_logs, and does not move company main.",
    why: "Sync must fail closed. Later patches — including independent internal-only work — wait. The contribution fork stays empty of logs because that patch was never IP-approved; a conflict is not an export.",
    apply: (state) => {
      const patches = state.patches.map((patch) =>
        patch.id === "upl_logs"
          ? { ...patch, status: "conflict" as const }
          : patch,
      );
      return {
        ...state,
        stepId: "conflict",
        upstream: tree(TOKENS_DIGEST),
        patches,
        company: state.company,
        contrib: state.contrib,
        conflict: {
          patchId: "upl_logs",
          branch: "uplink/conflict/upl_logs",
          blockedIds: ["upl_vendor"],
          files: tree(TOKENS_CONFLICT),
          phase: "stopped",
        },
        log: [
          ...state.log,
          "Sync conflict on upl_logs. Opened uplink/conflict/upl_logs. Company main not rebuilt. upl_vendor not applied.",
        ],
      };
    },
  },
  {
    id: "conflict-fix",
    title: "Checkout the conflict branch and fix",
    summary:
      "Ben fetches, checks out uplink/conflict/upl_logs, and edits the conflicted file. He keeps upstream’s digest local and logs after it. git add stages the resolution. The patch id is unchanged. Status is still conflict until resolve runs.",
    why: "Humans fix files on the conflict branch. They do not hand-edit company main or the contribution fork. Those refs are derived from the patch object after resolve.",
    apply: (state) => {
      if (!state.conflict) return state;
      return {
        ...state,
        stepId: "conflict-fix",
        conflict: {
          ...state.conflict,
          files: tree(TOKENS_RESOLVED),
          phase: "fixed",
        },
        log: [
          ...state.log,
          "Checked out uplink/conflict/upl_logs. Resolved markers in src/tokens.js and staged the file.",
        ],
      };
    },
  },
  {
    id: "resolve",
    title: "Resolve the same patch id",
    summary:
      "git uplink resolve upl_logs rewrites only that patch file, then rebuilds. Remaining patches replay: vendor telemetry applies again. upl_logs returns to queued. Nothing is pushed to the upstream-owned fork.",
    why: "One patch identity. Internal conflict resolution amends the same object. Resolve is not submit: queued work still needs the to-upstream Environment before it can leave the private forge.",
    apply: (state) => {
      const patches = state.patches.map((patch) => {
        if (patch.id === "upl_logs") {
          return { ...patch, status: "queued" as const, files: tree(TOKENS_RESOLVED) };
        }
        if (patch.id === "upl_vendor") {
          return { ...patch, files: withVendor(TOKENS_RESOLVED) };
        }
        return patch;
      });
      return rebuild({
        ...state,
        stepId: "resolve",
        patches,
        conflict: undefined,
        contrib: state.contrib,
        log: [
          ...state.log,
          "Resolved upl_logs. Company main rebuilt (upstream + amended logs + vendor). Fork unchanged — still no IP approval for logs.",
        ],
      });
    },
  },
  {
    id: "submit-logs",
    title: "IP review, then export the amended patch",
    summary:
      "Legal approves the to-upstream GitHub Environment for upl_logs. The same run pushes uplink/upl_logs — the amended log line on current public main — and opens PR #418. vendorTelemetry is not in that tree.",
    why: "Bytes leave the private forge only after to-upstream approval. The fork branch is generated from the patch, so the conflict resolution is what upstream reviews. Internal-only patches still never export.",
    apply: (state) => {
      const patches = state.patches.map((patch) =>
        patch.id === "upl_logs"
          ? { ...patch, status: "submitted" as const, prNumber: 418 }
          : patch,
      );
      const logs = patches.find((patch) => patch.id === "upl_logs")!;
      return {
        ...state,
        stepId: "submit-logs",
        patches,
        contrib: [
          {
            branch: "uplink/upl_logs",
            files: logs.files,
            prNumber: 418,
          },
        ],
        log: [
          ...state.log,
          "Approved to-upstream environment for upl_logs. Pushed uplink/upl_logs to the contribution fork and opened public PR #418.",
        ],
      };
    },
  },
];

export function runThrough(count: number): SimState {
  let state = initialState();
  for (const step of LAB_STEPS.slice(0, count)) {
    state = step.apply(state);
  }
  return state;
}
