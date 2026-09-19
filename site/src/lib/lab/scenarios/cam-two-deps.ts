import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import {
  TOKENS_CAM,
  TOKENS_SHA256,
  TOKENS_SHA256_TTL_7200,
  exampleSeed,
  tree,
} from "../fixtures";
import {
  branchPush,
  ciJob,
  importOps,
  mergeUpstream,
  resetStatus,
  startExample,
  submitOps,
  syncFlowBack,
  you,
} from "../ops";

export const camTwoDeps: LabScenario = {
  id: "cam-two-deps",
  title: "05 — Cam on two siblings",
  blurb:
    "Cam’s helper calls Asha’s hash and Ben’s TTL. Submit of Cam is refused until both siblings have merged upstream — not merely submitted.",
  highlight: "describeToken",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-siblings",
      title: "Import Asha and Ben (independent)",
      summary:
        "Both patches apply to seed main. After both imports, company main has sha256 and ttl() == 7200. Copy both ids.",
      why: "Cam must branch after both bases are queued. Order of Asha vs Ben on main does not matter to Cam.",
      operations: [
        branchPush("feat/sha256", "asha-sha256.diff", "Use SHA-256 for tokens"),
        you(["git checkout main"]),
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
          stepId: "import-siblings",
          patches: [...state.patches, asha, ben],
          log: [...state.log, "Imported upl_asha and upl_ben. Both queued, independent."],
        });
      },
    },
    {
      id: "import-cam",
      title: "Import Cam with both trailers",
      summary:
        "Cam’s PR body lists Uplink-Depends-On for both ids. Submit of Cam is refused until each upstream-bound dependency is submitted or merged.",
      why: "Cam’s patch file is only Cam’s unique delta against a tree that already had Asha and Ben.",
      operations: [
        branchPush("feat/cam", "cam-wire.diff", "Wire hash into a describe helper"),
        ...importOps({ title: "Wire hash into a describe helper", dependsOn: ["upl_asha", "upl_ben"] }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_cam",
          title: "Wire hash into a describe helper",
          queue: "upstream",
          status: "queued",
          dependsOn: ["upl_asha", "upl_ben"],
          files: tree(TOKENS_CAM),
        };
        return rebuild({
          ...state,
          stepId: "import-cam",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported upl_cam with dependsOn upl_asha, upl_ben. Submit of Cam still blocked.",
          ],
        });
      },
    },
    {
      id: "merge-siblings",
      title: "Export and merge the siblings first",
      summary:
        "Submit Asha and Ben; squash-merge both upstream PRs; sync. After sync they are merged. Cam is the leftover delta on public main. Do not submit Cam while the siblings are only submitted — that would apply Cam onto one sibling fork branch, which lacks the other.",
      why: "For a patch that depends on two independent patches, wait until those two have merged upstream.",
      operations: [
        you(
          ["git uplink approve upl_cam", "git uplink submit upl_cam"],
          "Refused while siblings are only queued/submitted. Wait until both are merged.",
        ),
        ciJob(
          "Uplink submit",
          ["git uplink init", "git uplink approve upl_cam", "git uplink submit upl_cam"],
          "Refused: Submit upl_asha before upl_cam (and the same for Ben). Do not export Cam onto a sibling fork branch.",
        ),
        ...submitOps("upl_asha"),
        ...submitOps("upl_ben"),
        mergeUpstream("412"),
        mergeUpstream("413"),
        ...syncFlowBack(),
      ],
      apply: (state) => {
        const patches = state.patches.map((patch) => {
          if (patch.id === "upl_asha") {
            return { ...patch, status: "merged" as const, mergedVia: "pr #412", prNumber: 412 };
          }
          if (patch.id === "upl_ben") {
            return { ...patch, status: "merged" as const, mergedVia: "pr #413", prNumber: 413 };
          }
          return patch;
        });
        return rebuild({
          ...state,
          stepId: "merge-siblings",
          upstream: tree(TOKENS_SHA256_TTL_7200),
          patches,
          contrib: [],
          log: [
            ...state.log,
            "Asha and Ben merged upstream. Sync dropped both. Cam is the leftover delta on public main.",
          ],
        });
      },
    },
    {
      id: "submit-cam",
      title: "Submit Cam’s leftover",
      summary:
        "Dispatch Uplink submit for Cam. The public PR is describeToken only — not a replay of Asha or Ben.",
      why: "Both dependsOn are merged, so submit applies Cam onto current public main.",
      operations: submitOps("upl_cam"),
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_cam" ? { ...patch, status: "submitted" as const, prNumber: 420 } : patch,
        );
        const cam = patches.find((patch) => patch.id === "upl_cam")!;
        return {
          ...state,
          stepId: "submit-cam",
          patches,
          contrib: [{ branch: "uplink/upl_cam", files: cam.files, prNumber: 420 }],
          log: [
            ...state.log,
            "Submitted upl_cam (#420). Public tree is describeToken on sha256 + ttl 7200; no replay of the siblings.",
          ],
        };
      },
    },
  ],
};
