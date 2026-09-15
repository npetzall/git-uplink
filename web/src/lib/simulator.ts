export type Intent = "upstream" | "internal-only";
export type Status =
  | "queued"
  | "approved"
  | "submitted"
  | "merged"
  | "dropped"
  | "conflict";

export type SimFileMap = Record<string, string>;

export type SimPatch = {
  id: string;
  title: string;
  intent: Intent;
  status: Status;
  dependsOn: string[];
  files: SimFileMap;
  prNumber?: number;
  mergedVia?: string;
};

export type SimState = {
  stepId: string;
  upstream: SimFileMap;
  company: SimFileMap;
  contrib: { branch: string; files: SimFileMap; prNumber?: number }[];
  patches: SimPatch[];
  log: string[];
  conflict?: { patchId: string; ours: string; theirs: string };
};

const BASE: SimFileMap = {
  "src/tokens.js": `export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 3600;
}
`,
  "README.md": "tokenkit\n",
};

function clone(files: SimFileMap): SimFileMap {
  return { ...files };
}

export function initialState(): SimState {
  return {
    stepId: "start",
    upstream: clone(BASE),
    company: clone(BASE),
    contrib: [],
    patches: [],
    log: ["Company mirror created from public upstream."],
  };
}

function applyPatches(upstream: SimFileMap, patches: SimPatch[]): SimFileMap {
  const next = clone(upstream);
  for (const patch of patches) {
    if (patch.status === "merged" || patch.status === "dropped") continue;
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
      "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Engineering review plus the uplink:import label is internal product approval: Uplink imports that PR as a queued patch. She never opens a second branch for upstream. IP has not run yet.",
    why: "Developers keep a normal GitHub Enterprise workflow. The upstream fork branch is derived later from this patch object.",
    apply: (state) => {
      const files = {
        "src/tokens.js": state.upstream["src/tokens.js"].replace(
          "return sha1(value);",
          "return sha256(value);",
        ),
        "README.md": state.upstream["README.md"],
      };
      const patch: SimPatch = {
        id: "upl_hash",
        title: "Use SHA-256 for tokens",
        intent: "upstream",
        status: "queued",
        dependsOn: [],
        files,
      };
      return rebuild({
        ...state,
        stepId: "carry-hash",
        patches: [...state.patches, patch],
        log: [
          ...state.log,
          "Imported internal PR #88 as upl_hash. Company main rebuilt: upstream + this patch.",
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
      const files = {
        "src/tokens.js": state.company["src/tokens.js"].replace(
          "return sha256(value);",
          'console.log("hash", value);\n  return sha256(value);',
        ),
        "README.md": state.company["README.md"],
      };
      const patch: SimPatch = {
        id: "upl_logs",
        title: "Log token hashes",
        intent: "upstream",
        status: "queued",
        dependsOn: ["upl_hash"],
        files,
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
      "Compliance needs a vendor telemetry hook that must not leave the enterprise. It is labeled internal-only. It still rebases onto upstream, but Uplink will refuse to export it.",
    why: "Odd case, but supported. The default remains: every other internal change is intended for upstream.",
    apply: (state) => {
      const files = {
        "src/tokens.js": `${state.company["src/tokens.js"].trimEnd()}\n\nexport function vendorTelemetry() {\n  return "emu-only";\n}\n`,
        "README.md": state.company["README.md"],
      };
      const patch: SimPatch = {
        id: "upl_vendor",
        title: "Vendor telemetry hook",
        intent: "internal-only",
        status: "queued",
        dependsOn: [],
        files,
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
      "Legal approves the oss GitHub Environment on the waiting submit run — a second gate, after the patch was already on company main. The same run records the receipt, pushes a generated branch to the upstream-owned private fork, and opens a PR against public main. Asha still has only her original internal branch.",
    why: "The IP gate is GitHub Environment oss (audit log + Deployments). The secrecy airlock is the private fork. The public PR is created only after that review. Maintainers merge a normal GitHub PR.",
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
          "Approved oss environment for upl_hash. Pushed uplink/upl_hash to the private fork and opened public PR #412.",
        ],
      };
    },
  },
  {
    id: "upstream-merges",
    title: "Maintainer merges, later hardens the change",
    summary:
      "Upstream squash-merges PR #412. They then land a follow-up that salts the hash. Because Uplink recorded the PR (and the Uplink-Patch-Id trailer), the original patch is dropped and the salt fix is kept.",
    why: "If the company kept applying its old SHA-256 patch, the later salt fix would be reverted on the next sync. Drop-on-merge is the whole point of the link between the internal patch and the upstream PR.",
    apply: (state) => {
      const upstream = {
        ...state.upstream,
        "src/tokens.js": state.upstream["src/tokens.js"].replace(
          "return sha1(value);",
          "return saltedSha256(value);",
        ),
      };
      const logsFiles: SimFileMap = {
        "src/tokens.js": `export function hash(value) {
  console.log("hash", value);
  return saltedSha256(value);
}

export function ttl() {
  return 3600;
}
`,
        "README.md": "tokenkit\n",
      };
      const vendorFiles: SimFileMap = {
        "src/tokens.js": `${logsFiles["src/tokens.js"].trimEnd()}

export function vendorTelemetry() {
  return "emu-only";
}
`,
        "README.md": "tokenkit\n",
      };
      const patches = state.patches.map((patch) => {
        if (patch.id === "upl_hash") {
          return { ...patch, status: "merged" as const, mergedVia: "pr #412" };
        }
        if (patch.id === "upl_logs") {
          return { ...patch, files: logsFiles };
        }
        if (patch.id === "upl_vendor") {
          return { ...patch, files: vendorFiles };
        }
        return patch;
      });
      return rebuild({
        ...state,
        stepId: "upstream-merges",
        upstream,
        patches,
        log: [
          ...state.log,
          "PR #412 merged. Dropped upl_hash. Rebased remaining patches onto saltedSha256.",
        ],
      });
    },
  },
  {
    id: "conflict",
    title: "Unrelated upstream edit conflicts with a pending patch",
    summary:
      "Someone else changes ttl() from 3600 to 1800. The logging patch still applies. A later company TTL patch would conflict — here we collide with the remaining hash-adjacent logging context after another overlapping edit, then stop the queue on upl_logs after an upstream rewrite of the hash function body.",
    why: "Sync must fail closed. The pending upstream contribution is amended in the same patch object so the open PR can be force-pushed from the refreshed patch.",
    apply: (state) => {
      const upstream = {
        ...state.upstream,
        "src/tokens.js": `export function hash(value) {
  return saltedSha256(value);
}

export function ttl() {
  return 1800;
}
`,
      };
      const patches = state.patches.map((patch) =>
        patch.id === "upl_logs"
          ? { ...patch, status: "conflict" as const }
          : patch,
      );
      return {
        ...state,
        stepId: "conflict",
        upstream,
        patches,
        company: applyPatches(upstream, patches),
        conflict: {
          patchId: "upl_logs",
          ours: 'console.log("hash", value);',
          theirs: "saltedSha256 already landed; log line needs a new home.",
        },
        log: [
          ...state.log,
          "Sync conflict on upl_logs. Opened internal PR against uplink/conflict/upl_logs.",
        ],
      };
    },
  },
  {
    id: "amend",
    title: "Resolve once; the upstream PR is amended",
    summary:
      "Ben fixes the conflict by logging after saltedSha256. Uplink refreshes upl_logs and, because that patch was destined for upstream, the next submit/sync force-pushes the contribution fork branch. Company main now matches upstream plus the amended log line plus vendor telemetry.",
    why: "One patch identity. Internal conflict resolution and upstream review comments both amend the same object, so nobody maintains a shadow branch.",
    apply: (state) => {
      const files = {
        "src/tokens.js": `export function hash(value) {
  const digest = saltedSha256(value);
  console.log("hash", value);
  return digest;
}

export function ttl() {
  return 1800;
}

export function vendorTelemetry() {
  return "emu-only";
}
`,
        "README.md": "tokenkit\n",
      };
      const patches = state.patches.map((patch) => {
        if (patch.id === "upl_logs") {
          return { ...patch, status: "queued" as const, files };
        }
        if (patch.id === "upl_vendor") {
          return {
            ...patch,
            files: {
              "src/tokens.js": files["src/tokens.js"],
              "README.md": files["README.md"],
            },
          };
        }
        return patch;
      });
      const next = rebuild({
        ...state,
        stepId: "amend",
        patches,
        conflict: undefined,
        contrib: [
          ...state.contrib.filter((branch) => branch.branch !== "uplink/upl_logs"),
          { branch: "uplink/upl_logs", files, prNumber: undefined },
        ],
        log: [
          ...state.log,
          "Amended upl_logs. Company main rebuilt. Contribution branch refreshed for the next submit.",
        ],
      });
      return next;
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
