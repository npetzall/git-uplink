import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import { TOKENS_SHA256, TOKENS_SHA256_LOGS, exampleSeed, tree } from "../fixtures";
import {
  branchPush,
  ciJob,
  importOps,
  preflightCi,
  resetStatus,
  startExample,
  submitOps,
  you,
} from "../ops";

export const stackedDependsOn: LabScenario = {
  id: "stacked-depends-on",
  title: "03 — Stacked depends-on",
  blurb:
    "Ben’s log line needs Asha’s SHA-256. Preflight without the trailer fails. Submit of Ben is refused until Asha is submitted.",
  highlight: "console.log",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-asha",
      title: "Import Asha (do not submit yet)",
      summary:
        "Same as solo fix through import. Copy Asha’s upl_… id. Company main already has sha256. IP has not run.",
      why: "Ben can build on queued work. He does not wait for IP.",
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
          log: [...state.log, "Imported upl_asha as queued. Do not submit yet."],
        });
      },
    },
    {
      id: "preflight-fail",
      title: "Preflight without depends-on fails",
      summary:
        "Ben opens a PR whose body omits Uplink-Depends-On. Export preflight applies his diff onto public main (still sha1) and fails. The workflow comments suggested trailer lines. Do not merge. Close the PR.",
      why: "Company main is not allowed to become the silent base of a later incomplete upstream PR.",
      operations: [
        branchPush("feat/ben-log-nodep", "ben-log.diff", "Log token hashes"),
        you(
          [
            'git uplink preflight --title "Log token hashes" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>',
          ],
          "Local check; the PR body has no Uplink-Depends-On line.",
        ),
        preflightCi("Log token hashes"),
      ],
      apply: (state) => ({
        ...state,
        stepId: "preflight-fail",
        log: [
          ...state.log,
          "Export preflight failed: Ben’s diff does not apply on public main without Uplink-Depends-On: upl_asha.",
        ],
      }),
    },
    {
      id: "import-ben",
      title: "Import Ben with the trailer",
      summary:
        "Open a new PR whose body has Uplink-Depends-On: upl_asha. Checks pass. Merge. Queue: Asha then Ben; Ben lists dependsOn. src/tokens.js logs then returns sha256.",
      why: "Stacking is recorded at import. Rebuild applies Asha then Ben.",
      operations: [
        branchPush("feat/ben-log", "ben-log.diff", "Log token hashes"),
        ...importOps({ title: "Log token hashes", dependsOn: ["upl_asha"] }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_ben",
          title: "Log token hashes",
          queue: "upstream",
          status: "queued",
          dependsOn: ["upl_asha"],
          files: tree(TOKENS_SHA256_LOGS),
        };
        return rebuild({
          ...state,
          stepId: "import-ben",
          patches: [...state.patches, patch],
          log: [...state.log, "Imported upl_ben with dependsOn upl_asha. Both queued."],
        });
      },
    },
    {
      id: "submit-ben-first",
      title: "Submit Ben first — refused",
      summary:
        "Uplink submit for Ben fails: Submit upl_asha before upl_ben. The contrib fork is unchanged.",
      why: "An upstream-bound patch may not export until each unmerged dependency is submitted or merged.",
      operations: [
        you(["git uplink approve upl_ben", "git uplink submit upl_ben"], "Refused: Submit upl_asha before upl_ben."),
        ciJob(
          "Uplink submit",
          ["git uplink init", "git uplink approve upl_ben", "git uplink submit upl_ben"],
          "Job fails. Fork unchanged.",
        ),
      ],
      apply: (state) => ({
        ...state,
        stepId: "submit-ben-first",
        log: [...state.log, "Submit upl_ben refused: Submit upl_asha before upl_ben."],
      }),
    },
    {
      id: "submit-order",
      title: "Submit Asha, then Ben",
      summary:
        "Submit Asha (approve to-upstream), then Ben. Ben’s public PR is stacked on Asha’s contrib branch until Asha merges.",
      why: "Submit order follows dependsOn. Ben’s export tree includes Asha until she is merged upstream.",
      operations: [...submitOps("upl_asha"), ...submitOps("upl_ben")],
      apply: (state) => {
        const patches = state.patches.map((patch) => {
          if (patch.id === "upl_asha") return { ...patch, status: "submitted" as const, prNumber: 412 };
          if (patch.id === "upl_ben") return { ...patch, status: "submitted" as const, prNumber: 418 };
          return patch;
        });
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        const ben = patches.find((patch) => patch.id === "upl_ben")!;
        return {
          ...state,
          stepId: "submit-order",
          patches,
          contrib: [
            { branch: "uplink/upl_asha", files: asha.files, prNumber: 412 },
            { branch: "uplink/upl_ben", files: ben.files, prNumber: 418 },
          ],
          log: [
            ...state.log,
            "Submitted upl_asha (#412), then upl_ben (#418) stacked on Asha’s contrib branch.",
          ],
        };
      },
    },
  ],
};
