import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import {
  TOKENS_SALTED,
  TOKENS_SHA256,
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
  syncFromUpstream,
  you,
} from "../ops";

export const soloFix: LabScenario = {
  id: "solo-fix",
  title: "01 — Solo fix",
  blurb:
    "Asha SHA-256: one internal PR, import, to-upstream submit, upstream merge, drop-on-merge, then a maintainer salt follow-up through from-upstream.",
  highlight: "saltedSha256",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-asha",
      title: "Import Asha’s SHA-256 change",
      summary:
        "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Merge plus import records upl_asha as queued. IP has not run. Nothing has left the private forge.",
      why: "One internal branch. The contribution fork branch is derived later from this patch object.",
      operations: [
        branchPush("feat/sha256", "asha-sha256.diff", "Use SHA-256 for tokens"),
        ...importOps({ title: "Use SHA-256 for tokens" }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_asha",
          title: "Use SHA-256 for tokens",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_SHA256),
        };
        return rebuild({
          ...state,
          stepId: "import-asha",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported internal PR #88 as upl_asha. Status queued. src/tokens.js on main calls sha256.",
          ],
        });
      },
    },
    {
      id: "submit-asha",
      title: "IP review, then export",
      summary:
        "Dispatch Uplink submit. IP approves the to-upstream Environment. The same run pushes uplink/upl_asha to the contribution fork and opens public PR #412.",
      why: "Submit is the first time bytes leave the private forge. App credentials exist only on the to-upstream environment.",
      operations: submitOps("upl_asha"),
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_asha" ? { ...patch, status: "submitted" as const, prNumber: 412 } : patch,
        );
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        return {
          ...state,
          stepId: "submit-asha",
          patches,
          contrib: [{ branch: "uplink/upl_asha", files: asha.files, prNumber: 412 }],
          log: [
            ...state.log,
            "Approved to-upstream for upl_asha. Opened public PR #412 from uplink/upl_asha.",
          ],
        };
      },
    },
    {
      id: "flow-back",
      title: "Upstream merges; sync drops the patch",
      summary:
        "Maintainers squash-merge PR #412. Hourly sync sees only Asha’s trailer, skips from-upstream, marks upl_asha merged, and never applies it again. Company main has SHA-256 because it is on upstream.",
      why: "Drop-on-merge is the link between the internal patch and the upstream PR. Re-applying the old delta would fight later maintainer edits.",
      operations: [mergeUpstream("412"), ...syncFlowBack()],
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_asha"
            ? { ...patch, status: "merged" as const, mergedVia: "pr #412" }
            : patch,
        );
        return rebuild({
          ...state,
          stepId: "flow-back",
          upstream: tree(TOKENS_SHA256),
          patches,
          contrib: [],
          log: [
            ...state.log,
            "PR #412 merged. Sync dropped upl_asha. Company main matches upstream (sha256).",
          ],
        });
      },
    },
    {
      id: "salt-follow-up",
      title: "Maintainer salts the hash",
      summary:
        "Upstream lands a follow-up that salts the hasher. That commit is not a company patch, so inspect waits on from-upstream. After approval, company main has saltedSha256 and does not re-apply Asha’s old return sha256(value).",
      why: "Foreign public commits must pass inbound review. Flow-back of our own patches does not.",
      operations: [
        you(
          [
            "git fetch origin && git checkout main && git reset --hard origin/main",
            'git apply "$KIT/patches/upstream-salt-hash.diff"',
            'git commit -am "follow-up: salt the hash"',
            "git push origin main",
          ],
          "In the upstream clone.",
        ),
        ...syncFromUpstream(),
      ],
      apply: (state) =>
        rebuild({
          ...state,
          stepId: "salt-follow-up",
          upstream: tree(TOKENS_SALTED),
          log: [
            ...state.log,
            "from-upstream approved the salt follow-up. Company main has saltedSha256. upl_asha stays merged.",
          ],
        }),
    },
  ],
};
