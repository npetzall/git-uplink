import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import {
  TOKENS_SHA256,
  TOKENS_SHA256_TTL_7200,
  TOKENS_TTL_7200,
  exampleSeed,
  tree,
} from "../fixtures";
import {
  branchPush,
  importOps,
  mergeUpstream,
  resetStatus,
  startExample,
  submitOps,
  syncFlowBack,
  you,
} from "../ops";

export const parallelIndependent: LabScenario = {
  id: "parallel-independent",
  title: "02 — Parallel independent",
  blurb:
    "Asha hash + Ben TTL from the same baseline. Merge Ben’s upstream PR first; Asha stays queued and still applies on company main.",
  highlight: "7200",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-both",
      title: "Import two independent PRs",
      summary:
        "Asha and Ben branch from seed main. Neither records Uplink-Depends-On. Merge Asha first, then Ben. Import is serialized by uplink-mutate. Both patches are queued. src/tokens.js has sha256 and ttl() == 7200.",
      why: "Queue order is not upstream order. Independent diffs land without a dependsOn trailer.",
      operations: [
        branchPush("feat/sha256", "asha-sha256.diff", "Use SHA-256 for tokens"),
        you(["git checkout main"], "Then Ben from the same baseline."),
        branchPush("feat/ttl", "ben-ttl.diff", "Extend TTL"),
        ...importOps({ title: "Use SHA-256 for tokens" }),
        ...importOps({ title: "Extend TTL" }),
        resetStatus(),
      ],
      apply: (state) => {
        const asha: SimPatch = {
          id: "upl_asha",
          title: "Use SHA-256 for tokens",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_SHA256),
        };
        const ben: SimPatch = {
          id: "upl_ben",
          title: "Extend TTL",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_SHA256_TTL_7200),
        };
        return rebuild({
          ...state,
          stepId: "import-both",
          patches: [...state.patches, asha, ben],
          log: [
            ...state.log,
            "Imported PR #88 as upl_asha, then PR #89 as upl_ben. Both queued. No dependsOn.",
          ],
        });
      },
    },
    {
      id: "submit-both",
      title: "Submit independently",
      summary:
        "Dispatch Uplink submit for each id (either order). Each public PR is the patch on public main, not stacked on the other.",
      why: "They did not depend on each other, so export trees are independent.",
      operations: [...submitOps("upl_asha"), ...submitOps("upl_ben")],
      apply: (state) => {
        const patches = state.patches.map((patch) => {
          if (patch.id === "upl_asha") return { ...patch, status: "submitted" as const, prNumber: 412 };
          if (patch.id === "upl_ben") return { ...patch, status: "submitted" as const, prNumber: 413 };
          return patch;
        });
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        return {
          ...state,
          stepId: "submit-both",
          patches,
          contrib: [
            { branch: "uplink/upl_asha", files: asha.files, prNumber: 412 },
            { branch: "uplink/upl_ben", files: tree(TOKENS_TTL_7200), prNumber: 413 },
          ],
          log: [
            ...state.log,
            "Submitted upl_asha (#412) and upl_ben (#413). Each PR is the patch on public main.",
          ],
        };
      },
    },
    {
      id: "merge-ben-first",
      title: "Upstream merges Ben first",
      summary:
        "Squash-merge Ben’s public PR, then run Uplink sync. Ben’s trailer is the only new commit, so inspect applies immediately. Ben is merged. Asha stays submitted and still applies on company main.",
      why: "Being first on company main does not mean you must merge first publicly. Drop-on-merge is per patch id.",
      operations: [mergeUpstream("413"), ...syncFlowBack()],
      apply: (state) => {
        const patches = state.patches.map((patch) => {
          if (patch.id === "upl_ben") {
            return { ...patch, status: "merged" as const, mergedVia: "pr #413" };
          }
          if (patch.id === "upl_asha") {
            return { ...patch, files: tree(TOKENS_SHA256_TTL_7200) };
          }
          return patch;
        });
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        return rebuild({
          ...state,
          stepId: "merge-ben-first",
          upstream: tree(TOKENS_TTL_7200),
          patches,
          contrib: [{ branch: "uplink/upl_asha", files: asha.files, prNumber: 412 }],
          log: [
            ...state.log,
            "PR #413 merged. Dropped upl_ben. upl_asha still submitted and applied (sha256 + ttl 7200).",
          ],
        });
      },
    },
    {
      id: "merge-asha",
      title: "Asha flows back too",
      summary:
        "Merge Asha’s upstream PR and sync again. Both patches are merged. Company main matches public main.",
      why: "After both drop, nothing is left to replay except the internal-only tooling patch.",
      operations: [mergeUpstream("412"), ...syncFlowBack()],
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_asha"
            ? { ...patch, status: "merged" as const, mergedVia: "pr #412" }
            : patch,
        );
        return rebuild({
          ...state,
          stepId: "merge-asha",
          upstream: tree(TOKENS_SHA256_TTL_7200),
          patches,
          contrib: [],
          log: [...state.log, "PR #412 merged. Dropped upl_asha. Company main matches upstream."],
        });
      },
    },
  ],
};
